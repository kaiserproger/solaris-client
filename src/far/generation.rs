//! Never-generated far columns computed on this machine, only on servers
//! whose operator shares the generation program (`solaris:lod/program`,
//! `[lod] share_terrain_keys`). Such a client asks the server for stored
//! columns only and meanwhile generates every tile still missing columns
//! whole, in the renderer's compute pass (`TILES_PER_DISPATCH` per frame), or
//! on a CPU worker thread when the GPU path is unavailable (headless, no
//! compute, `SOLARIS_CLIENT_FAR_GPU=0`). Generated samples rank lowest: any
//! server or chunk sample replaces them.
use super::program::{Column, Program};
use super::{write, Far, Source, TileKey, EDGE, SAMPLES};
use crate::protocol::{content::Content, Result};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// Tiles handed out (GPU queue or CPU worker) and not yet answered: enough
/// for every in-flight compute dispatch to be full.
const MAX_PENDING: usize = 48;
/// A tile not answered by then is handed out again.
const PENDING_TIMEOUT: Duration = Duration::from_secs(10);
/// Decompressed program bound (the vanilla program is about 200 KiB).
const MAX_PROGRAM_BYTES: u64 = 8 << 20;

type CpuWorker = (
    mpsc::Sender<TileKey>,
    mpsc::Receiver<(TileKey, Vec<Column>, Duration)>,
);

pub(super) struct Generation {
    program: Arc<Program>,
    /// Land and underwater colour per program biome, resolved on first use.
    palette: Option<Vec<[u32; 2]>>,
    pending: HashMap<TileKey, Instant>,
    /// Tiles queued for the renderer (GPU path), oldest first.
    gpu_queue: Vec<TileKey>,
    gpu_revision: u64,
    cpu: Option<CpuWorker>,
    tiles_gpu: u64,
    tiles_cpu: u64,
    cpu_time: Duration,
}

impl Far {
    /// `solaris:lod/program`: the server's zlib-compressed generation program.
    pub(crate) fn program(&mut self, payload: &[u8]) -> Result<()> {
        let Some((&protocol, compressed)) = payload.split_first() else {
            return Err("empty far terrain program".into());
        };
        if protocol != super::PROTOCOL {
            return Err("unsupported far terrain protocol".into());
        }
        let mut bytes = Vec::new();
        flate2::read::ZlibDecoder::new(compressed)
            .take(MAX_PROGRAM_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| format!("far terrain program: {error}"))?;
        if bytes.len() as u64 > MAX_PROGRAM_BYTES {
            return Err("far terrain program too large".into());
        }
        let program = Arc::new(Program::decode(&bytes)?);
        self.generation = Some(Generation {
            program,
            palette: None,
            pending: HashMap::new(),
            gpu_queue: Vec::new(),
            gpu_revision: 0,
            cpu: None,
            tiles_gpu: 0,
            tiles_cpu: 0,
            cpu_time: Duration::ZERO,
        });
        // Tiles answered before now may still have router-less holes.
        for tile in self.tiles.values_mut() {
            tile.generated = false;
        }
        self.revision += 1;
        Ok(())
    }

    /// Whether the renderer generates tiles (else the CPU worker does).
    pub(crate) fn set_gpu(&mut self, gpu: bool) {
        self.gpu = gpu;
    }

    /// Hand out tiles that still miss columns, nearest first, without waiting
    /// for the server's stored-column answer: generated samples rank lowest,
    /// so stored columns replace them whenever that answer arrives.
    pub(super) fn schedule_generation(&mut self, now: Instant) {
        let gpu = self.gpu;
        let Some(generation) = self.generation.as_mut() else {
            return;
        };
        let expired: Vec<TileKey> = generation
            .pending
            .iter()
            .filter(|(_, since)| now.duration_since(**since) >= PENDING_TIMEOUT)
            .map(|(key, _)| *key)
            .collect();
        for key in expired {
            generation.pending.remove(&key);
            generation.gpu_queue.retain(|queued| *queued != key);
            generation.gpu_revision += 1;
            if let Some(tile) = self.tiles.get_mut(&key) {
                tile.generated = false;
            }
            self.revision += 1;
        }
        for key in &self.desired {
            if generation.pending.len() >= MAX_PENDING {
                break;
            }
            let Some(tile) = self.tiles.get_mut(key) else {
                continue;
            };
            if tile.generated {
                continue;
            }
            tile.generated = true;
            if !tile.missing() {
                continue;
            }
            generation.pending.insert(*key, now);
            if gpu {
                generation.gpu_queue.push(*key);
                generation.gpu_revision += 1;
                self.revision += 1;
            } else {
                let program = Arc::clone(&generation.program);
                let (send, _) = generation.cpu.get_or_insert_with(|| spawn_cpu(program));
                let _ = send.send(*key);
            }
        }
    }

    /// Store finished CPU worker tiles.
    pub(crate) fn poll_generated(&mut self, content: &Content) -> Result<()> {
        let Some((_, receive)) = self.generation.as_ref().and_then(|g| g.cpu.as_ref()) else {
            return Ok(());
        };
        let finished: Vec<_> = receive.try_iter().collect();
        for (key, columns, elapsed) in finished {
            if let Some(generation) = self.generation.as_mut() {
                generation.tiles_cpu += 1;
                generation.cpu_time += elapsed;
            }
            self.store_generated(key, &columns, content)?;
        }
        Ok(())
    }

    /// Store tiles the renderer generated.
    pub(crate) fn generated(
        &mut self,
        tiles: Vec<(TileKey, Vec<Column>)>,
        content: &Content,
    ) -> Result<()> {
        for (key, columns) in tiles {
            if let Some(generation) = self.generation.as_mut() {
                generation.tiles_gpu += 1;
                generation.gpu_queue.retain(|queued| *queued != key);
                generation.gpu_revision += 1;
                self.revision += 1;
            }
            self.store_generated(key, &columns, content)?;
        }
        Ok(())
    }

    fn store_generated(
        &mut self,
        key: TileKey,
        columns: &[Column],
        content: &Content,
    ) -> Result<()> {
        let Some(generation) = self.generation.as_mut() else {
            return Ok(());
        };
        generation.pending.remove(&key);
        if columns.len() != SAMPLES {
            return Err("far terrain generation returned a partial tile".into());
        }
        if generation.palette.is_none() {
            if self.colours.is_none() {
                self.colours = Some(super::Colours::new(content)?);
            }
            let colours = self.colours.as_mut().expect("resolved above");
            let registry: HashMap<&str, u32> = colours
                .biomes
                .iter()
                .enumerate()
                .map(|(id, (_, name))| (name.as_str(), id as u32))
                .collect();
            let ids: Vec<Option<u32>> = generation
                .program
                .biomes
                .iter()
                .map(|name| registry.get(name.as_str()).copied())
                .collect();
            let palette = ids
                .into_iter()
                .map(|biome| {
                    [false, true].map(|underwater| {
                        colours
                            .surface(biome, underwater)
                            .map_or(0, |state| colours.rgba(content, state, biome))
                    })
                })
                .collect();
            generation.palette = Some(palette);
        }
        let palette = generation.palette.as_ref().expect("resolved above");
        let sea_level = generation.program.sea_level;
        let Some(tile) = self.tiles.get_mut(&key) else {
            return Ok(());
        };
        let mut changed = false;
        for (index, column) in columns.iter().enumerate() {
            let underwater = column.height < sea_level - 1;
            let height = if underwater {
                sea_level - 1
            } else {
                column.height
            };
            let rgba = palette
                .get(usize::from(column.biome))
                .map_or(0, |colours| colours[usize::from(underwater)]);
            if rgba != 0 {
                changed |= write(tile, index, height, rgba, Source::Generated);
            }
        }
        if changed {
            self.revision += 1;
        }
        Ok(())
    }

    /// The tiles were dropped (dimension change): forget handed-out work.
    pub(super) fn reset_generation(&mut self) {
        if let Some(generation) = &mut self.generation {
            generation.pending.clear();
            generation.gpu_queue.clear();
            generation.gpu_revision += 1;
        }
    }

    /// Content changed: biome colours are resolved again on demand.
    pub(super) fn forget_palette(&mut self) {
        if let Some(generation) = &mut self.generation {
            generation.palette = None;
        }
    }

    /// The GPU path's program and queue for the scene.
    pub(super) fn generation_scene(&self) -> (Option<Arc<Program>>, Vec<TileKey>, u64) {
        match &self.generation {
            Some(generation) if self.gpu => (
                Some(Arc::clone(&generation.program)),
                generation.gpu_queue.clone(),
                generation.gpu_revision,
            ),
            _ => (None, Vec::new(), 0),
        }
    }

    pub(super) fn generation_metrics(&self) -> serde_json::Value {
        let Some(generation) = &self.generation else {
            return serde_json::json!({"program": false});
        };
        serde_json::json!({
            "program": true,
            "mode": if self.gpu { "gpu" } else { "cpu" },
            "registers": generation.program.registers(),
            "pending": generation.pending.len(),
            "tiles_gpu": generation.tiles_gpu,
            "tiles_cpu": generation.tiles_cpu,
            "cpu_ms_per_tile": (generation.tiles_cpu > 0).then(|| {
                generation.cpu_time.as_secs_f64() * 1000. / generation.tiles_cpu as f64
            }),
        })
    }
}

/// The world columns of a tile's samples, x fastest.
pub(crate) fn tile_columns(key: TileKey) -> impl Iterator<Item = [i32; 2]> {
    let origin = key.origin();
    let spacing = key.spacing();
    (0..SAMPLES).map(move |index| {
        [
            (origin[0] + (index % EDGE) as i64 * spacing) as i32,
            (origin[1] + (index / EDGE) as i64 * spacing) as i32,
        ]
    })
}

fn spawn_cpu(program: Arc<Program>) -> CpuWorker {
    let (send_key, receive_key) = mpsc::channel::<TileKey>();
    let (send_tile, receive_tile) = mpsc::channel();
    let spawned = std::thread::Builder::new()
        .name("far-terrain-generate".into())
        .spawn(move || {
            let mut regs = Vec::new();
            while let Ok(key) = receive_key.recv() {
                let started = Instant::now();
                let columns: Vec<Column> = tile_columns(key)
                    .map(|[x, z]| program.column(x, z, &mut regs))
                    .collect();
                if send_tile.send((key, columns, started.elapsed())).is_err() {
                    return;
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("far terrain generation worker unavailable: {error}");
    }
    (send_key, receive_tile)
}
