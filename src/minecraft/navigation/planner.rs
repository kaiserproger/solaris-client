//! Bounded cardinal A* over actual standable collision-shape tops, not block solidity.
//! Unknown space is rejected by Space; this module has no protocol or input side effects.
use std::collections::{BinaryHeap, HashMap};

pub(super) const MAX_STATES: usize = 8192;
const MAX_COLUMNS: usize = 4096;
const MAX_QUERIES: usize = 65536;
const EPS: f64 = 0.001;

pub(super) trait Space {
    fn clear(&mut self, min: [f64; 3], max: [f64; 3]) -> Result<bool, String>;
    fn supported(&mut self, position: [f64; 3], half: f64) -> Result<bool, String>;
    /// Write received block-shape tops only. No entities, guessed air, or heightmap inference.
    fn surfaces(
        &mut self,
        x: i32,
        z: i32,
        low: f64,
        high: f64,
        out: &mut Vec<f64>,
    ) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Node {
    x: i32,
    y: i32,
    z: i32,
}
impl Node {
    fn position(self) -> [f64; 3] {
        [
            f64::from(self.x) + 0.5,
            f64::from(self.y) / 4096.,
            f64::from(self.z) + 0.5,
        ]
    }
}
#[derive(Clone, Copy)]
struct Record {
    cost: u32,
    parent: Option<Node>,
}

pub(super) struct Planner {
    half: f64,
    height: f64,
    columns: HashMap<[i32; 2], Vec<Node>>,
    records: HashMap<Node, Record>,
    open: BinaryHeap<std::cmp::Reverse<(u32, u32, Node)>>,
    surfaces: Vec<f64>,
    queries: usize,
    pub(super) path: Vec<[f64; 3]>,
}
impl Planner {
    pub(super) fn new(half: f64, height: f64) -> Self {
        Self {
            half,
            height,
            columns: HashMap::new(),
            records: HashMap::new(),
            open: BinaryHeap::new(),
            surfaces: Vec::with_capacity(32),
            queries: 0,
            path: Vec::new(),
        }
    }
    fn body(&self, p: [f64; 3]) -> ([f64; 3], [f64; 3]) {
        (
            [p[0] - self.half + EPS, p[1] + EPS, p[2] - self.half + EPS],
            [
                p[0] + self.half - EPS,
                p[1] + self.height - EPS,
                p[2] + self.half - EPS,
            ],
        )
    }
    fn clear(
        &mut self,
        space: &mut impl Space,
        min: [f64; 3],
        max: [f64; 3],
    ) -> Result<bool, String> {
        self.queries += 1;
        if self.queries > MAX_QUERIES {
            return Err("navigation search bound".into());
        }
        space.clear(min, max)
    }
    pub(super) fn collision_free(
        &mut self,
        space: &mut impl Space,
        p: [f64; 3],
    ) -> Result<bool, String> {
        let (min, max) = self.body(p);
        // Runtime validation does not consume the one-search budget.
        space.clear(min, max)
    }
    /// Conservative swept body: ordinary <=0.6 step, <=1 jump with headroom,
    /// or <=3-block descent. No jumping across unsupported gaps.
    pub(super) fn transition(
        &mut self,
        space: &mut impl Space,
        from: [f64; 3],
        to: [f64; 3],
    ) -> Result<bool, String> {
        let rise = to[1] - from[1];
        if !(-3.0..=1.001).contains(&rise) {
            return Ok(false);
        }
        let elevated = from[1] + if rise > 0.6001 { 1.25 } else { rise.max(0.) };
        let (a, _) = self.body(from);
        let (_, raised) = self.body([from[0], elevated, from[2]]);
        if !space.clear(a, raised)? {
            return Ok(false);
        }
        let (lo, hi) = self.body([from[0], elevated, from[2]]);
        let (end_lo, end_hi) = self.body([to[0], elevated, to[2]]);
        let min = std::array::from_fn(|i| lo[i].min(end_lo[i]));
        let max = std::array::from_fn(|i| hi[i].max(end_hi[i]));
        if !space.clear(min, max)? {
            return Ok(false);
        }
        let (dest, _) = self.body(to);
        if !space.clear(dest, end_hi)? {
            return Ok(false);
        }
        let midpoint = [
            (from[0] + to[0]) * 0.5,
            from[1].min(to[1]),
            (from[2] + to[2]) * 0.5,
        ];
        // A supported endpoint and midpoint prevent path edges through one-cell pits.
        Ok(space.supported(to, self.half)?
            && (rise.abs() > EPS || space.supported(midpoint, self.half)?))
    }
    fn column(
        &mut self,
        space: &mut impl Space,
        x: i32,
        z: i32,
        low: f64,
        high: f64,
    ) -> Result<(), String> {
        if self.columns.contains_key(&[x, z]) {
            return Ok(());
        }
        if self.columns.len() >= MAX_COLUMNS {
            return Err("navigation search bound".into());
        }
        self.queries += 1;
        if self.queries > MAX_QUERIES {
            return Err("navigation search bound".into());
        }
        self.surfaces.clear();
        space.surfaces(x, z, low, high, &mut self.surfaces)?;
        self.surfaces.sort_unstable_by(f64::total_cmp);
        self.surfaces.dedup_by(|a, b| (*a - *b).abs() < EPS);
        if self.surfaces.len() > 32 {
            return Err("navigation search bound".into());
        }
        let mut nodes = Vec::with_capacity(self.surfaces.len());
        for index in 0..self.surfaces.len() {
            let y = self.surfaces[index];
            if !y.is_finite() || y < low || y > high {
                continue;
            }
            let scaled = (y * 4096.).ceil();
            if scaled < f64::from(i32::MIN) || scaled > f64::from(i32::MAX) {
                continue;
            }
            let node = Node {
                x,
                y: scaled as i32,
                z,
            };
            let p = node.position();
            let (min, max) = self.body(p);
            if self.clear(space, min, max)? && space.supported(p, self.half)? {
                nodes.push(node);
            }
        }
        self.columns.insert([x, z], nodes);
        Ok(())
    }
    pub(super) fn plan(
        &mut self,
        space: &mut impl Space,
        start: [f64; 3],
        target: [i32; 3],
        solid_target: bool,
    ) -> Result<bool, String> {
        self.columns.clear();
        self.records.clear();
        self.open.clear();
        self.path.clear();
        self.queries = 0;
        // Every plan anchors its finite search envelope at the caller's current
        // position. Pinning the envelope to the first plan of the task made
        // every later replan fail once the player dipped below (or climbed
        // above) that first origin's low/high band.
        let origin = start;
        let low = origin[1].min(f64::from(target[1])) - 3.;
        let high = origin[1].max(f64::from(target[1])) + 2.;
        if start[1] < low || start[1] > high {
            return Ok(false);
        }
        let x = start[0].floor() as i32;
        let z = start[2].floor() as i32;
        self.column(space, x, z, low, high)?;
        let mut root = None;
        let length = self.columns[&[x, z]].len();
        for i in 0..length {
            let node = self.columns[&[x, z]][i];
            if (node.position()[1] - start[1]).abs() < 0.6
                && self.transition(space, start, node.position())?
            {
                root = Some(node);
                break;
            }
        }
        let Some(root) = root else {
            return Ok(false);
        };
        self.records.insert(
            root,
            Record {
                cost: 0,
                parent: None,
            },
        );
        self.open
            .push(std::cmp::Reverse((heuristic(root, target), 0, root)));
        let far = (f64::from(target[0]) + 0.5 - origin[0]).powi(2)
            + (f64::from(target[2]) + 0.5 - origin[2]).powi(2)
            > 48. * 48.
            || (f64::from(target[1]) - origin[1]).abs() > 8.;
        while let Some(std::cmp::Reverse((_, cost, node))) = self.open.pop() {
            if self.records[&node].cost != cost {
                continue;
            }
            // A solid target (wall, placed step) is only satisfied by a
            // standable endpoint beyond the start cell: terminating at the
            // start cell would report "arrived" without moving (Nastya's
            // placed 1-up step). The caller falls back to adjacent arrival
            // when no further endpoint exists.
            if near(node.position(), target, 1.5) && (!solid_target || node != root) {
                let mut cursor = Some(node);
                while let Some(n) = cursor {
                    self.path.push(n.position());
                    cursor = self.records[&n].parent;
                }
                self.path.reverse();
                return Ok(true);
            }
            for [dx, dz] in [[1, 0], [0, 1], [-1, 0], [0, -1]] {
                let Some(x) = node.x.checked_add(dx) else {
                    continue;
                };
                let Some(z) = node.z.checked_add(dz) else {
                    continue;
                };
                let distance = (f64::from(x) + 0.5 - origin[0]).powi(2)
                    + (f64::from(z) + 0.5 - origin[2]).powi(2);
                if distance > 48. * 48. {
                    continue;
                }
                self.column(space, x, z, low, high)?;
                for index in 0..self.columns[&[x, z]].len() {
                    let next = self.columns[&[x, z]][index];
                    let rise = next.position()[1] - node.position()[1];
                    if !(-3.0..=1.001).contains(&rise) {
                        continue;
                    }
                    let next_cost = cost + 10 + (rise.abs() * 4.) as u32;
                    if self.records.get(&next).is_some_and(|r| r.cost <= next_cost) {
                        continue;
                    }
                    self.queries += 3;
                    if self.queries > MAX_QUERIES {
                        return Err("navigation search bound".into());
                    }
                    if !self.transition(space, node.position(), next.position())? {
                        continue;
                    }
                    if self.records.len() >= MAX_STATES {
                        return Err("navigation search bound".into());
                    }
                    self.records.insert(
                        next,
                        Record {
                            cost: next_cost,
                            parent: Some(node),
                        },
                    );
                    self.open.push(std::cmp::Reverse((
                        next_cost + heuristic(next, target),
                        next_cost,
                        next,
                    )));
                    if self.open.len() > MAX_STATES * 4 {
                        return Err("navigation search bound".into());
                    }
                }
            }
            // A far leg must never exhaust the one-search budget: stop
            // expanding once a quarter of the state budget holds a leg, and
            // return the best leg found so far below. Unknown space still
            // rejects expansion, so a leg never crosses unloaded chunks.
            const FAR_LEG_STATES: usize = MAX_STATES / 4;
            if far && self.records.len() >= FAR_LEG_STATES {
                break;
            }
        }
        if !far || self.records.len() < 2 {
            return Ok(false);
        }
        // Beyond the single-search radius, return the best first leg toward
        // the target instead of declaring it unreachable. The task walks the
        // leg with ordinary movement and replans closer; unknown space still
        // rejects expansion, so a leg never crosses unloaded chunks or walls.
        let mut endpoint: Option<Node> = None;
        let mut best = f64::INFINITY;
        for node in self.records.keys().copied() {
            let p = node.position();
            let score = (p[0] - (f64::from(target[0]) + 0.5)).powi(2)
                + (p[2] - (f64::from(target[2]) + 0.5)).powi(2)
                + (p[1] - f64::from(target[1])).powi(2) * 4.;
            if score < best {
                best = score;
                endpoint = Some(node);
            }
        }
        let Some(endpoint) = endpoint else {
            return Ok(false);
        };
        let mut cursor = Some(endpoint);
        while let Some(n) = cursor {
            self.path.push(n.position());
            cursor = self.records[&n].parent;
        }
        self.path.reverse();
        Ok(self.path.len() >= 2)
    }
}
fn heuristic(node: Node, target: [i32; 3]) -> u32 {
    // The goal is a region, not its potentially solid center. Subtract two cells
    // so the cardinal-distance lower bound remains admissible for any endpoint.
    ((node.x.abs_diff(target[0]) + node.z.abs_diff(target[2])).saturating_sub(2)) * 10
}
pub(super) fn near(p: [f64; 3], target: [i32; 3], radius: f64) -> bool {
    (p[0] - (f64::from(target[0]) + 0.5)).powi(2) + (p[2] - (f64::from(target[2]) + 0.5)).powi(2)
        <= radius * radius
        && (p[1] - f64::from(target[1])).abs() <= 1.25
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Terrain {
        walls: Vec<([f64; 3], [f64; 3])>,
        loaded: [i32; 4],
    }
    impl Terrain {
        fn flat() -> Self {
            Self {
                walls: vec![],
                loaded: [-20, 20, -20, 20],
            }
        }
        fn wall(&mut self, min: [f64; 3], max: [f64; 3]) {
            self.walls.push((min, max));
        }
    }
    impl Space for Terrain {
        fn clear(&mut self, min: [f64; 3], max: [f64; 3]) -> Result<bool, String> {
            Ok(min[0] >= f64::from(self.loaded[0])
                && max[0] <= f64::from(self.loaded[1])
                && min[2] >= f64::from(self.loaded[2])
                && max[2] <= f64::from(self.loaded[3])
                && min[1] >= -EPS
                && !self
                    .walls
                    .iter()
                    .any(|(a, b)| (0..3).all(|i| max[i] > a[i] && min[i] < b[i])))
        }
        fn supported(&mut self, p: [f64; 3], _: f64) -> Result<bool, String> {
            Ok(p[1].abs() < EPS
                || self.walls.iter().any(|(a, b)| {
                    (p[1] - b[1]).abs() < EPS
                        && p[0] >= a[0]
                        && p[0] <= b[0]
                        && p[2] >= a[2]
                        && p[2] <= b[2]
                }))
        }
        fn surfaces(
            &mut self,
            x: i32,
            z: i32,
            _: f64,
            _: f64,
            out: &mut Vec<f64>,
        ) -> Result<(), String> {
            if x < self.loaded[0]
                || x >= self.loaded[1]
                || z < self.loaded[2]
                || z >= self.loaded[3]
            {
                return Ok(());
            }
            out.push(0.);
            for (a, b) in &self.walls {
                if f64::from(x) + 0.5 > a[0]
                    && f64::from(x) + 0.5 < b[0]
                    && f64::from(z) + 0.5 > a[2]
                    && f64::from(z) + 0.5 < b[2]
                {
                    out.push(b[1]);
                }
            }
            Ok(())
        }
    }
    #[test]
    fn barrier_requires_real_detour() {
        let mut terrain = Terrain::flat();
        terrain.wall([2., 0., -2.], [3., 3., 3.]);
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .plan(&mut terrain, [0.5, 0., 0.5], [6, 0, 0], false)
            .unwrap());
        assert!(planner.path.iter().any(|p| p[2] < -2. || p[2] > 3.));
        assert!(planner.path.windows(2).all(|edge| planner_transition(
            &mut terrain,
            edge[0],
            edge[1]
        )));
    }
    fn planner_transition(terrain: &mut Terrain, a: [f64; 3], b: [f64; 3]) -> bool {
        Planner::new(0.3, 1.8).transition(terrain, a, b).unwrap()
    }
    #[test]
    fn unavailable_space_cannot_bridge_route() {
        let mut terrain = Terrain::flat();
        terrain.loaded = [-2, 3, -2, 3];
        let mut planner = Planner::new(0.3, 1.8);
        assert!(!planner
            .plan(&mut terrain, [0.5, 0., 0.5], [6, 0, 0], false)
            .unwrap());
        assert!(planner.path.is_empty());
    }
    #[test]
    fn enclosed_target_is_unreachable() {
        let mut terrain = Terrain::flat();
        terrain.wall([3., 0., -3.], [4., 4., 4.]);
        terrain.wall([8., 0., -3.], [9., 4., 4.]);
        terrain.wall([3., 0., -3.], [9., 4., -2.]);
        terrain.wall([3., 0., 3.], [9., 4., 4.]);
        let mut planner = Planner::new(0.3, 1.8);
        assert!(!planner
            .plan(&mut terrain, [0.5, 0., 0.5], [6, 0, 0], false)
            .unwrap());
    }
    #[test]
    fn solid_target_has_collision_free_adjacent_endpoint() {
        let mut terrain = Terrain::flat();
        terrain.wall([5., 0., 0.], [6., 2., 1.]);
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .plan(&mut terrain, [0.5, 0., 0.5], [5, 0, 0], false)
            .unwrap());
        let end = *planner.path.last().unwrap();
        assert!(near(end, [5, 0, 0], 1.5));
        assert!(planner.collision_free(&mut terrain, end).unwrap());
        assert!(end[0] < 5. || end[0] > 6. || end[2] < 0. || end[2] > 1.);
    }

    #[test]
    fn solid_step_beside_start_routes_onto_it() {
        // Nastya: a solid 1-up step adjacent to the start must not terminate
        // at the start cell (instant "arrival" without moving). The plan must
        // reach the step top so the task walks onto it.
        let mut terrain = Terrain::flat();
        terrain.wall([4., 0., 0.], [5., 1., 1.]);
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .plan(&mut terrain, [5.87, 0., 0.5], [4, 0, 0], true)
            .unwrap());
        let end = *planner.path.last().unwrap();
        // The endpoint must be a walkable cell beyond the start cell (never
        // an instant "arrival" without moving): adjacent or atop the step.
        assert!(near(end, [4, 0, 0], 1.5));
        assert!((end[0] - 5.87).powi(2) + (end[2] - 0.5).powi(2) > 0.22 * 0.22);
        assert!(planner.path.windows(2).all(|edge| planner_transition(
            &mut terrain,
            edge[0],
            edge[1]
        )));
        // Without the solidity flag the legacy adjacent arrival still holds.
        let mut legacy = Planner::new(0.3, 1.8);
        assert!(legacy
            .plan(&mut terrain, [5.87, 0., 0.5], [4, 0, 0], false)
            .unwrap());
        assert_eq!(legacy.path.len(), 1);
    }

    #[test]
    fn jump_headroom_and_safe_descent() {
        let mut terrain = Terrain::flat();
        terrain.wall([1., 0., 0.], [2., 1., 1.]);
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .transition(&mut terrain, [0.5, 0., 0.5], [1.5, 1., 0.5])
            .unwrap());
        terrain.wall([0., 2., 0.], [2., 3., 1.]);
        assert!(!planner
            .transition(&mut terrain, [0.5, 0., 0.5], [1.5, 1., 0.5])
            .unwrap());
        assert!(!planner
            .transition(&mut Terrain::flat(), [0.5, 4., 0.5], [1.5, 0., 0.5])
            .unwrap());
    }
    #[test]
    fn exact_arrival_boundary() {
        assert!(near([-1., 1.25, 0.5], [0, 0, 0], 1.5));
        assert!(!near([-1.001, 0., 0.5], [0, 0, 0], 1.5));
    }
    #[test]
    fn replan_after_dip_uses_current_position_envelope() {
        // West plateau (top 5) with a stepped ramp down to the east lowland.
        let mut terrain = Terrain::flat();
        terrain.wall([-20., 0., -20.], [-2., 5., 20.]);
        terrain.wall([-2., 0., -2.], [-1., 4., 3.]);
        terrain.wall([-1., 0., -2.], [0., 3., 3.]);
        terrain.wall([0., 0., -2.], [1., 2., 3.]);
        terrain.wall([1., 0., -2.], [2., 1., 3.]);
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .plan(&mut terrain, [-5.5, 5., 0.5], [-8, 5, 0], false)
            .unwrap());
        // After dipping to the lowland mid-route, a replan from the current
        // position must anchor its search envelope at the current position,
        // not at the task's first origin (whose low bound is y=2 here).
        assert!(planner
            .plan(&mut terrain, [5.5, 0., 0.5], [-8, 5, 0], false)
            .unwrap());
    }
    #[test]
    fn one_wide_two_tall_tunnel_with_steps() {
        // Masha: 1-wide horizontal tunnels must stay routable, including
        // stepping up and down one block inside them.
        let mut terrain = Terrain::flat();
        terrain.wall([-1., 0., -8.], [9., 3., -1.]);
        terrain.wall([-1., 0., 1.], [9., 3., 8.]);
        terrain.wall([-1., 2., -1.], [9., 3., 2.]);
        terrain.wall([4., 0., -1.], [5., 1., 2.]);
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .plan(&mut terrain, [0.5, 0., 0.5], [7, 0, 0], false)
            .unwrap());
        assert!(planner.path.windows(2).all(|edge| planner_transition(
            &mut terrain,
            edge[0],
            edge[1]
        )));
    }
    #[test]
    fn far_target_plans_first_leg_toward_it() {
        let mut terrain = Terrain::flat();
        terrain.loaded = [-200, 200, -200, 200];
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .plan(&mut terrain, [0.5, 0., 0.5], [150, 0, 0], false)
            .unwrap());
        assert!(planner.path.len() >= 2);
        let end = *planner.path.last().unwrap();
        let start_distance = (150.5f64 - 0.5).powi(2);
        let end_distance = (150.5f64 - end[0]).powi(2) + (0.5f64 - end[2]).powi(2);
        assert!(end_distance < start_distance);
        assert!((end[0] - 0.5).powi(2) + (end[2] - 0.5).powi(2) <= 49. * 49.);
        assert!(planner.path.windows(2).all(|edge| planner_transition(
            &mut terrain,
            edge[0],
            edge[1]
        )));
    }
    #[test]
    fn far_leg_never_enters_unloaded_space() {
        let mut terrain = Terrain::flat();
        terrain.loaded = [-2, 3, -2, 3];
        let mut planner = Planner::new(0.3, 1.8);
        assert!(planner
            .plan(&mut terrain, [0.5, 0., 0.5], [150, 0, 0], false)
            .unwrap());
        assert!(planner.path.len() >= 2);
        for p in &planner.path {
            assert!(p[0] >= -2. && p[0] <= 3. && p[2] >= -2. && p[2] <= 3.);
        }
        assert!(!near(*planner.path.last().unwrap(), [150, 0, 0], 1.5));
    }
    #[test]
    fn search_work_is_bounded() {
        let mut terrain = Terrain::flat();
        terrain.loaded = [-100, 100, -100, 100];
        let mut planner = Planner::new(0.3, 1.8);
        let result = planner.plan(&mut terrain, [0.5, 0., 0.5], [0, 8, 0], false);
        assert_eq!(result.unwrap_err(), "navigation search bound");
        assert!(planner.records.len() <= MAX_STATES);
        assert!(planner.columns.len() <= MAX_COLUMNS);
    }
}
