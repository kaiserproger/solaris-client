use crate::{
    api::{ChunkDraw, DynamicGeometry},
    protocol::{
        content::Content,
        world::{Chunk, Neighborhood, World},
        Result,
    },
};
use glam::Mat4;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Condvar, Mutex, PoisonError, Weak,
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

/// Jobs each worker may hold beyond the one it is building. Dispatch happens
/// only when the session updates the scene, so a worker must not run dry
/// between two updates: eight queued sections (~10-30 ms of meshing) cover a
/// frame, while snapshots taken at dispatch stay fresh.
const QUEUED_PER_WORKER: usize = 8;

/// Half the hardware threads: the rest serve the session, the renderer, the
/// network and audio threads and, in singleplayer, the local server.
fn worker_count() -> usize {
    thread::available_parallelism().map_or(2, |n| n.get() / 2).clamp(2, 8)
}
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
    key: [i32; 3],
    revision: u64,
    epochs: [u64; 3],
    generation: u64,
    elapsed_ms: f64,
    result: Result<Option<BuiltSection>>,
}
/// One scored dispatch candidate; collected once per update, not per job.
struct Candidate {
    score: f64,
    key: [i32; 3],
    revision: u64,
}
/// Jobs waiting for any worker. Prompt jobs (replacements of drawn sections,
/// edit-created sections, the camera's own sections) enter at the front, so a
/// long first-load backlog never delays them by more than the jobs already
/// being built.
struct JobQueue<T = Job> {
    state: Mutex<(VecDeque<T>, bool)>,
    ready: Condvar,
}
impl<T> Default for JobQueue<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new((VecDeque::new(), false)),
            ready: Condvar::new(),
        }
    }
}
impl<T> JobQueue<T> {
    fn push(&self, front: Vec<T>, back: Vec<T>) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        // `front` is best-first; pushing it reversed keeps that order.
        for job in front.into_iter().rev() {
            state.0.push_front(job);
        }
        state.0.extend(back);
        drop(state);
        self.ready.notify_all();
    }
    /// The next job, or `None` once closed and drained.
    fn pop(&self) -> Option<T> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(job) = state.0.pop_front() {
                return Some(job);
            }
            if state.1 {
                return None;
            }
            state = self.ready.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
    }
    fn close(&self) {
        self.state.lock().unwrap_or_else(PoisonError::into_inner).1 = true;
        self.ready.notify_all();
    }
}
struct Section {
    revision: u64,
    running: Option<(u64, u64)>,
    queued: Instant,
    bytes: usize,
    unknown_light_samples: u64,
    unknown_biome_samples: u64,
    // A block change into an all-air section created this entry: nothing
    // else in it awaits a first load, so it is admitted like a replacement.
    created_by_edit: bool,
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
    queue: Arc<JobQueue>,
    threads: Vec<thread::JoinHandle<()>>,
    receive: mpsc::Receiver<Completion>,
    /// Dispatched jobs whose completion has not been received yet.
    in_flight: usize,
    max_in_flight: usize,
    /// Reused per-update candidate lists: [prompt, first loads].
    candidates: [Vec<Candidate>; 2],
    cancellation: Arc<AtomicU64>,
    generation: u64,
    epochs: [u64; 3],
    catalog: Option<Arc<Catalog>>,
    rules: Option<Rules>,
    sections: BTreeMap<[i32; 3], Section>,
    /// Sections invalidated since their last committed build. Dispatch walks
    /// only these, not every section (a 32-chunk view holds ~45k).
    pending: BTreeSet<[i32; 3]>,
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
    /// Columns whose received chunk or drawn-section key set changed since the
    /// session last refreshed far-terrain coverage (see `take_coverage_changes`).
    coverage_dirty: BTreeSet<[i32; 2]>,
    /// Draws were cleared wholesale: every column must be re-evaluated.
    coverage_reset: bool,
    /// Received columns inside the mesh view as last reconciled. The weak
    /// handle identifies a replaced chunk and keeps its address from reuse.
    columns: BTreeMap<[i32; 2], Weak<Chunk>>,
}
impl Terrain {
    pub fn new(wake: Wake) -> Result<Self> {
        let workers = worker_count();
        let max_in_flight = workers * (1 + QUEUED_PER_WORKER);
        let (send, receive) = mpsc::sync_channel(max_in_flight);
        let cancellation = Arc::new(AtomicU64::new(0));
        let queue = Arc::new(JobQueue::<Job>::default());
        let mut threads = Vec::with_capacity(workers);
        for worker in 0..workers {
            let send = send.clone();
            let wake = wake.clone();
            let cancellation = cancellation.clone();
            let jobs = queue.clone();
            let spawned = thread::Builder::new()
                .name(format!("terrain-{worker}"))
                .spawn(move || {
                    while let Some(job) = jobs.pop() {
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
                });
            match spawned {
                Ok(handle) => threads.push(handle),
                Err(error) => {
                    queue.close();
                    return Err(format!("terrain_worker_start_failed: {error}"));
                }
            }
        }
        Ok(Self {
            queue,
            threads,
            receive,
            in_flight: 0,
            max_in_flight,
            candidates: [Vec::new(), Vec::new()],
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
            pending: BTreeSet::new(),
            counters: Counters::default(),
            hidden: Arc::default(),
            coverage_dirty: BTreeSet::new(),
            coverage_reset: true,
            columns: BTreeMap::new(),
        })
    }
    pub fn activate(&mut self, content: &Content, epochs: [u64; 3]) -> Result<()> {
        let catalog = Arc::new(Catalog::capture(content)?);
        self.cancel_generation();
        self.epochs = epochs;
        self.catalog = Some(catalog);
        self.sections.clear();
        self.pending.clear();
        self.resident_bytes = 0;
        self.camera = None;
        self.view = None;
        self.coverage_reset = true;
        self.columns.clear();
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
        self.pending.clear();
        self.resident_bytes = 0;
        self.camera = None;
        self.view = None;
        self.hidden = Arc::default();
        self.coverage_reset = true;
        self.columns.clear();
        draws.clear();
    }
    /// Columns whose far-terrain coverage may have changed since the last
    /// call: a column was received, replaced or forgotten, or one of its
    /// sections gained or lost its draw. `None` means re-evaluate everything.
    pub fn take_coverage_changes(&mut self) -> Option<BTreeSet<[i32; 2]>> {
        if std::mem::take(&mut self.coverage_reset) {
            self.coverage_dirty.clear();
            return None;
        }
        Some(std::mem::take(&mut self.coverage_dirty))
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
        let key = position.map(|v| v.div_euclid(16));
        self.sections
            .get(&key)
            .is_none_or(|section| !self.pending.contains(&key) && section.running.is_none())
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
            self.pending.extend(self.sections.keys().copied());
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
                if self.pending.insert(key) {
                    section.queued = Instant::now();
                } else {
                    self.counters.coalesced += 1;
                }
                section.revision = self.next_revision;
            }
            std::collections::btree_map::Entry::Vacant(entry) => {
                entry.insert(Section {
                    revision: self.next_revision,
                    running: None,
                    queued: Instant::now(),
                    bytes: 0,
                    unknown_light_samples: 0,
                    unknown_biome_samples: 0,
                    created_by_edit: false,
                });
                self.pending.insert(key);
            }
        }
        self.has_pending = true;
        self.counters.queue_peak = self.counters.queue_peak.max(self.sections.len());
        Ok(())
    }
    pub fn invalidate_chunk(&mut self, world: &World, key: [i32; 2]) -> Result<()> {
        self.coverage_dirty.insert(key);
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
        let created = !self.sections.contains_key(&key);
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
        if created && let Some(section) = self.sections.get_mut(&key) {
            section.created_by_edit = true;
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
        let shape_changed = self.view_min_y != world.min_y || self.view_height != world.height;
        let lifecycle_changed = self.view_lifecycle != lifecycle || shape_changed;
        if previous == Some(next) && !lifecycle_changed {
            return Ok(false);
        }
        self.view = Some(next);
        self.view_lifecycle = lifecycle;
        self.view_min_y = world.min_y;
        self.view_height = world.height;
        if previous == Some(next) && !shape_changed {
            // Same window and dimension shape: only received columns came,
            // went or were replaced. Reconcile those columns instead of every
            // section (a 32-chunk view holds ~45k) on each chunk arrival.
            return self.sync_columns(world, next, draws);
        }
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
            self.pending.remove(key);
            if draws.remove(key).is_some() {
                changed = true;
                self.coverage_dirty.insert([key[0], key[2]]);
            }
            false
        });
        draws.retain(|key, _| {
            let keep = self.sections.contains_key(key);
            if !keep {
                self.coverage_dirty.insert([key[0], key[2]]);
            }
            keep
        });
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
        self.columns.clear();
        self.columns.extend(
            world
                .chunks()
                .filter(|(column, _)| next.contains(**column))
                .map(|(column, chunk)| (*column, Arc::downgrade(chunk))),
        );
        Ok(changed)
    }
    /// Retire the sections of columns that left the world and admit missing
    /// sections of columns that arrived or were replaced, found by one merged
    /// walk of the loaded and the recorded columns.
    fn sync_columns(
        &mut self,
        world: &World,
        view: MeshView,
        draws: &mut BTreeMap<[i32; 3], ChunkDraw>,
    ) -> Result<bool> {
        let mut gone = Vec::new();
        let mut arrived = Vec::new();
        let mut recorded = self.columns.iter().peekable();
        for (&column, chunk) in world.chunks() {
            if !view.contains(column) {
                continue;
            }
            while let Some((&key, _)) = recorded.next_if(|(key, _)| **key < column) {
                gone.push(key);
            }
            let same = recorded
                .next_if(|(key, _)| **key == column)
                .is_some_and(|(_, handle)| std::ptr::eq(handle.as_ptr(), Arc::as_ptr(chunk)));
            if !same {
                arrived.push((column, Arc::downgrade(chunk), chunk.sections.len()));
            }
        }
        gone.extend(recorded.map(|(key, _)| *key));
        let bottom = world.min_y.div_euclid(16);
        let mut changed = false;
        for column in gone {
            self.columns.remove(&column);
            for y in bottom..(world.min_y + world.height).div_euclid(16) {
                let key = [column[0], y, column[1]];
                if let Some(section) = self.sections.remove(&key) {
                    self.resident_bytes -= section.bytes;
                    self.counters.removed += 1;
                    self.pending.remove(&key);
                }
                if draws.remove(&key).is_some() {
                    changed = true;
                    self.coverage_dirty.insert(column);
                }
            }
        }
        for (column, handle, sections) in arrived {
            for y in 0..sections {
                let key = [column[0], bottom + y as i32, column[1]];
                if !self.sections.contains_key(&key) {
                    self.invalidate_section(world, key)?;
                }
            }
            self.columns.insert(column, handle);
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
            let now = Instant::now();
            for key in &self.pending {
                if let Some(section) = self.sections.get_mut(key) {
                    section.queued = now;
                }
            }
        }
        self.camera = Some(camera);
        let mut changed = self.sync_view(world, camera, render_distance, draws)?;
        while let Ok(completion) = self.receive.try_recv() {
            self.in_flight -= 1;
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
            self.pending.remove(&completion.key);
            section.created_by_edit = false;
            self.resident_bytes = next_bytes;
            let replaced = draws.insert(
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
            if replaced.is_none() {
                self.coverage_dirty.insert([completion.key[0], completion.key[2]]);
            }
            changed = true;
        }
        let free = self.max_in_flight - self.in_flight;
        if free > 0 && self.has_pending {
            self.dispatch(world, camera, &catalog, rules, draws, free);
        }
        Ok(changed)
    }
    /// Score every eligible section once and queue the best `free` jobs.
    /// Prompt jobs go first (and to the front of the worker queue):
    /// replacements of drawn sections, sections a block edit created and the
    /// camera's own 3x3x3 sections (as in `spawn_meshes_ready`). Every fourth
    /// job is reserved for first loads, so continuous visible edits cannot
    /// starve them. Lower score wins: chunk distance minus four per second of
    /// waiting.
    fn dispatch(
        &mut self,
        world: &World,
        camera: [f64; 3],
        catalog: &Arc<Catalog>,
        rules: Rules,
        draws: &BTreeMap<[i32; 3], ChunkDraw>,
        free: usize,
    ) {
        let now = Instant::now();
        let center = camera.map(|coordinate| (coordinate / 16.).floor() as i32);
        let near = |key: &[i32; 3]| (0..3).all(|axis| (key[axis] - center[axis]).abs() <= 1);
        let mut candidates = std::mem::take(&mut self.candidates);
        let [prompt, first_loads] = &mut candidates;
        prompt.clear();
        first_loads.clear();
        // Both sets are ordered by key, so one merged walk answers "drawn?".
        let mut drawn = draws.keys().peekable();
        for key in &self.pending {
            let Some(section) = self.sections.get(key) else {
                continue;
            };
            if section.running == Some((self.generation, section.revision)) {
                continue;
            }
            while drawn.next_if(|drawn| *drawn < key).is_some() {}
            let distance = (0..3)
                .map(|i| (f64::from(key[i]) * 16. + 8. - camera[i]).powi(2))
                .sum::<f64>()
                .sqrt()
                / 16.;
            let candidate = Candidate {
                score: distance - now.saturating_duration_since(section.queued).as_secs_f64() * 4.,
                key: *key,
                revision: section.revision,
            };
            if section.created_by_edit || near(key) || drawn.peek() == Some(&key) {
                prompt.push(candidate);
            } else {
                first_loads.push(candidate);
            }
        }
        let eligible = prompt.len() + first_loads.len();
        let order = |a: &Candidate, b: &Candidate| {
            a.score.total_cmp(&b.score).then_with(|| a.key.cmp(&b.key))
        };
        for list in [&mut *prompt, &mut *first_loads] {
            if list.len() > free {
                list.select_nth_unstable_by(free - 1, order);
                list.truncate(free);
            }
            list.sort_unstable_by(order);
        }
        let (mut front, mut back) = (Vec::new(), Vec::new());
        let (mut next_prompt, mut next_first_load) = (0, 0);
        while front.len() + back.len() < free {
            let first_load_turn = self.counters.dispatched % 4 == 3;
            let is_prompt = next_prompt < prompt.len()
                && !(first_load_turn && next_first_load < first_loads.len());
            let candidate = if is_prompt {
                next_prompt += 1;
                &prompt[next_prompt - 1]
            } else if next_first_load < first_loads.len() {
                next_first_load += 1;
                &first_loads[next_first_load - 1]
            } else {
                break;
            };
            let job = Job {
                key: candidate.key,
                revision: candidate.revision,
                epochs: self.epochs,
                generation: self.generation,
                world: world.snapshot([candidate.key[0], candidate.key[2]]),
                catalog: catalog.clone(),
                rules,
                hidden: self.hidden.clone(),
            };
            if let Some(section) = self.sections.get_mut(&candidate.key) {
                section.running = Some((self.generation, candidate.revision));
            }
            self.counters.dispatched += 1;
            if is_prompt {
                front.push(job);
            } else {
                back.push(job);
            }
        }
        let queued = front.len() + back.len();
        // Only a selection that took every eligible section proves none remains.
        self.has_pending = queued < eligible;
        self.in_flight += queued;
        self.queue.push(front, back);
        self.candidates = candidates;
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
    pub(crate) fn mesh_cpu_total_ms(&self) -> f64 {
        self.counters.cpu_ms
    }
    pub fn metrics(&self) -> Value {
        let pending = self.pending.len();
        // Dispatched and not yet integrated: building or waiting in the queue.
        let running = self.in_flight;
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
        json!({"epochs":self.epochs,"generation":self.generation.to_string(),"worker_count":self.threads.len(),
            "pending_sections":pending,"running_jobs":running,"result_queue_bound":self.max_in_flight,
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
    /// Called every frame: look up the 27 neighbouring keys, never scan the queue.
    pub fn spawn_meshes_ready(&self, camera: [f64; 3]) -> bool {
        let center = camera.map(|coordinate| (coordinate / 16.).floor() as i32);
        self.catalog.is_some()
            && self.rules.is_some()
            && (0..27).all(|i| {
                let key = [center[0] + i % 3 - 1, center[1] + i / 3 % 3 - 1, center[2] + i / 9 - 1];
                self.sections
                    .get(&key)
                    .is_none_or(|section| !self.pending.contains(&key) && section.running.is_none())
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
            "requested_revision":section.revision.to_string(),"pending":self.pending.contains(key),
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
        // Queued jobs see the cancelled generation and return at once.
        self.queue.close();
        for handle in self.threads.drain(..) {
            let _ = handle.join();
        }
    }
}
