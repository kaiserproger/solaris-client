//! GPU far-terrain generation for servers that share their LOD program
//! (`[lod] share_terrain_keys`). Whole tiles run in one compute dispatch
//! (`far_generate.wgsl`, one invocation per column) and their columns are
//! read back to the session (`Renderer::drain_far_generated`), which colours
//! and stores them like any other sample. `far/program.rs` is the f64
//! reference and the CPU fallback when compute is unavailable.
//!
//! Budget: `TILES_PER_DISPATCH` tiles per frame (timestamp pass
//! `far_terrain_generate`), at most `IN_FLIGHT` readbacks of 2 KiB per tile.
use crate::api::FarGeneration;
use crate::far::program::{Column, Program};
use crate::far::{TileKey, SAMPLES};
use std::collections::{HashSet, VecDeque};
use std::sync::{mpsc, Arc};
use wgpu::util::DeviceExt;

use super::timing::{self, Timing};

/// Tiles per frame's dispatch. A tile's 256 long-running invocations leave
/// the GPU latency-bound: 1 and 16 tiles both measured ~9 ms on the dev GPU,
/// so a 2900-tile fill takes a few seconds of frames instead of minutes.
const TILES_PER_DISPATCH: usize = 16;
const IN_FLIGHT: usize = 3;
const WORKGROUP: usize = 64;

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    geometry: [i32; 4],
    surface: [u32; 4],
    climate: [u32; 4],
    roots_a: [u32; 4],
    roots_b: [u32; 4],
    tables: [u32; 4],
}

/// The generator shader for a program with `registers` registers.
pub(super) fn shader_source(registers: usize) -> String {
    include_str!("far_generate.wgsl").replace("REGISTER_COUNT", &registers.max(1).to_string())
}

/// The program flattened into the shader's word tables (layout in
/// `far_generate.wgsl`); `params.tables[3]` (job count) is set per dispatch.
fn tables(program: &Program) -> (Params, Vec<u32>) {
    let f = |v: f64| (v as f32).to_bits();
    let mut data = Vec::new();
    let octaves = data.len() as u32;
    for octave in &program.octaves {
        for word in octave.p.chunks_exact(4) {
            data.push(u32::from_le_bytes(word.try_into().expect("4 bytes")));
        }
        data.extend(octave.offset.map(f));
    }
    let normals = data.len() as u32;
    let levels_start = normals + program.normals.len() as u32 * 9;
    let mut levels = Vec::new();
    for normal in &program.normals {
        data.push(f(normal.value_factor));
        for perlin in [&normal.first, &normal.second] {
            data.extend([
                f(perlin.input),
                f(perlin.value),
                levels_start + levels.len() as u32,
                perlin.levels.len() as u32,
            ]);
            for (amplitude, octave) in &perlin.levels {
                levels.extend([f(*amplitude), octave.unwrap_or(u32::MAX)]);
            }
        }
    }
    data.extend(levels);
    let blended = data.len() as u32;
    for noise in &program.blended {
        data.extend(noise.octaves);
        data.extend(noise.scales.map(f));
    }
    let mut code = |code: &crate::far::program::Code| {
        let insts = data.len() as u32;
        for inst in &code.insts {
            data.push(u32::from(inst.op));
            data.extend(inst.args);
            data.extend(inst.constants.map(f));
            data.push(0);
        }
        let points = data.len() as u32;
        for point in &code.points {
            data.extend([
                point.location.to_bits(),
                point.derivative.to_bits(),
                u32::from(point.register),
                point.value,
            ]);
        }
        (insts, code.insts.len() as u32, points)
    };
    let surface = code(&program.surface);
    let climate = code(&program.climate);
    let rows = data.len() as u32;
    for (bounds, biome) in &program.rows {
        for value in bounds.iter().flatten() {
            data.push((*value).clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32 as u32);
        }
        data.push(u32::from(*biome));
    }
    let roots = program.climate_roots;
    let params = Params {
        geometry: [
            program.min_y,
            program.height,
            program.cell_width,
            program.cell_height,
        ],
        surface: [surface.0, surface.1, surface.2, program.flat_len],
        climate: [climate.0, climate.1, climate.2, program.surface_root],
        roots_a: [roots[0], roots[1], roots[2], roots[3]],
        roots_b: [roots[4], roots[5], rows, program.rows.len() as u32],
        tables: [octaves, normals, blended, 0],
    };
    (params, data)
}

struct Loaded {
    program: Arc<Program>,
    params: Params,
    pipeline: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    uniform: wgpu::Buffer,
    jobs: wgpu::Buffer,
    out: wgpu::Buffer,
    /// Bound through `bind`; owned here so its lifetime is explicit.
    _tables: wgpu::Buffer,
}

struct Pending {
    program: Arc<Program>,
    keys: Vec<TileKey>,
    readback: wgpu::Buffer,
    mapped: Option<mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

pub(super) struct FarGenerator {
    /// Compute shaders are available and not disabled.
    pub supported: bool,
    loaded: Option<Loaded>,
    pending: VecDeque<Pending>,
    inflight: HashSet<TileKey>,
    /// Delivered tiles the session has not yet dropped from its queue.
    delivered: HashSet<TileKey>,
    queue_revision: u64,
    done: Vec<(TileKey, Vec<Column>)>,
    pub tiles: u64,
    pub error: Option<String>,
}

impl FarGenerator {
    pub fn new(supported: bool) -> Self {
        Self {
            supported,
            loaded: None,
            pending: VecDeque::new(),
            inflight: HashSet::new(),
            delivered: HashSet::new(),
            queue_revision: 0,
            done: Vec::new(),
            tiles: 0,
            error: None,
        }
    }

    fn load(&mut self, device: &wgpu::Device, program: &Arc<Program>) -> Result<(), String> {
        if self
            .loaded
            .as_ref()
            .is_some_and(|loaded| Arc::ptr_eq(&loaded.program, program))
        {
            return Ok(());
        }
        self.loaded = None;
        let (params, data) = tables(program);
        let source = shader_source(program.registers());
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("far terrain generate"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("far terrain generate"),
            layout: None,
            module: &shader,
            entry_point: Some("generate"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(error) = pollster::block_on(device.pop_error_scope()) {
            return Err(format!("far terrain generator: {error}"));
        }
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far generate params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tables = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("far generate tables"),
            contents: bytemuck::cast_slice(&data),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let jobs = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far generate jobs"),
            size: (TILES_PER_DISPATCH * 16) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let out = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far generate columns"),
            size: (TILES_PER_DISPATCH * SAMPLES * 8) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("far generate"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: tables.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: jobs.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        self.loaded = Some(Loaded {
            program: Arc::clone(program),
            params,
            pipeline,
            bind,
            uniform,
            jobs,
            out,
            _tables: tables,
        });
        Ok(())
    }

    /// Record one dispatch for the next queued tiles not yet in flight.
    pub fn encode(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        work: &FarGeneration,
        timing: &Timing,
    ) {
        if work.revision != self.queue_revision {
            self.queue_revision = work.revision;
            self.delivered.clear();
        }
        let Some(program) = work.program.as_ref().filter(|_| self.supported) else {
            self.loaded = None;
            return;
        };
        if self.pending.len() >= IN_FLIGHT {
            return;
        }
        let keys: Vec<TileKey> = work
            .tiles
            .iter()
            .filter(|key| !self.inflight.contains(key) && !self.delivered.contains(key))
            .take(TILES_PER_DISPATCH)
            .copied()
            .collect();
        if keys.is_empty() {
            return;
        }
        if let Err(error) = self.load(device, program) {
            // A driver that rejects the shader falls back to the CPU worker.
            self.error = Some(error);
            self.supported = false;
            return;
        }
        let loaded = self.loaded.as_ref().expect("loaded above");
        let jobs: Vec<[i32; 4]> = keys
            .iter()
            .map(|key| [i32::from(key.level), key.x, key.z, 0])
            .collect();
        let mut params = loaded.params;
        params.tables[3] = keys.len() as u32;
        queue.write_buffer(&loaded.uniform, 0, bytemuck::bytes_of(&params));
        queue.write_buffer(&loaded.jobs, 0, bytemuck::cast_slice(&jobs));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some(timing::PASSES[timing::FAR_TERRAIN_GENERATE]),
                timestamp_writes: timing.compute_writes(timing::FAR_TERRAIN_GENERATE),
            });
            pass.set_pipeline(&loaded.pipeline);
            pass.set_bind_group(0, &loaded.bind, &[]);
            pass.dispatch_workgroups((keys.len() * SAMPLES / WORKGROUP) as u32, 1, 1);
        }
        let size = (keys.len() * SAMPLES * 8) as u64;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("far generate readback"),
            size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&loaded.out, 0, &readback, 0, size);
        self.inflight.extend(keys.iter().copied());
        self.pending.push_back(Pending {
            program: Arc::clone(program),
            keys,
            readback,
            mapped: None,
        });
    }

    /// After the frame's submission: map this frame's readback.
    pub fn submitted(&mut self) {
        for pending in &mut self.pending {
            if pending.mapped.is_none() {
                let (send, receive) = mpsc::sync_channel(1);
                pending
                    .readback
                    .slice(..)
                    .map_async(wgpu::MapMode::Read, move |result| {
                        let _ = send.send(result);
                    });
                pending.mapped = Some(receive);
            }
        }
    }

    /// Finished tiles, oldest first; results of a replaced program are dropped.
    pub fn drain(&mut self) -> Vec<(TileKey, Vec<Column>)> {
        while let Some(pending) = self.pending.front() {
            let Some(result) = pending.mapped.as_ref().and_then(|m| m.try_recv().ok()) else {
                break;
            };
            let pending = self.pending.pop_front().expect("front checked");
            for key in &pending.keys {
                self.inflight.remove(key);
            }
            if result.is_err() {
                continue;
            }
            let current = self
                .loaded
                .as_ref()
                .is_some_and(|loaded| Arc::ptr_eq(&loaded.program, &pending.program));
            if current {
                let bytes = pending.readback.slice(..).get_mapped_range();
                let words: &[u32] = bytemuck::cast_slice(&bytes);
                for (key, tile) in pending.keys.iter().zip(words.chunks_exact(SAMPLES * 2)) {
                    let columns = tile
                        .chunks_exact(2)
                        .map(|pair| Column {
                            height: pair[0] as i32,
                            biome: pair[1] as u16,
                        })
                        .collect();
                    self.done.push((*key, columns));
                    self.delivered.insert(*key);
                    self.tiles += 1;
                }
            }
            pending.readback.unmap();
        }
        std::mem::take(&mut self.done)
    }

    pub fn metrics(&self) -> serde_json::Value {
        serde_json::json!({
            "supported": self.supported,
            "loaded": self.loaded.is_some(),
            "tiles": self.tiles,
            "in_flight": self.pending.len(),
            "error": self.error,
        })
    }
}

#[cfg(test)]
#[path = "far_generate_tests.rs"]
mod tests;
