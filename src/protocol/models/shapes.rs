use crate::protocol::{
    Result,
    content::{FluidKind, FluidState},
};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, sync::Arc};

const TERRAIN_FACTS_VERSION: &str = "26.3";

#[derive(Clone, Copy, Default)]
pub(crate) struct BlockOffset {
    enabled: bool,
    vertical: bool,
    horizontal: f32,
    height: f32,
}
impl BlockOffset {
    pub fn at(self, [x, _, z]: [i32; 3]) -> [f64; 3] {
        if !self.enabled {
            return [0.; 3];
        }
        let seed = super::position_seed([x, 0, z]);
        let horizontal = |shift: u32| {
            ((f64::from(((seed >> shift) & 15i64) as f32 / 15.) - 0.5) * 0.5)
                .clamp(-f64::from(self.horizontal), f64::from(self.horizontal))
        };
        let y = if self.vertical {
            f64::from((((seed >> 4) & 15) as f32 / 15. - 1.) * self.height)
        } else {
            0.
        };
        [horizontal(0), y, horizontal(8)]
    }
}
#[derive(Clone)]
pub(crate) struct TerrainFact {
    pub selection: Arc<Vec<[f64; 6]>>,
    pub occlusion: Arc<Vec<[f64; 6]>>,
    pub inside: Option<Arc<Vec<[f64; 6]>>>,
    pub redstone_conductor: bool,
    pub faces: [Arc<Vec<[f64; 4]>>; 6],
    pub solid: bool,
    pub can_occlude: bool,
    pub shade_brightness: f32,
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    pub view_blocking: bool,
    pub light_dampening: u8,
    pub solid_render: bool,
    pub collision_full: bool,
    pub sturdy_faces: [bool; 6],
    pub is_ice_block: bool,
    pub offset: BlockOffset,
    pub skip: [Vec<u32>; 6],
    fluid_type: String,
    fluid_amount: u8,
    fluid_falling: bool,
}
#[derive(Default)]
pub(crate) struct FeatureRequirements {
    pub blocks: BTreeMap<String, Arc<[String]>>,
    pub items: BTreeMap<String, Arc<[String]>>,
}
impl FeatureRequirements {
    fn parse(value: Value) -> Result<Self> {
        let mut raw: BTreeMap<String, BTreeMap<String, Vec<String>>> =
            serde_json::from_value(value)
                .map_err(|e| format!("verified required features: {e}"))?;
        let mut cache = std::collections::BTreeSet::<Arc<[String]>>::new();
        let mut parse = |registry: &str| -> Result<BTreeMap<String, Arc<[String]>>> {
            let entries = raw
                .remove(registry)
                .ok_or("required feature registry missing")?;
            let mut out = BTreeMap::new();
            for (name, mut values) in entries {
                if super::super::content::normalize_id(&name)? != name {
                    return Err("required feature target must be canonical identifier".into());
                }
                if values.len() > 64 {
                    return Err("required feature count bound".into());
                }
                for feature in &values {
                    if super::super::content::normalize_id(feature)? != *feature {
                        return Err("required feature must be canonical identifier".into());
                    }
                }
                values.sort_unstable();
                if values.windows(2).any(|pair| pair[0] == pair[1]) {
                    return Err("duplicate required feature".into());
                }
                let shared = if let Some(shared) = cache.get::<[String]>(values.as_slice()) {
                    shared.clone()
                } else {
                    let shared: Arc<[String]> = values.into();
                    cache.insert(shared.clone());
                    shared
                };
                out.insert(name, shared);
            }
            Ok(out)
        };
        let blocks = parse("blocks")?;
        let items = parse("items")?;
        Ok(Self { blocks, items })
    }
}
type FaceBoxes = Vec<Vec<[f64; 6]>>;
type SkipMap = BTreeMap<usize, BTreeMap<usize, Vec<u32>>>;

#[derive(serde::Deserialize)]
struct TerrainReport<F, S> {
    version: String,
    states: Vec<TerrainRow<F>>,
    skip_same_block: S,
    property_domains: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    required_features: Value,
}
#[derive(serde::Deserialize)]
struct TerrainRow<F> {
    fingerprint: String,
    selection: Vec<[f64; 6]>,
    occlusion: Vec<[f64; 6]>,
    inside_owner: String,
    #[serde(default)]
    inside: Option<Vec<[f64; 6]>>,
    faces: F,
    redstone_conductor: bool,
    solid: bool,
    can_occlude: bool,
    shade_brightness: f64,
    friction: f64,
    speed_factor: f64,
    jump_factor: f64,
    view_blocking: bool,
    light_dampening: u8,
    solid_render: bool,
    collision_full: bool,
    sturdy_faces: [bool; 6],
    is_ice_block: bool,
    offset: bool,
    vertical_offset: bool,
    max_horizontal_offset: f64,
    max_vertical_offset: f64,
    fluid: TerrainFluid,
}
#[derive(serde::Deserialize)]
struct TerrainFluid {
    #[serde(rename = "type")]
    kind: String,
    amount: u8,
    falling: bool,
}

impl TerrainFact {
    pub fn load(
        path: &Path,
        fingerprints: &[u64],
        presentation: bool,
    ) -> Result<(
        Vec<Self>,
        BTreeMap<String, BTreeMap<String, Vec<String>>>,
        FeatureRequirements,
    )> {
        let bytes = crate::protocol::content::bounded_bytes(path, 64 << 20)?;
        if presentation {
            let report: TerrainReport<FaceBoxes, SkipMap> =
                serde_json::from_slice(&bytes).map_err(|e| format!("terrain facts: {e}"))?;
            Self::from_report(report, fingerprints, Some, Some)
        } else {
            // Render faces and neighbor-face suppression are not protocol/state.
            // IgnoredAny consumes them directly, without allocating their trees.
            let report: TerrainReport<serde::de::IgnoredAny, serde::de::IgnoredAny> =
                serde_json::from_slice(&bytes).map_err(|e| format!("terrain facts: {e}"))?;
            Self::from_report(report, fingerprints, |_| None, |_| None)
        }
    }
    fn from_report<F, S>(
        report: TerrainReport<F, S>,
        fingerprints: &[u64],
        face_boxes: impl Fn(F) -> Option<FaceBoxes>,
        skip_map: impl FnOnce(S) -> Option<SkipMap>,
    ) -> Result<(
        Vec<Self>,
        BTreeMap<String, BTreeMap<String, Vec<String>>>,
        FeatureRequirements,
    )> {
        if report.version != TERRAIN_FACTS_VERSION {
            return Err("terrain facts version mismatch".into());
        }
        if report.states.len() != fingerprints.len() {
            return Err("terrain facts state count mismatch".into());
        }
        let count = report.states.len();
        let skipped = skip_map(report.skip_same_block).unwrap_or_default();
        let mut boxes_cache: BTreeMap<Vec<u64>, Arc<Vec<[f64; 6]>>> = BTreeMap::new();
        let mut faces_cache: BTreeMap<Vec<u64>, Arc<Vec<[f64; 4]>>> = BTreeMap::new();
        let empty_face = Arc::new(Vec::new());
        let mut out = Vec::with_capacity(count);
        for (id, row) in report.states.into_iter().enumerate() {
            if row.fingerprint.parse::<u64>().ok() != Some(fingerprints[id]) {
                return Err(format!("terrain shape state fingerprint mismatch {id}"));
            }
            if [
                row.shade_brightness,
                row.friction,
                row.speed_factor,
                row.jump_factor,
                row.max_horizontal_offset,
                row.max_vertical_offset,
            ]
            .iter()
            .any(|value| !value.is_finite())
                || row.light_dampening > 15
                || row.fluid.amount > 8
            {
                return Err(format!("terrain numeric fact bound {id}"));
            }
            let offset = BlockOffset {
                enabled: row.offset,
                vertical: row.vertical_offset,
                horizontal: row.max_horizontal_offset as f32,
                height: row.max_vertical_offset as f32,
            };
            validate_boxes(&row.selection)?;
            validate_boxes(&row.occlusion)?;
            let at_zero = offset.at([0; 3]);
            let mut selection = row.selection;
            for b in &mut selection {
                for axis in 0..3 {
                    b[axis] -= at_zero[axis];
                    b[axis + 3] -= at_zero[axis];
                }
            }
            let selection = intern_boxes(selection, &mut boxes_cache);
            let occlusion = intern_boxes(row.occlusion, &mut boxes_cache);
            let inside = if row.inside_owner == "net.minecraft.world.level.block.PowderSnowBlock" {
                if row.inside.is_some() {
                    return Err("contextual powder-snow inside shape must not be baked".into());
                }
                None
            } else {
                let boxes = row.inside.ok_or("missing terrain inside shape")?;
                validate_boxes(&boxes)?;
                Some(intern_boxes(boxes, &mut boxes_cache))
            };
            let mut faces = std::array::from_fn(|_| Arc::clone(&empty_face));
            if let Some(raw_faces) = face_boxes(row.faces) {
                if raw_faces.len() != 6 {
                    return Err("terrain face count".into());
                }
                for (face, boxes) in raw_faces.into_iter().enumerate() {
                    validate_boxes(&boxes)?;
                    let (u, v) = match face {
                        0 | 1 => (0, 2),
                        2 | 3 => (0, 1),
                        _ => (2, 1),
                    };
                    let rectangles: Vec<[f64; 4]> = boxes
                        .iter()
                        .map(|b| [b[u], b[v], b[u + 3], b[v + 3]])
                        .collect();
                    let key = rectangles
                        .iter()
                        .flat_map(|r| r.map(f64::to_bits))
                        .collect();
                    faces[face] = faces_cache
                        .entry(key)
                        .or_insert_with(|| Arc::new(rectangles))
                        .clone();
                }
            }
            let mut skip: [Vec<u32>; 6] = std::array::from_fn(|_| Vec::new());
            if let Some(directions) = skipped.get(&id) {
                for (&direction, neighbors) in directions {
                    if direction >= 6 {
                        return Err("terrain skip direction".into());
                    }
                    for &neighbor in neighbors {
                        if neighbor as usize >= count {
                            return Err("terrain skipped state bound".into());
                        }
                        skip[direction].push(neighbor);
                    }
                    skip[direction].sort_unstable();
                    skip[direction].dedup();
                }
            }
            out.push(Self {
                selection,
                occlusion,
                inside,
                redstone_conductor: row.redstone_conductor,
                faces,
                solid: row.solid,
                can_occlude: row.can_occlude,
                shade_brightness: row.shade_brightness as f32,
                friction: row.friction as f32,
                speed_factor: row.speed_factor as f32,
                jump_factor: row.jump_factor as f32,
                view_blocking: row.view_blocking,
                light_dampening: row.light_dampening,
                solid_render: row.solid_render,
                collision_full: row.collision_full,
                sturdy_faces: row.sturdy_faces,
                is_ice_block: row.is_ice_block,
                offset,
                skip,
                fluid_type: row.fluid.kind,
                fluid_amount: row.fluid.amount,
                fluid_falling: row.fluid.falling,
            });
        }
        let domains = report.property_domains;
        for properties in domains.values() {
            for values in properties.values() {
                if values.is_empty()
                    || values.len() > 256
                    || values
                        .iter()
                        .collect::<std::collections::BTreeSet<_>>()
                        .len()
                        != values.len()
                {
                    return Err("property domain value count/duplicate bound".into());
                }
            }
        }
        let features = FeatureRequirements::parse(report.required_features)?;
        Ok((out, domains, features))
    }
    pub fn fluid(&self, block: &str) -> Result<Option<FluidState>> {
        let kind = match self.fluid_type.as_str() {
            "minecraft:empty" => return Ok(None),
            "minecraft:water" | "minecraft:flowing_water" => FluidKind::Water,
            "minecraft:lava" | "minecraft:flowing_lava" => FluidKind::Lava,
            other => return Err(format!("unsupported mandatory fluid {other}")),
        };
        Ok(Some(FluidState {
            kind,
            amount: self.fluid_amount,
            falling: self.fluid_falling,
            waterlogged: !matches!(
                block,
                "minecraft:water" | "minecraft:lava" | "minecraft:bubble_column"
            ),
        }))
    }
    #[cfg(test)]
    pub fn fixture(cube: bool) -> Self {
        let boxes = if cube {
            vec![[0., 0., 0., 1., 1., 1.]]
        } else {
            Vec::new()
        };
        let rects = if cube {
            vec![[0., 0., 1., 1.]]
        } else {
            Vec::new()
        };
        Self {
            selection: Arc::new(boxes.clone()),
            occlusion: Arc::new(boxes),
            inside: Some(Arc::new(vec![[0., 0., 0., 1., 1., 1.]])),
            redstone_conductor: cube,
            faces: std::array::from_fn(|_| Arc::new(rects.clone())),
            solid: cube,
            can_occlude: cube,
            shade_brightness: if cube { 0.2 } else { 1. },
            friction: 0.6,
            speed_factor: 1.,
            jump_factor: 1.,
            view_blocking: cube,
            light_dampening: if cube { 15 } else { 0 },
            solid_render: cube,
            collision_full: cube,
            sturdy_faces: [cube; 6],
            is_ice_block: false,
            offset: BlockOffset::default(),
            skip: std::array::from_fn(|_| Vec::new()),
            fluid_type: "minecraft:empty".into(),
            fluid_amount: 0,
            fluid_falling: false,
        }
    }
}
fn validate_boxes(boxes: &[[f64; 6]]) -> Result<()> {
    if boxes.len() > 4096 {
        return Err("terrain box count bound".into());
    }
    for coordinates in boxes {
        if coordinates
            .iter()
            .any(|v| !v.is_finite() || !(-16. ..=16.).contains(v))
        {
            return Err("terrain shape coordinate bound".into());
        }
        if (0..3).any(|a| coordinates[a] >= coordinates[a + 3]) {
            return Err("terrain reversed shape bounds".into());
        }
    }
    Ok(())
}
fn intern_boxes(
    boxes: Vec<[f64; 6]>,
    cache: &mut BTreeMap<Vec<u64>, Arc<Vec<[f64; 6]>>>,
) -> Arc<Vec<[f64; 6]>> {
    let key = boxes.iter().flat_map(|b| b.map(f64::to_bits)).collect();
    cache.entry(key).or_insert_with(|| Arc::new(boxes)).clone()
}
/// Exact rectangular union containment, with no allocation in the meshing hot path.
/// Partition each source rectangle at every target boundary; each open cell must be covered.
pub(crate) fn rectangles_cover(target: &[[f64; 4]], source: &[[f64; 4]]) -> bool {
    if source.is_empty() {
        return true;
    }
    if target.is_empty() {
        return false;
    }
    for s in source {
        let edges = |axis: usize| {
            std::iter::once(s[axis])
                .chain(std::iter::once(s[axis + 2]))
                .chain(
                    target
                        .iter()
                        .flat_map(move |t| [t[axis], t[axis + 2]])
                        .filter(move |v| *v > s[axis] && *v < s[axis + 2]),
                )
        };
        for x in edges(0) {
            if x >= s[2] {
                continue;
            }
            let right = edges(0)
                .filter(|v| *v > x)
                .min_by(f64::total_cmp)
                .unwrap_or(s[2]);
            for y in edges(1) {
                if y >= s[3] {
                    continue;
                }
                let bottom = edges(1)
                    .filter(|v| *v > y)
                    .min_by(f64::total_cmp)
                    .unwrap_or(s[3]);
                let px = (x + right) / 2.;
                let py = (y + bottom) / 2.;
                if !target
                    .iter()
                    .any(|t| px >= t[0] && px <= t[2] && py >= t[1] && py <= t[3])
                {
                    return false;
                }
            }
        }
    }
    true
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_face_union_covers_seam_but_not_hole() {
        assert!(rectangles_cover(
            &[[0., 0., 0.5, 1.], [0.5, 0., 1., 1.]],
            &[[0., 0., 1., 1.]]
        ));
        assert!(!rectangles_cover(
            &[[0., 0., 0.4, 1.], [0.6, 0., 1., 1.]],
            &[[0., 0., 1., 1.]]
        ));
        assert!(rectangles_cover(
            &[[0., 0., 1., 0.5]],
            &[[0., 0., 0.5, 0.5]]
        ));
        assert!(!rectangles_cover(&[[0., 0., 1., 0.5]], &[[0., 0., 1., 1.]]));
    }
    #[test]
    fn position_offset_is_y_independent_and_within_declared_bounds() {
        let offset = BlockOffset {
            enabled: true,
            vertical: true,
            horizontal: 0.125,
            height: 0.2,
        };
        let a = offset.at([-77, 0, 23]);
        assert_eq!(a, offset.at([-77, 100, 23]));
        assert!(
            a[0].abs() <= 0.125 && a[2].abs() <= 0.125 && a[1] >= -f64::from(0.2f32) && a[1] <= 0.
        );
    }
}
