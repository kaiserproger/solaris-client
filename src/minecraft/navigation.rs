//! Generic bounded navigation owned by the ordinary Session input/task path.
mod planner;
use super::Outcome;
use crate::{
    api::InputIntent,
    player::{collision, Bounds},
    protocol::Result,
    session::Session,
};
use planner::{Planner, Space};
use serde_json::{json, Value};
use std::time::{Duration, Instant};

pub(super) struct Task {
    target: [i32; 3],
    epochs: (u64, u64),
    deadline: Instant,
    planner: Planner,
    colliders: Vec<Bounds>,
    waypoint: usize,
    last_tick: Option<u64>,
    progress_tick: u64,
    best_distance: f64,
    replans: u8,
    held: [bool; 2],
    released: bool,
}

struct ReceivedSpace<'a> {
    session: &'a Session,
    colliders: &'a mut Vec<Bounds>,
}
impl ReceivedSpace<'_> {
    /// True when the target cell holds colliding geometry. Unloaded reads
    /// as non-solid; arrival still needs a nearby collision-free pose.
    fn target_solid(&mut self, target: [i32; 3]) -> Result<bool> {
        let area = Bounds {
            min: [
                f64::from(target[0]),
                f64::from(target[1]),
                f64::from(target[2]),
            ],
            max: [
                f64::from(target[0]) + 1.,
                f64::from(target[1]) + 1.,
                f64::from(target[2]) + 1.,
            ],
        };
        if !self.gather(area)? {
            return Ok(false);
        }
        Ok(self.colliders.iter().any(|b| b.intersects(area)))
    }
    fn gather(&mut self, area: Bounds) -> Result<bool> {
        match collision::gather(
            &self.session.world,
            self.session
                .content
                .as_ref()
                .ok_or("client resources unavailable")?,
            &self.session.entities,
            self.session.client_tick,
            area,
            self.colliders,
        ) {
            Ok(()) => Ok(true),
            Err(error) if error == "unloaded_collision_neighborhood" => Ok(false),
            Err(error) => Err(error),
        }
    }
    fn safe(&self, area: Bounds) -> Result<bool> {
        let world = &self.session.world;
        let content = self
            .session
            .content
            .as_ref()
            .ok_or("client resources unavailable")?;
        let lo = area.min.map(|v| v.floor() as i32);
        let hi = area.max.map(|v| v.floor() as i32);
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                for x in lo[0]..=hi[0] {
                    let Some(state) = world.block([x, y, z]) else {
                        return Ok(false);
                    };
                    // Walking only: fluid/climbing/context-dependent collision paths must
                    // not be treated as ordinary air or as stable support.
                    if content.info(state)?.fluid.is_some()
                        || matches!(
                            content.name(state),
                            Some(
                                "minecraft:powder_snow"
                                    | "minecraft:scaffolding"
                                    | "minecraft:fire"
                                    | "minecraft:soul_fire"
                                    | "minecraft:cactus"
                                    | "minecraft:sweet_berry_bush"
                                    | "minecraft:lava"
                            )
                        )
                    {
                        return Ok(false);
                    }
                }
            }
        }
        Ok(true)
    }
}
impl Space for ReceivedSpace<'_> {
    fn clear(&mut self, min: [f64; 3], max: [f64; 3]) -> Result<bool> {
        let area = Bounds { min, max };
        let world = &self.session.world;
        if min[1] < f64::from(world.min_y)
            || max[1] > f64::from(world.min_y) + f64::from(world.height)
            || !world.border_contains_point(min)
            || !world.border_contains_point(max)
        {
            return Ok(false);
        }
        if !self.gather(area)?
            || self.colliders.iter().any(|b| b.intersects(area))
            || !self.safe(area)?
        {
            return Ok(false);
        }
        let content = self
            .session
            .content
            .as_ref()
            .ok_or("client resources unavailable")?;
        self.session.entities.collision_bounds(
            area,
            self.session.player.entity_id,
            None,
            self.session.client_tick,
            self.session.player.position,
            &self.session.world,
            content,
            self.colliders,
        )?;
        Ok(!self.colliders.iter().any(|b| b.intersects(area))
            && !self
                .session
                .world
                .border_collides(self.session.player.position, area)?)
    }
    fn supported(&mut self, position: [f64; 3], half: f64) -> Result<bool> {
        let area = Bounds {
            min: [
                position[0] - half + 0.001,
                position[1] - 0.002,
                position[2] - half + 0.001,
            ],
            max: [
                position[0] + half - 0.001,
                position[1] + 0.0001,
                position[2] + half - 0.001,
            ],
        };
        if !self.gather(area)? || !self.safe(area)? {
            return Ok(false);
        }
        Ok(self
            .colliders
            .iter()
            .any(|b| b.intersects(area) && (b.max[1] - position[1]).abs() <= 0.002))
    }
    fn surfaces(&mut self, x: i32, z: i32, low: f64, high: f64, out: &mut Vec<f64>) -> Result<()> {
        let area = Bounds {
            min: [f64::from(x) + 0.2, low - 0.01, f64::from(z) + 0.2],
            max: [f64::from(x) + 0.8, high + 0.01, f64::from(z) + 0.8],
        };
        if !self.gather(area)? {
            return Ok(());
        }
        for b in self.colliders.iter() {
            if b.max[1] >= low
                && b.max[1] <= high
                && b.min[0] < f64::from(x) + 0.8
                && b.max[0] > f64::from(x) + 0.2
                && b.min[2] < f64::from(z) + 0.8
                && b.max[2] > f64::from(z) + 0.2
            {
                if out.len() >= 256 {
                    return Err("navigation search bound".into());
                }
                out.push(b.max[1]);
            }
        }
        Ok(())
    }
}
fn require_play(session: &Session) -> Result<()> {
    if session.phase != "play" || !session.player.pose_known || session.content.is_none() {
        return Err("client is not in play".into());
    }
    if session.player.health.is_some_and(|health| health <= 0.) {
        return Err("navigation interrupted because the player died".into());
    }
    if session.player.vehicle.is_some() {
        return Err("navigation requires an unmounted player".into());
    }
    Ok(())
}
pub(super) fn start(
    session: &mut Session,
    name: &str,
    arguments: &Value,
) -> Result<Option<Outcome<Task>>> {
    if name != "minecraft_navigate_to_block" {
        return Ok(None);
    }
    // This operation takes ownership of movement, including any previously held
    // strafe/sneak/sprint keys. Validation failures must not leave them held.
    session.input(InputIntent::ReleaseAll)?;
    require_play(session)?;
    let mut target = [0; 3];
    for (index, key) in ["x", "y", "z"].into_iter().enumerate() {
        target[index] = arguments[key]
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| format!("{key} must be a signed 32-bit integer"))?;
    }
    let seconds = match arguments.get("timeout_seconds") {
        None => 8.,
        Some(value) => value
            .as_f64()
            .filter(|v| v.is_finite() && (0.1..=120.).contains(v))
            .ok_or("timeout_seconds outside bounds")?,
    };
    if target[1] < session.world.min_y
        || i64::from(target[1]) >= i64::from(session.world.min_y) + i64::from(session.world.height)
    {
        return Err(format!(
            "target y is outside the client world height: {}",
            target[1]
        ));
    }
    if !target_loaded(session, target) {
        return Err(target_error("target chunk is not client-loaded", target));
    }
    if !session.player.position.iter().all(|v| v.is_finite()) {
        return Err("client produced an invalid navigation observation".into());
    }
    // Long trips plan segmented legs toward the target and replan closer with
    // ordinary movement only (no teleport); the single-search 48-block radius
    // still bounds each plan call.
    if session.screen() != "game" {
        super::inventory::start(session, "minecraft_close_screen", &json!({}))?;
    }
    let scale = session.player.attribute("minecraft:scale");
    if !scale.is_finite() || scale <= 0. {
        return Err("client produced an invalid navigation observation".into());
    }
    let mut task = Task {
        target,
        epochs: session.context_epochs(),
        deadline: Instant::now() + Duration::from_secs_f64(seconds),
        planner: Planner::new(f64::from(0.6f32) * scale / 2., f64::from(1.8f32) * scale),
        colliders: Vec::with_capacity(64),
        waypoint: 0,
        last_tick: None,
        progress_tick: session.client_tick,
        best_distance: f64::INFINITY,
        replans: 0,
        held: [false; 2],
        released: false,
    };
    match task.on_event(session) {
        Ok(Some(value)) => Ok(Some(Outcome::Complete(value))),
        Ok(None) => Ok(Some(Outcome::Pending(task))),
        Err(error) => Err(error),
    }
}
fn target_loaded(session: &Session, target: [i32; 3]) -> bool {
    session
        .world
        .chunk([target[0].div_euclid(16), target[2].div_euclid(16)])
        .is_some()
}
fn target_error(prefix: &str, target: [i32; 3]) -> String {
    format!("{prefix}: {}, {}, {}", target[0], target[1], target[2])
}
impl Task {
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(super) fn cancel(&mut self, session: &mut Session) -> Result<()> {
        if self.released {
            return Ok(());
        }
        self.released = true;
        self.held = [false; 2];
        // Session clears local keys before any fallible network flush. ReleaseAll
        // also ends ordinary interaction holds, and works after disconnect.
        session.input(InputIntent::ReleaseAll).map(|_| ())
    }
    pub(super) fn on_event(&mut self, session: &mut Session) -> Result<Option<Value>> {
        let result = self.advance(session);
        if !matches!(result, Ok(None)) {
            let release = self.cancel(session);
            if result.is_ok() {
                release?;
            }
        }
        result
    }
}
fn run_search(
    planner: &mut Planner,
    space: &mut ReceivedSpace<'_>,
    start: [f64; 3],
    target: [i32; 3],
    solid_target: bool,
) -> Result<bool> {
    match planner.plan(space, start, target, solid_target) {
        Ok(found) => Ok(found),
        Err(error) if error == "navigation search bound" => Ok(false),
        Err(error) => Err(error),
    }
}
impl Task {
    fn plan(&mut self, session: &Session) -> Result<()> {
        if self.replans >= 24 {
            return Err(target_error(
                "target is unreachable from the current loaded client position",
                self.target,
            ));
        }
        self.replans += 1;
        let solid = {
            let mut space = ReceivedSpace {
                session,
                colliders: &mut self.colliders,
            };
            space.target_solid(self.target)?
        };
        let mut space = ReceivedSpace {
            session,
            colliders: &mut self.colliders,
        };
        let mut found = run_search(
            &mut self.planner,
            &mut space,
            session.player.position,
            self.target,
            solid,
        );
        let mut adjacent_fallback = false;
        if found.as_ref().is_ok_and(|inner| !inner) && solid {
            // No walkable endpoint beyond the start cell (e.g. the only
            // standable cell near a wall is the one already occupied):
            // standing beside the solid target counts as arrived, as before.
            let mut space = ReceivedSpace {
                session,
                colliders: &mut self.colliders,
            };
            found = run_search(
                &mut self.planner,
                &mut space,
                session.player.position,
                self.target,
                false,
            );
            adjacent_fallback = true;
        }
        if !found? {
            // A far leg that ends short of the target is still walked: the
            // task advances along it and replans closer. Only a plan with no
            // walkable cells at all is unreachable.
            if self.planner.path.len() < 2 {
                return Err(target_error(
                    "target is unreachable from the current loaded client position",
                    self.target,
                ));
            }
        }
        // A solid target is satisfied by any walkable endpoint the search
        // returned (adjacent or atop): the arrival gate requires reaching
        // that endpoint, never the raw target cell. A single-node path only
        // counts for air targets and the adjacent fallback.
        if self.planner.path.len() < 2
            && (solid && !adjacent_fallback
                || !planner::near(*self.planner.path.first().unwrap(), self.target, 1.5))
        {
            return Err(target_error(
                "target is unreachable from the current loaded client position",
                self.target,
            ));
        }
        self.waypoint = 0;
        self.best_distance = f64::INFINITY;
        self.progress_tick = session.client_tick;
        Ok(())
    }
    fn advance(&mut self, session: &mut Session) -> Result<Option<Value>> {
        if self.released {
            return Err("navigation was cancelled".into());
        }
        if session.context_epochs() != self.epochs {
            return Err("stale_context: navigation connection or world changed".into());
        }
        require_play(session)?;
        let p = session.player.position;
        if !p.iter().all(|v| v.is_finite()) {
            return Err("client produced an invalid navigation observation".into());
        }
        if !target_loaded(session, self.target) {
            return Err(target_error(
                "target chunk became unloaded during navigation",
                self.target,
            ));
        }
        let nearby = session.player.grounded && planner::near(p, self.target, 1.5);
        // A solid target (Nastya's placed 1-up step) is never "arrived" from
        // beside it: the player must reach the standable endpoint the planner
        // chose atop/adjacent to it. Air targets keep the old nearby-pose rule.
        let solid = if nearby {
            let mut space = ReceivedSpace {
                session,
                colliders: &mut self.colliders,
            };
            space.target_solid(self.target)?
        } else {
            false
        };
        if solid && self.planner.path.is_empty() {
            if !session.player.grounded {
                return Ok(None);
            }
            self.plan(session)?;
        }
        let arrived_at_endpoint = solid
            && self.planner.path.last().is_some_and(|end| {
                horizontal_squared(p, *end) <= 0.6 * 0.6 && (p[1] - end[1]).abs() <= 0.6
            });
        let free = if nearby && (!solid || arrived_at_endpoint) {
            let mut space = ReceivedSpace {
                session,
                colliders: &mut self.colliders,
            };
            self.planner.collision_free(&mut space, p)?
        } else {
            false
        };
        if nearby && free && (!solid || arrived_at_endpoint) {
            return Ok(Some(
                json!({"arrived":true,"x":self.target[0],"y":self.target[1],"z":self.target[2],
                "player_x":p[0],"player_y":p[1],"player_z":p[2]}),
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(format!(
                "navigation timed out before arrival at {}, {}, {}",
                self.target[0], self.target[1], self.target[2]
            ));
        }
        if self.last_tick == Some(session.client_tick) {
            return Ok(None);
        }
        self.last_tick = Some(session.client_tick);
        if self.planner.path.is_empty() {
            if !session.player.grounded {
                return Ok(None);
            }
            self.plan(session)?;
        }
        // Advance only on the observed pose, never on elapsed input duration.
        while self.waypoint < self.planner.path.len() {
            let point = self.planner.path[self.waypoint];
            if horizontal_squared(p, point) <= 0.22 * 0.22
                && (p[1] - point[1]).abs() <= 0.4
                && session.player.grounded
            {
                self.waypoint += 1;
                self.best_distance = f64::INFINITY;
                self.progress_tick = session.client_tick;
            } else {
                break;
            }
        }
        if self.waypoint == self.planner.path.len() {
            // The far leg is walked; reaching its end short of the target is
            // progress, not failure: replan from here toward the target.
            let done = planner::near(p, self.target, 1.5);
            self.plan(session)?;
            if !done {
                return Ok(None);
            }
        }
        let point = self.planner.path[self.waypoint];
        let distance = horizontal_squared(p, point);
        if distance + 0.0025 < self.best_distance {
            self.best_distance = distance;
            self.progress_tick = session.client_tick;
        }
        let remaining = self.planner.path[self.waypoint..]
            .iter()
            .map(|q| horizontal_squared(p, *q))
            .fold(0f64, f64::max)
            .sqrt();
        let stall_window = (20 + (remaining / 4.) as u64).min(200);
        let stalled = session.client_tick.wrapping_sub(self.progress_tick) >= stall_window;
        let blocked = if session.player.grounded {
            let mut space = ReceivedSpace {
                session,
                colliders: &mut self.colliders,
            };
            !self.planner.transition(&mut space, p, point)?
        } else {
            false
        };
        if blocked || (stalled && session.player.grounded) {
            self.movement(session, [false; 2])?;
            self.plan(session)?;
            return Ok(None);
        }
        let dx = point[0] - p[0];
        let dz = point[2] - p[2];
        let yaw = (dz.atan2(dx).to_degrees() - 90.) as f32;
        if (yaw - session.player.yaw).abs() > 0.05 || session.player.pitch.abs() > 0.05 {
            session.input(InputIntent::Look { yaw, pitch: 0. })?;
        }
        let jump = session.player.grounded && point[1] - p[1] > 0.6001;
        self.movement(session, [distance > 0.12 * 0.12, jump])?;
        Ok(None)
    }
    fn movement(&mut self, session: &mut Session, next: [bool; 2]) -> Result<()> {
        for (i, key) in ["forward", "jump"].into_iter().enumerate() {
            if self.held[i] != next[i] {
                session.input(InputIntent::Key {
                    key: key.into(),
                    pressed: next[i],
                })?;
                self.held[i] = next[i];
            }
        }
        Ok(())
    }
}
fn horizontal_squared(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[2] - b[2]).powi(2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::content::Content;
    use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap, World};
    use crate::ui::UiState;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    fn active_task(session: &Session) -> Task {
        Task {
            target: [6, 2, 7],
            epochs: session.context_epochs(),
            deadline: Instant::now() + Duration::from_secs(8),
            planner: Planner::new(0.3, 1.8),
            colliders: Vec::new(),
            waypoint: 0,
            last_tick: None,
            progress_tick: session.client_tick,
            best_distance: f64::INFINITY,
            replans: 0,
            held: [true; 2],
            released: false,
        }
    }

    fn hold_movement(session: &mut Session) {
        for key in [
            "forward", "back", "left", "right", "jump", "sneak", "sprint",
        ] {
            session
                .input(InputIntent::Key {
                    key: key.into(),
                    pressed: true,
                })
                .unwrap();
        }
    }

    #[test]
    fn cancellation_releases_real_session_inputs_and_cannot_resume() {
        let mut session = Session::new_mode("navigation-cancel-test".into(), false);
        hold_movement(&mut session);
        let mut task = active_task(&session);
        task.cancel(&mut session).unwrap();
        assert_eq!(session.observation().data["inputs"]["held"], json!([]));
        assert_eq!(
            task.on_event(&mut session).unwrap_err(),
            "navigation was cancelled"
        );
        task.cancel(&mut session).unwrap();
        assert_eq!(session.observation().data["inputs"]["held"], json!([]));
    }

    #[test]
    fn stale_world_task_errors_and_releases_real_session_inputs() {
        let mut session = Session::new_mode("navigation-fence-test".into(), false);
        hold_movement(&mut session);
        let mut task = active_task(&session);
        // A task captured from a previous world cannot control the current one.
        task.epochs.1 = task.epochs.1.wrapping_sub(1);
        assert_eq!(
            task.on_event(&mut session).unwrap_err(),
            "stale_context: navigation connection or world changed"
        );
        assert_eq!(session.observation().data["inputs"]["held"], json!([]));
    }

    fn play_session(name: &str) -> Session {
        let mut session = Session::new_mode(name.into(), false);
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        content.insert_test_font("minecraft:default", &json!({"providers":[]}));
        content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
        session.ui = Some(UiState::new(&content).unwrap());
        session.content = Some(content);
        session.entities.models = None;
        session
            .entities
            .activate_models(Arc::new(crate::entities::Models {
                rigs: BTreeMap::new(),
                schemas: BTreeMap::new(),
                families: BTreeMap::new(),
                builtin_variants: BTreeMap::new(),
                simple_particles: Vec::new(),
                default_skins: Vec::new(),
                poses: BTreeMap::new(),
                attributes: BTreeMap::new(),
                attribute_ids: BTreeMap::new(),
                digest: String::new(),
                armor_models: BTreeMap::new(),
                motion: BTreeMap::new(),
            }));
        session.phase = "play".into();
        session.player.pose_known = true;
        session.player.grounded = true;
        session
    }

    /// Flat grass: state 1 fills y=0 as ground, state 0 is air above.
    fn grass_world(session: &mut Session, chunks: std::ops::Range<i32>) {
        session.world = World::new(0, 16);
        let uniform = |size| Palette {
            bits: 0,
            values: vec![0],
            words: Vec::new(),
            size,
        };
        for x in chunks.clone() {
            for z in 3..6 {
                session
                    .world
                    .insert(
                        [x, z],
                        Chunk {
                            sections: vec![Arc::new(Section {
                                blocks: {
                                    let mut palette = uniform(4096);
                                    for zz in 0..16 {
                                        for xx in 0..16 {
                                            palette.set(zz * 16 + xx, 1);
                                        }
                                    }
                                    palette
                                },
                                biomes: uniform(64),
                                non_empty_block_count: 256,
                            })],
                            sky: BTreeMap::from([(1, Arc::new(vec![0xff; 2048]))]),
                            block: BTreeMap::from([(1, Arc::new(vec![0; 2048]))]),
                            block_entities: crate::protocol::world::empty_block_entities(),
                            surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                            surface_heightmap_received: false,
                        },
                    )
                    .unwrap();
            }
        }
    }

    #[test]
    fn flat_grass_hops_plan_without_false_unreachable() {
        // Gosha/Kolya/Lena/Vasya: 1-block steps and 4-15 block hops on flat
        // grass were rejected while manual press_inputs walked the same
        // ground. The planner must find a walkable path for each of them.
        let mut session = play_session("navigation-flat-grass");
        grass_world(&mut session, 8..10);
        session.player.position = [136.5, 1., 63.5];
        // Multi-block hops must produce a walkable path, never unreachable.
        for target in [[140, 1, 64], [150, 1, 65]] {
            let mut task = Task {
                target,
                epochs: session.context_epochs(),
                deadline: Instant::now() + Duration::from_secs(8),
                planner: Planner::new(0.3, 1.8),
                colliders: Vec::new(),
                waypoint: 0,
                last_tick: None,
                progress_tick: session.client_tick,
                best_distance: f64::INFINITY,
                replans: 0,
                held: [false; 2],
                released: false,
            };
            task.plan(&session)
                .unwrap_or_else(|e| panic!("flat grass {target:?} must plan: {e}"));
            assert!(task.planner.path.len() >= 2);
        }
    }

    #[test]
    fn adjacent_flat_step_arrives_without_moving() {
        // A 1-block step on flat grass is already within arrival tolerance:
        // start() must complete immediately, never error unreachable.
        let mut session = play_session("navigation-adjacent-step");
        grass_world(&mut session, 8..10);
        session.player.position = [136.5, 1., 63.5];
        let outcome = start(
            &mut session,
            "minecraft_navigate_to_block",
            &json!({"x": 137, "y": 1, "z": 63}),
        )
        .unwrap()
        .expect("adjacent step must start");
        match outcome {
            Outcome::Complete(value) => assert_eq!(value["arrived"], json!(true)),
            Outcome::Pending(_) => panic!("adjacent step must not need walking"),
        }
        assert_eq!(session.observation().data["inputs"]["held"], json!([]));
    }

    #[test]
    fn long_trip_accepts_segmented_plan() {
        // Gosha/Misha: 30-block hops and multi-hundred-block trips were
        // rejected outright. start() must admit them and produce a first
        // walkable leg with ordinary movement only (no teleport).
        let mut session = play_session("navigation-long-trip");
        grass_world(&mut session, 8..13);
        session.player.position = [136.5, 1., 63.5];
        let outcome = start(
            &mut session,
            "minecraft_navigate_to_block",
            &json!({"x": 200, "y": 1, "z": 63}),
        )
        .unwrap()
        .expect("long trip must start a task");
        match outcome {
            Outcome::Complete(_) => panic!("200-block trip must not complete instantly"),
            Outcome::Pending(_) => {}
        }
    }

    #[test]
    fn placed_step_one_up_requires_movement_never_instant_arrival() {
        // Nastya: navigating to a player-placed dirt step 1 block up from an
        // adjacent stand reported arrived:true without moving (4 tries).
        // The task must walk onto the step, never complete while the pose
        // is still on the ground beside it.
        let mut session = play_session("navigation-placed-step");
        grass_world(&mut session, 9..10);
        let place = |session: &mut Session, p: [i32; 3]| {
            session
                .world
                .set(p, 1, |state| {
                    session.content.as_ref().unwrap().block_is_air(state)
                })
                .unwrap();
        };
        place(&mut session, [149, 1, 51]);
        assert_eq!(session.world.block([149, 1, 51]), Some(1));
        session.player.position = [150.37, 1., 51.5];
        let outcome = start(
            &mut session,
            "minecraft_navigate_to_block",
            &json!({"x": 149, "y": 1, "z": 51}),
        )
        .unwrap()
        .expect("placed step must start");
        match outcome {
            Outcome::Complete(value) => {
                assert_eq!(session.player.position, [150.37, 1., 51.5]);
                panic!("must not instant-arrive beside a 1-up step: {value}");
            }
            // Pending with ordinary movement held: the task walks toward the
            // step endpoint instead of reporting arrival without moving.
            // (Release is verified on completion/cancel paths by the
            // cancellation and stale-context tests above.)
            Outcome::Pending(_) => assert_eq!(
                session.observation().data["inputs"]["held"],
                json!(["forward"])
            ),
        }
    }
}
