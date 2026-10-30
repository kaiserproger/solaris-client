use crate::{
    api::{ChunkDraw, DynamicGeometry},
    protocol::{
        content::Content,
        world::{Neighborhood, World},
        Result,
    },
};
use glam::Mat4;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Instant,
};
#[cfg(test)]
mod border_tests;
mod catalog;
mod mesher;
mod visibility;
use catalog::Catalog;
use mesher::{BuiltSection, Rules};

const WORKERS: usize = 2;
// A 65×65 view may contain up to 24 nonempty vertical sections per column.
pub(crate) const MAX_SECTIONS: usize = 131072;

pub(crate) fn section_limit(radius: u32) -> usize {
    let side = radius.clamp(16, 32) as usize * 2 + 1;
    (32768 * side * side / (33 * 33)).min(MAX_SECTIONS)
}
// CPU payload budget covers only geometry awaiting a submitted GPU upload.
const CPU_MESH_BUDGET: usize = 512 * 1024 * 1024;
const RESULT_BYTE_LIMIT: usize = 8 * 1024 * 1024;

type Wake = Arc<dyn Fn() + Send + Sync>;
struct Job {
    key: [i32; 3],
    revision: u64,
    epochs: [u64; 3],
    generation: u64,
    world: Neighborhood,
    catalog: Arc<Catalog>,
    rules: Rules,
    hidden: Arc<Vec<[i32; 3]>>,
}
struct Completion {
    worker: usize,
    key: [i32; 3],
    revision: u64,
    epochs: [u64; 3],
    generation: u64,
    elapsed_ms: f64,
    result: Result<Option<BuiltSection>>,
}
struct Worker {
    send: Option<mpsc::SyncSender<Job>>,
    thread: Option<thread::JoinHandle<()>>,
    busy: bool,
}
struct Section {
    revision: u64,
    pending: bool,
    running: Option<(u64, u64)>,
    queued: Instant,
    bytes: usize,
    unknown_light_samples: u64,
    unknown_biome_samples: u64,
}
#[derive(Default)]
struct Counters {
    dispatched: u64,
    completed: u64,
    stale: u64,
    cancelled: u64,
    coalesced: u64,
    removed: u64,
    queue_peak: usize,
    cpu_ms: f64,
    last_cpu_ms: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct MeshView {
    center: [i32; 2],
    radius: u32,
}
impl MeshView {
    fn contains(self, column: [i32; 2]) -> bool {
        (0..2).all(|axis| column[axis].abs_diff(self.center[axis]) <= self.radius)
    }
}

/// Session-owned scheduling; workers own only nine immutable chunk handles and
/// an immutable render catalog. No worker observes mutable packet-applied state.
pub struct Terrain {
    workers: Vec<Worker>,
    receive: mpsc::Receiver<Completion>,
    cancellation: Arc<AtomicU64>,
    generation: u64,
    epochs: [u64; 3],
    catalog: Option<Arc<Catalog>>,
    rules: Option<Rules>,
    sections: BTreeMap<[i32; 3], Section>,
    next_revision: u64,
    resident_bytes: usize,
    camera: Option<[f64; 3]>,
    view: Option<MeshView>,
    view_lifecycle: u64,
    view_min_y: i32,
    view_height: i32,
    // False only after dispatch selection proves no eligible job remains.
    has_pending: bool,
    counters: Counters,
    hidden: Arc<Vec<[i32; 3]>>,
}
impl Terrain {
    pub fn new(wake: Wake) -> Result<Self> {
        let (send, receive) = mpsc::sync_channel(WORKERS);
        let cancellation = Arc::new(AtomicU64::new(0));
        let mut workers = Vec::with_capacity(WORKERS);
        for worker in 0..WORKERS {
            let (job_send, jobs) = mpsc::sync_channel::<Job>(1);
            let send = send.clone();
            let wake = wake.clone();
            let cancellation = cancellation.clone();
            let handle = thread::Builder::new()
                .name(format!("terrain-{worker}"))
                .spawn(move || {
                    while let Ok(job) = jobs.recv() {
                        let start = Instant::now();
                        let result = mesher::build_hiding(
                            &job.world,
                            &job.catalog,
                            job.key,
                            job.rules,
                            &cancellation,
                            job.generation,
                            RESULT_BYTE_LIMIT,
                            &job.hidden,
                        );
                        let completion = Completion {
                            worker,
                            key: job.key,
                            revision: job.revision,
                            epochs: job.epochs,
                            generation: job.generation,
                            elapsed_ms: start.elapsed().as_secs_f64() * 1000.,
                            result,
                        };
                        if send.send(completion).is_err() {
                            break;
                        }
                        wake();
                    }
                })
                .map_err(|e| format!("terrain_worker_start_failed: {e}"))?;
            workers.push(Worker {
                send: Some(job_send),
                thread: Some(handle),
                busy: false,
            });
        }
        Ok(Self {
            workers,
            receive,
            cancellation,
            generation: 0,
            epochs: [0; 3],
            catalog: None,
            rules: None,
            sections: BTreeMap::new(),
            next_revision: 0,
            resident_bytes: 0,
            camera: None,
            view: None,
            view_lifecycle: 0,
            view_min_y: 0,
            view_height: 0,
            has_pending: false,
            counters: Counters::default(),
            hidden: Arc::default(),
        })
    }
    pub fn activate(&mut self, content: &Content, epochs: [u64; 3]) -> Result<()> {
        let catalog = Arc::new(Catalog::capture(content)?);
        self.cancel_generation();
        self.epochs = epochs;
        self.catalog = Some(catalog);
        self.sections.clear();
        self.resident_bytes = 0;
        self.camera = None;
        self.view = None;
        self.hidden = Arc::default();
        Ok(())
    }
    fn cancel_generation(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("terrain generation exhausted");
        self.cancellation.store(self.generation, Ordering::Release);
        self.has_pending = true;
    }
    pub fn reset(&mut self, draws: &mut BTreeMap<[i32; 3], ChunkDraw>) {
        self.cancel_generation();
        self.catalog = None;
        self.sections.clear();
        self.resident_bytes = 0;
        self.camera = None;
        self.view = None;
        self.hidden = Arc::default();
        draws.clear();
    }
    /// Draw `position` through a presentation animation instead of the section
    /// mesh. The block meshes as air until [`Terrain::unhide_block`].
    pub fn hide_block(&mut self, world: &World, position: [i32; 3]) -> Result<()> {
        if self.hidden.contains(&position) {
            return Ok(());
        }
        Arc::make_mut(&mut self.hidden).push(position);
        self.invalidate_block(world, position)
    }
    pub fn unhide_block(&mut self, world: &World, position: [i32; 3]) -> Result<()> {
        let Some(index) = self.hidden.iter().position(|p| *p == position) else {
            return Ok(());
        };
        Arc::make_mut(&mut self.hidden).swap_remove(index);
        self.invalidate_block(world, position)
    }
    /// True once the section holding `position` has no queued or running job,
    /// i.e. the mesh on screen already reflects the latest hide/unhide.
    pub fn block_settled(&self, position: [i32; 3]) -> bool {
        self.sections
            .get(&position.map(|v| v.div_euclid(16)))
            .is_none_or(|section| !section.pending && section.running.is_none())
    }
    /// Dimension cardinal shading and user visual settings are explicit inputs,
    /// not renderer guesses. A rule change invalidates all section results.
    pub fn set_render_rules(
        &mut self,
        directional_shade: [f32; 6],
        ambient_occlusion: bool,
        biome_blend_radius: u8,
        fade_duration_ms: Option<u64>,
    ) -> Result<()> {
        if directional_shade
            .iter()
            .any(|v| !v.is_finite() || !(0. ..=1.).contains(v))
            || biome_blend_radius > 7
            || fade_duration_ms.is_some_and(|v| v > 2000)
        {
            return Err("invalid_source_terrain_render_rules".into());
        }
        let next = Rules {
            directional_shade,
            ambient_occlusion,
            biome_blend_radius,
            fade_duration_ms,
        };
        if self.rules != Some(next) {
            self.cancel_generation();
            for section in self.sections.values_mut() {
                section.pending = true;
            }
            self.rules = Some(next);
        }
        Ok(())
    }
    fn invalidate_section(&mut self, world: &World, key: [i32; 3]) -> Result<()> {
        if self
            .view
            .is_some_and(|view| !view.contains([key[0], key[2]]))
        {
            return Ok(());
        }
        let Some(chunk) = world.chunk([key[0], key[2]]) else {
            return Ok(());
        };
        if key[1] < world.min_y.div_euclid(16)
            || key[1] >= world.min_y.div_euclid(16) + chunk.sections.len() as i32
        {
            return Ok(());
        }
        let index = (key[1] - world.min_y.div_euclid(16)) as usize;
        if !self.sections.contains_key(&key) && chunk.sections[index].non_empty_block_count == 0 {
            return Ok(());
        }
        if !self.sections.contains_key(&key)
            && self.sections.len() >= section_limit(self.view.map_or(16, |view| view.radius))
        {
            return Err("terrain_section_queue_bound_reached".into());
        }
        self.next_revision = self
            .next_revision
            .checked_add(1)
            .ok_or("terrain_revision_exhausted")?;
        match self.sections.entry(key) {
            std::collections::btree_map::Entry::Occupied(mut entry) => {
                let section = entry.get_mut();
                if section.pending {
                    self.counters.coalesced += 1;
                } else {
                    section.queued = Instant::now();
                }
                section.revision = self.next_revision;
                section.pending = true;
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Section {
                    revision: self.next_revision,
                    pending: true,
                    running: None,
                    queued: Instant::now(),
                    bytes: 0,
                    unknown_light_samples: 0,
                    unknown_biome_samples: 0,
                });
            }
        }
        self.has_pending = true;
        self.counters.queue_peak = self.counters.queue_peak.max(self.sections.len());
        Ok(())
    }
    pub fn invalidate_chunk(&mut self, world: &World, key: [i32; 2]) -> Result<()> {
        // Full column replacement/light/biome/unload changes AO diagonals and
        // default biome blending in all eight neighboring columns, too.
        for x in key[0] - 1..=key[0] + 1 {
            for z in key[1] - 1..=key[1] + 1 {
                let Some(chunk) = world.chunk([x, z]) else {
                    continue;
                };
                for y in 0..chunk.sections.len() {
                    self.invalidate_section(world, [x, world.min_y.div_euclid(16) + y as i32, z])?;
                }
            }
        }
        Ok(())
    }
    pub fn invalidate_block(&mut self, world: &World, position: [i32; 3]) -> Result<()> {
        let key = position.map(|v| v.div_euclid(16));
        // AO reaches two cells, fluid corners one, biome blending the selected
        // radius. Cartesian boundary dependencies include corner sections.
        let horizontal = i32::from(self.rules.map_or(2, |r| r.biome_blend_radius)).max(2);
        let span = |axis: usize| {
            let radius = if axis == 1 { 2 } else { horizontal };
            let local = position[axis].rem_euclid(16);
            (
                key[axis] - i32::from(local < radius),
                key[axis] + i32::from(local >= 16 - radius),
            )
        };
        let (x0, x1) = span(0);
        let (y0, y1) = span(1);
        let (z0, z1) = span(2);
        for x in x0..=x1 {
            for y in y0..=y1 {
                for z in z0..=z1 {
                    self.invalidate_section(world, [x, y, z])?;
                }
            }
        }
        Ok(())
    }
    fn sync_view(
        &mut self,
        world: &World,
        camera: [f64; 3],
        render_distance: u32,
        draws: &mut BTreeMap<[i32; 3], ChunkDraw>,
    ) -> Result<bool> {
        let next = MeshView {
            center: [camera[0], camera[2]].map(|v| (v / 16.).floor() as i32),
            radius: render_distance,
        };
        let previous = self.view;
        let lifecycle = world.chunk_lifecycle_revision();
        let lifecycle_changed = self.view_lifecycle != lifecycle
            || self.view_min_y != world.min_y
            || self.view_height != world.height;
        if previous == Some(next) && !lifecycle_changed {
            return Ok(false);
        }
        self.view = Some(next);
        self.view_lifecycle = lifecycle;
        self.view_min_y = world.min_y;
        self.view_height = world.height;
        let mut changed = false;
        // Retire before admitting completed jobs: a late result from the old
        // window must not refill residency after a move or distance reduction.
        self.sections.retain(|key, section| {
            if next.contains([key[0], key[2]])
                && world.chunk([key[0], key[2]]).is_some()
                && key[1] >= world.min_y.div_euclid(16)
                && key[1] < (world.min_y + world.height).div_euclid(16)
            {
                return true;
            }
            self.resident_bytes -= section.bytes;
            self.counters.removed += 1;
            changed |= draws.remove(key).is_some();
            false
        });
        draws.retain(|key, _| self.sections.contains_key(key));
        if previous != Some(next) || lifecycle_changed {
            for (&column, chunk) in world.chunks() {
                if !next.contains(column) {
                    continue;
                }
                let entering = previous.is_none_or(|old| !old.contains(column));
                for y in 0..chunk.sections.len() {
                    let key = [column[0], world.min_y.div_euclid(16) + y as i32, column[1]];
                    if entering || !self.sections.contains_key(&key) {
                        self.invalidate_section(world, key)?;
                    }
                }
            }
        }
        Ok(changed)
    }
    pub fn update(
        &mut self,
        world: &World,
        camera: [f64; 3],
        render_distance: u32,
        draws: &mut BTreeMap<[i32; 3], ChunkDraw>,
    ) -> Result<bool> {
        if camera.iter().any(|v| !v.is_finite()) {
            return Err("non_finite_terrain_camera".into());
        }
        let catalog = self
            .catalog
            .as_ref()
            .ok_or("terrain_resources_not_activated")?
            .clone();
        let rules = self.rules.ok_or("source_terrain_render_rules_not_set")?;
        if self.camera.is_some_and(|old| {
            (0..3).map(|i| (camera[i] - old[i]).powi(2)).sum::<f64>() > 128. * 128.
        }) {
            // Cancel old snapshots immediately; pending keys remain deduplicated
            // and receive current camera priority, not an old prefetch FIFO.
            self.cancel_generation();
            for section in self.sections.values_mut().filter(|s| s.pending) {
                section.queued = Instant::now();
            }
        }
        self.camera = Some(camera);
        let mut changed = self.sync_view(world, camera, render_distance, draws)?;
        while let Ok(completion) = self.receive.try_recv() {
            self.workers[completion.worker].busy = false;
            self.has_pending = true;
            if let Some(section) = self.sections.get_mut(&completion.key) {
                if section.running == Some((completion.generation, completion.revision)) {
                    section.running = None;
                }
            }
            self.counters.completed += 1;
            self.counters.cpu_ms += completion.elapsed_ms;
            self.counters.last_cpu_ms = completion.elapsed_ms;
            let valid = completion.generation == self.generation
                && completion.epochs == self.epochs
                && self
                    .sections
                    .get(&completion.key)
                    .is_some_and(|s| s.revision == completion.revision)
                && world
                    .chunk([completion.key[0], completion.key[2]])
                    .is_some();
            if !valid {
                self.counters.stale += 1;
                continue;
            }
            let Some(built) = completion.result? else {
                self.counters.cancelled += 1;
                continue;
            };
            let section = self.sections.get_mut(&completion.key).unwrap();
            let bytes = mesher::geometry_bytes(&built.geometry);
            let next_bytes = self.resident_bytes - section.bytes + bytes;
            if next_bytes > CPU_MESH_BUDGET {
                return Err("terrain_cpu_mesh_residency_byte_bound_reached".into());
            }
            section.bytes = bytes;
            section.unknown_light_samples = built.unknown_light_samples;
            section.unknown_biome_samples = built.unknown_biome_samples;
            section.pending = false;
            self.resident_bytes = next_bytes;
            draws.insert(
                completion.key,
                ChunkDraw {
                    revision: completion.revision,
                    origin: completion.key.map(|v| f64::from(v) * 16.),
                    bounds_min: built.bounds_min,
                    bounds_max: built.bounds_max,
                    visibility: built.visibility,
                    geometry: Some(Arc::new(built.geometry)),
                    fade_duration_ms: rules.fade_duration_ms,
                },
            );
            changed = true;
        }
        for worker in 0..self.workers.len() {
            if !self.has_pending {
                break;
            }
            if self.workers[worker].busy {
                continue;
            }
            // Reserve every fourth dispatch for first loads,
            // so continuous visible edits cannot indefinitely starve first loads.
            let prioritize_remesh = self.counters.dispatched % 4 != 3;
            let candidate = self
                .sections
                .iter()
                .filter(|(_, s)| s.pending)
                .filter(|(_, section)| section.running != Some((self.generation, section.revision)))
                .min_by(|(a, sa), (b, sb)| {
                    let score = |key: &[i32; 3], section: &Section| {
                        let distance = (0..3)
                            .map(|i| (f64::from(key[i]) * 16. + 8. - camera[i]).powi(2))
                            .sum::<f64>()
                            .sqrt()
                            / 16.;
                        distance - section.queued.elapsed().as_secs_f64() * 4.
                    };
                    // Give replacements prompt admission without discarding
                    // the first-load queue's fairness dispatches.
                    let resident_order = if prioritize_remesh {
                        draws.contains_key(*b).cmp(&draws.contains_key(*a))
                    } else {
                        draws.contains_key(*a).cmp(&draws.contains_key(*b))
                    };
                    resident_order
                        .then_with(|| score(a, sa).total_cmp(&score(b, sb)))
                        .then_with(|| a.cmp(b))
                })
                .map(|(&key, s)| (key, s.revision));
            let Some((key, revision)) = candidate else {
                self.has_pending = false;
                break;
            };
            let job = Job {
                key,
                revision,
                epochs: self.epochs,
                generation: self.generation,
                world: world.snapshot([key[0], key[2]]),
                catalog: catalog.clone(),
                rules,
                hidden: self.hidden.clone(),
            };
            self.workers[worker]
                .send
                .as_ref()
                .unwrap()
                .try_send(job)
                .map_err(|_| "terrain_worker_queue_invariant_failed".to_owned())?;
            self.workers[worker].busy = true;
            self.counters.dispatched += 1;
            self.sections.get_mut(&key).unwrap().running = Some((self.generation, revision));
        }
        Ok(changed)
    }
    /// Release only the exact committed upload payload; stale acknowledgements
    /// cannot free a replacement or change scheduling/visibility metadata.
    pub fn release_uploaded(
        &mut self,
        draws: &mut BTreeMap<[i32; 3], ChunkDraw>,
        uploaded: impl IntoIterator<Item = ([i32; 3], u64)>,
    ) {
        for (key, revision) in uploaded {
            let Some(draw) = draws.get_mut(&key) else {
                continue;
            };
            let Some(section) = self.sections.get_mut(&key) else {
                continue;
            };
            if draw.revision == revision
                && section.revision == revision
                && draw.geometry.take().is_some()
            {
                self.resident_bytes -= section.bytes;
                section.bytes = 0;
            }
        }
    }

    pub fn append_moving_block(
        &self,
        world: &World,
        state: u32,
        position: [i32; 3],
        transform: Mat4,
        out: &mut DynamicGeometry,
    ) -> Result<bool> {
        mesher::moving_block(
            &world.snapshot([position[0].div_euclid(16), position[2].div_euclid(16)]),
            self.catalog
                .as_deref()
                .ok_or("terrain_resources_not_activated")?,
            self.rules.ok_or("source_terrain_render_rules_not_set")?,
            state,
            position,
            transform,
            out,
        )
    }
    pub fn metrics(&self) -> Value {
        let pending = self.sections.values().filter(|s| s.pending).count();
        let running = self.workers.iter().filter(|w| w.busy).count();
        let unknown_light = self
            .sections
            .values()
            .map(|s| s.unknown_light_samples)
            .sum::<u64>();
        let unknown_biome = self
            .sections
            .values()
            .map(|s| s.unknown_biome_samples)
            .sum::<u64>();
        json!({"epochs":self.epochs,"generation":self.generation.to_string(),"worker_count":WORKERS,
            "pending_sections":pending,"running_jobs":running,"result_queue_bound":WORKERS,
            "section_descriptor_count":self.sections.len(),"section_descriptor_bound":MAX_SECTIONS,
            "queue_peak":self.counters.queue_peak,"dispatched_jobs":self.counters.dispatched,
            "completed_jobs":self.counters.completed,"stale_discarded_jobs":self.counters.stale,
            "cancelled_jobs":self.counters.cancelled,"deduplicated_invalidations":self.counters.coalesced,
            "unloaded_sections":self.counters.removed,"mesh_cpu_total_ms":self.counters.cpu_ms,
            "last_mesh_cpu_ms":self.counters.last_cpu_ms,"cpu_mesh_resident_bytes":self.resident_bytes,
            "cpu_mesh_budget_bytes":CPU_MESH_BUDGET,"single_result_byte_bound":RESULT_BYTE_LIMIT,
            "unknown_light_samples":unknown_light,"unknown_biome_samples":unknown_biome,
            "geometry_ready":self.catalog.is_some() && self.rules.is_some() && pending == 0 && running == 0,
            "source_sampling_complete":unknown_light == 0 && unknown_biome == 0,
            "catalog_ready":self.catalog.is_some(),"render_rules_ready":self.rules.is_some()})
    }
    /// Initial presentation waits on the camera's mesh neighborhood, not distant jobs.
    pub fn spawn_meshes_ready(&self, camera: [f64; 3]) -> bool {
        let center = camera.map(|coordinate| (coordinate / 16.).floor() as i32);
        self.catalog.is_some()
            && self.rules.is_some()
            && self.sections.iter().all(|(key, section)| {
                (0..3).any(|axis| (key[axis] - center[axis]).abs() > 1)
                    || (!section.pending && section.running.is_none())
            })
    }
    /// Explicit capture diagnostics only, not per-frame observation work.
    pub fn capture_metrics(&self) -> Value {
        let nearby_sections: Vec<Value> = self
            .sections
            .iter()
            .filter(|(key, _)| {
                self.camera.is_some_and(|camera| {
                    (0..3).all(|axis| (key[axis] - (camera[axis] / 16.).floor() as i32).abs() <= 1)
                })
            })
            .map(|(key, section)| {
                json!({"section":key,
            "requested_revision":section.revision.to_string(),"pending":section.pending,
            "running_revision":section.running.map(|(_, revision)| revision.to_string())})
            })
            .collect();
        let mut result = self.metrics();
        result["nearby_sections"] = json!(nearby_sections);
        result
    }
}
impl Drop for Terrain {
    fn drop(&mut self) {
        self.cancel_generation();
        for worker in &mut self.workers {
            worker.send.take();
        }
        // Drain bounded completions so senders cannot block while being joined.
        while self.workers.iter().any(|w| w.busy) {
            match self.receive.recv() {
                Ok(completion) => self.workers[completion.worker].busy = false,
                Err(_) => break,
            }
        }
        for worker in &mut self.workers {
            if let Some(handle) = worker.thread.take() {
                let _ = handle.join();
            }
        }
    }
}
