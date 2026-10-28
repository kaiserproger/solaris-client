pub mod fluids;
pub mod lighting;
pub(crate) mod mipmaps;
pub(crate) mod shapes;
use super::{
    content::{ResourceSource, TextureRegion},
    Result,
};
use crate::api::RenderLayer;
use glam::{Mat3, Vec3};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TintKind {
    None,
    Grass,
    Foliage,
    DryFoliage,
    Water,
    Constant([f32; 3]),
}
#[derive(Clone, Debug)]
pub struct BakedQuad {
    pub positions: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub tint: TintKind,
    pub tint_index: Option<u32>,
    pub cullface: Option<u8>,
    pub face: u8,
    pub shade: bool,
    pub ambient_occlusion: bool,
    pub layer: RenderLayer,
    pub alpha: f32,
    pub texture_layer: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct ModelTransform {
    pub rotation: [f32; 3],
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}
impl Default for ModelTransform {
    fn default() -> Self {
        Self {
            rotation: [0.; 3],
            translation: [0.; 3],
            scale: [1.; 3],
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct BakedModel {
    pub quads: Vec<BakedQuad>,
    pub display: BTreeMap<String, ModelTransform>,
    parts: Vec<WeightedPart>,
    multipart: bool,
}
#[derive(Clone, Debug)]
struct WeightedPart {
    variants: Vec<(u32, Arc<Vec<BakedQuad>>)>,
    total: u32,
}
impl BakedModel {
    /// Enumerates chosen parts without allocating; random source and reseeding match 26.1.2.
    pub fn for_each_quad(&self, position: [i32; 3], mut emit: impl FnMut(&BakedQuad)) {
        for q in &self.quads {
            emit(q);
        }
        let mut random = JavaRandom::new(position_seed(position));
        let seed = if self.multipart {
            Some(random.next_long())
        } else {
            None
        };
        for part in &self.parts {
            if let Some(seed) = seed {
                random = JavaRandom::new(seed);
            }
            let mut choice = if part.variants.len() == 1 {
                0
            } else {
                random.next_int(part.total)
            };
            for (weight, quads) in &part.variants {
                if choice < *weight {
                    for q in quads.iter() {
                        emit(q);
                    }
                    break;
                }
                choice -= weight;
            }
        }
    }
    pub fn maximum_quad_count(&self) -> usize {
        self.quads.len()
            + self
                .parts
                .iter()
                .map(|p| p.variants.iter().map(|(_, q)| q.len()).max().unwrap_or(0))
                .sum::<usize>()
    }
    pub(crate) fn visit_mut(&mut self, mut f: impl FnMut(&mut BakedQuad)) {
        for q in &mut self.quads {
            f(q);
        }
        for p in &mut self.parts {
            for (_, quads) in &mut p.variants {
                for q in Arc::make_mut(quads) {
                    f(q);
                }
            }
        }
    }
}

pub(crate) struct Baker<'a> {
    source: &'a ResourceSource,
    resolved: BTreeMap<String, Value>,
    geometry: BTreeMap<String, Arc<Vec<RawQuad>>>,
    plans: BTreeMap<String, usize>,
    models: Vec<RawModel>,
}
#[derive(Clone)]
struct RawQuad {
    quad: BakedQuad,
    texture: String,
    force_translucent: bool,
}
pub(crate) struct RawModel {
    quads: Vec<RawQuad>,
    parts: Vec<Vec<(u32, Arc<Vec<RawQuad>>)>>,
    multipart: bool,
    display: BTreeMap<String, ModelTransform>,
}
impl RawModel {
    pub fn maximum_quad_count(&self) -> usize {
        self.quads.len()
            + self
                .parts
                .iter()
                .map(|part| part.iter().map(|(_, quads)| quads.len()).max().unwrap_or(0))
                .sum::<usize>()
    }
    pub fn textures(&self, into: &mut BTreeSet<String>) {
        for q in &self.quads {
            into.insert(q.texture.clone());
        }
        for p in &self.parts {
            for (_, qs) in p {
                for q in qs.iter() {
                    into.insert(q.texture.clone());
                }
            }
        }
    }
    pub fn finish(
        self,
        textures: &BTreeMap<String, TextureRegion>,
        layers: &BTreeMap<String, RenderLayer>,
    ) -> Result<BakedModel> {
        let bake = |q: &RawQuad| -> Result<BakedQuad> {
            let t = textures
                .get(&q.texture)
                .ok_or_else(|| format!("missing mandatory sprite {}", q.texture))?;
            let mut out = q.quad.clone();
            for uv in &mut out.uv {
                *uv = [
                    t.uv[0] + uv[0] * (t.uv[2] - t.uv[0]),
                    t.uv[1] + uv[1] * (t.uv[3] - t.uv[1]),
                ];
            }
            out.texture_layer = t.layer;
            out.layer = if q.force_translucent {
                RenderLayer::Translucent
            } else {
                layers[&q.texture]
            };
            Ok(out)
        };
        let quads = self.quads.iter().map(&bake).collect::<Result<Vec<_>>>()?;
        let mut parts = Vec::new();
        for p in self.parts {
            let mut total = 0u32;
            let mut variants = Vec::new();
            for (weight, qs) in p {
                total = total
                    .checked_add(weight)
                    .filter(|v| *v <= i32::MAX as u32)
                    .ok_or("model total weight bound")?;
                variants.push((
                    weight,
                    Arc::new(qs.iter().map(&bake).collect::<Result<Vec<_>>>()?),
                ));
            }
            parts.push(WeightedPart { variants, total });
        }
        Ok(BakedModel {
            quads,
            parts,
            multipart: self.multipart,
            display: self.display,
        })
    }
}
impl<'a> Baker<'a> {
    pub fn new(source: &'a ResourceSource) -> Self {
        Self {
            source,
            resolved: BTreeMap::new(),
            geometry: BTreeMap::new(),
            plans: BTreeMap::new(),
            models: Vec::new(),
        }
    }
    pub fn into_models(self) -> Vec<RawModel> {
        self.models
    }
    fn resolve(&mut self, id: &str, stack: &mut Vec<String>) -> Result<Value> {
        let id = super::content::normalize_id(id)?;
        if let Some(v) = self.resolved.get(&id) {
            return Ok(v.clone());
        }
        if stack.len() >= 64 || stack.contains(&id) {
            return Err(format!("model parent cycle/depth at {id}"));
        }
        stack.push(id.clone());
        let own = self.source.json("models", &id)?;
        let mut inherited = match own["parent"].as_str() {
            Some("builtin/generated") | Some("minecraft:builtin/generated") => {
                serde_json::json!({"generated":true})
            }
            Some("builtin/entity") | Some("minecraft:builtin/entity") => {
                serde_json::json!({"special":true})
            }
            Some(p) => self.resolve(p, stack)?,
            None => serde_json::json!({}),
        };
        for (key, value) in own.as_object().ok_or("model must be an object")? {
            if key == "textures" || key == "display" {
                let mut merged = inherited[key].as_object().cloned().unwrap_or_default();
                merged.extend(
                    value
                        .as_object()
                        .ok_or("model textures/display must be object")?
                        .clone(),
                );
                inherited[key] = Value::Object(merged);
            } else if key != "parent" {
                inherited[key] = value.clone();
            }
        }
        stack.pop();
        self.resolved.insert(id, inherited.clone());
        Ok(inherited)
    }
    pub fn model(&mut self, id: &str) -> Result<RawModel> {
        let model = self.resolve(id, &mut Vec::new())?;
        if model["special"].as_bool() == Some(true) {
            return Err(format!(
                "special item model {id} needs registered entity renderer"
            ));
        }
        let quads = if model["generated"].as_bool() == Some(true) {
            generated_elements(self.source, &model)?
        } else {
            bake_elements(&model, &serde_json::json!({}), TintKind::None)?
        };
        Ok(RawModel {
            quads,
            parts: Vec::new(),
            multipart: false,
            display: display_transforms(&model)?,
        })
    }
    pub fn state(&mut self, block: &str, properties: &Value, definition: &Value) -> Result<usize> {
        let mut selected = Vec::new();
        let multipart = definition["multipart"].is_array();
        if let Some(variants) = definition["variants"].as_object() {
            for (key, model) in variants {
                if variant_matches(key, properties)? {
                    selected.push(model);
                }
            }
            if selected.len() != 1 {
                return Err(format!(
                    "blockstate {block} matched {} variants",
                    selected.len()
                ));
            }
        } else if let Some(parts) = definition["multipart"].as_array() {
            if parts.len() > 1024 {
                return Err("multipart part bound".into());
            }
            for part in parts {
                if part["when"].is_null() || condition_matches(&part["when"], properties, 0)? {
                    selected.push(&part["apply"]);
                }
            }
        } else {
            return Err(format!(
                "blockstate {block} has neither variants nor multipart"
            ));
        }
        let tint = tint_kind(block, properties);
        let plan_key = format!("{multipart}:{tint:?}:{selected:?}");
        if let Some(&id) = self.plans.get(&plan_key) {
            return Ok(id);
        }
        let mut out = RawModel {
            quads: Vec::new(),
            parts: Vec::new(),
            multipart,
            display: BTreeMap::new(),
        };
        for selected in selected {
            let alternatives: Vec<&Value> = match selected.as_array() {
                Some(v) => v.iter().collect(),
                None => vec![selected],
            };
            if alternatives.is_empty() || alternatives.len() > 256 {
                return Err("model variant bound".into());
            }
            let mut group = Vec::new();
            for descriptor in alternatives {
                let weight = descriptor["weight"].as_u64().unwrap_or(1);
                if weight == 0 || weight > i32::MAX as u64 {
                    return Err("invalid variant weight".into());
                }
                let key = format!("{}:{tint:?}", descriptor);
                let qs = if let Some(qs) = self.geometry.get(&key) {
                    qs.clone()
                } else {
                    let model = self.resolve(
                        descriptor["model"]
                            .as_str()
                            .ok_or("variant missing model")?,
                        &mut Vec::new(),
                    )?;
                    let qs = Arc::new(bake_elements(&model, descriptor, tint)?);
                    self.geometry.insert(key, qs.clone());
                    qs
                };
                group.push((weight as u32, qs));
            }
            if group.len() == 1 {
                out.quads.extend(group.pop().unwrap().1.iter().cloned());
            } else {
                out.parts.push(group);
            }
        }
        let id = self.models.len();
        self.models.push(out);
        self.plans.insert(plan_key, id);
        Ok(id)
    }
}

pub fn variant_matches(key: &str, properties: &Value) -> Result<bool> {
    if key.is_empty() {
        return Ok(true);
    }
    for term in key.split(',') {
        let (property, value) = term
            .split_once('=')
            .ok_or("invalid blockstate variant predicate")?;
        if !property_matches(property, value, properties)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn property_matches(property: &str, alternatives: &str, properties: &Value) -> Result<bool> {
    let actual = properties[property]
        .as_str()
        .ok_or_else(|| format!("unknown blockstate property {property}"))?;
    let (negated, alternatives) = alternatives
        .strip_prefix('!')
        .map_or((false, alternatives), |v| (true, v));
    let found = alternatives.split('|').any(|v| v == actual);
    Ok(found != negated)
}
pub fn condition_matches(condition: &Value, properties: &Value, depth: usize) -> Result<bool> {
    if depth > 32 {
        return Err("multipart condition depth".into());
    }
    let condition = condition
        .as_object()
        .ok_or("multipart condition must be object")?;
    for (key, value) in condition {
        let matches = if key == "OR" || key == "AND" {
            let terms = value
                .as_array()
                .ok_or("multipart logical terms must be array")?;
            let mut result = key == "AND";
            for term in terms {
                let m = condition_matches(term, properties, depth + 1)?;
                if key == "AND" {
                    result &= m;
                } else {
                    result |= m;
                }
            }
            result
        } else {
            property_matches(
                key,
                value.as_str().ok_or("multipart predicate must be string")?,
                properties,
            )?
        };
        if !matches {
            return Ok(false);
        }
    }
    Ok(true)
}
const DIRECTIONS: [[f32; 3]; 6] = [
    [0., -1., 0.],
    [0., 1., 0.],
    [0., 0., -1.],
    [0., 0., 1.],
    [-1., 0., 0.],
    [1., 0., 0.],
];
const NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
fn direction(name: &str) -> Result<u8> {
    NAMES
        .iter()
        .position(|v| *v == name)
        .map(|v| v as u8)
        .ok_or_else(|| format!("invalid face {name}"))
}
fn nearest_direction(normal: Vec3) -> u8 {
    (0..6)
        .max_by(|a, b| {
            normal
                .dot(Vec3::from_array(DIRECTIONS[*a]))
                .total_cmp(&normal.dot(Vec3::from_array(DIRECTIONS[*b])))
        })
        .unwrap() as u8
}
fn coordinate(value: &Value) -> Result<f32> {
    value
        .as_f64()
        .filter(|v| v.is_finite() && (-2048. ..=2048.).contains(v))
        .map(|v| v as f32)
        .ok_or("model coordinate bound".into())
}
fn vector(value: &Value) -> Result<Vec3> {
    let a = value
        .as_array()
        .filter(|a| a.len() == 3)
        .ok_or("model vector must have 3 coordinates")?;
    Ok(Vec3::new(
        coordinate(&a[0])?,
        coordinate(&a[1])?,
        coordinate(&a[2])?,
    ))
}
fn quarter(value: &Value) -> Result<f32> {
    let n = value.as_i64().unwrap_or(0);
    if ![0, 90, 180, 270].contains(&n) {
        return Err("blockstate rotation must be quadrant".into());
    }
    Ok(-(n as f32).to_radians())
}
fn texture_ref(model: &Value, mut name: String) -> Result<(String, bool)> {
    if !name.starts_with('#') && model["textures"].get(&name).is_some() {
        name.insert(0, '#');
    }
    let mut seen = BTreeSet::new();
    for _ in 0..64 {
        if let Some(key) = name.strip_prefix('#') {
            if !seen.insert(key.to_owned()) {
                return Err(format!("texture reference cycle {key}"));
            }
            let value = &model["textures"][key];
            if let Some(next) = value.as_str() {
                name = next.to_owned();
            } else if let Some(sprite) = value["sprite"].as_str() {
                return Ok((
                    super::content::normalize_id(sprite)?,
                    value["force_translucent"].as_bool().unwrap_or(false),
                ));
            } else {
                return Err(format!("unresolved texture reference {key}"));
            }
        } else {
            return Ok((super::content::normalize_id(&name)?, false));
        }
    }
    Err("texture reference depth".into())
}
/// FaceInfo order: UV corners (min,min), (min,max), (max,max), (max,min).
pub fn face_positions(face: u8, min: [f32; 3], max: [f32; 3]) -> [[f32; 3]; 4] {
    let [a, b, c] = min;
    let [x, y, z] = max;
    match face {
        0 => [[a, b, z], [a, b, c], [x, b, c], [x, b, z]],
        1 => [[a, y, c], [a, y, z], [x, y, z], [x, y, c]],
        2 => [[x, y, c], [x, b, c], [a, b, c], [a, y, c]],
        3 => [[a, y, z], [a, b, z], [x, b, z], [x, y, z]],
        4 => [[a, y, c], [a, b, c], [a, b, z], [a, y, z]],
        _ => [[x, y, z], [x, b, z], [x, b, c], [x, y, c]],
    }
}
fn default_uv(face: u8, min: Vec3, max: Vec3) -> [f32; 4] {
    match face {
        0 => [min.x, 16. - max.z, max.x, 16. - min.z],
        1 => [min.x, min.z, max.x, max.z],
        2 => [16. - max.x, 16. - max.y, 16. - min.x, 16. - min.y],
        3 => [min.x, 16. - max.y, max.x, 16. - min.y],
        4 => [min.z, 16. - max.y, max.z, 16. - min.y],
        _ => [16. - max.z, 16. - max.y, 16. - min.z, 16. - min.y],
    }
}
fn uv_basis(face: u8) -> (Vec3, Vec3) {
    match face {
        0 => (Vec3::X, -Vec3::Z),
        1 => (Vec3::X, Vec3::Z),
        2 => (-Vec3::X, -Vec3::Y),
        3 => (Vec3::X, -Vec3::Y),
        4 => (Vec3::Z, -Vec3::Y),
        _ => (-Vec3::Z, -Vec3::Y),
    }
}
fn bake_elements(model: &Value, descriptor: &Value, tint: TintKind) -> Result<Vec<RawQuad>> {
    let rotation = Mat3::from_rotation_y(quarter(&descriptor["y"])?)
        * Mat3::from_rotation_x(quarter(&descriptor["x"])?);
    let uvlock = descriptor["uvlock"].as_bool().unwrap_or(false);
    let mut out = Vec::new();
    let Some(elements) = model["elements"].as_array() else {
        return Ok(out);
    };
    if elements.len() > 4096 {
        return Err("model element bound".into());
    }
    for element in elements {
        let min = vector(&element["from"])?;
        let max = vector(&element["to"])?;
        let mut er = Mat3::IDENTITY;
        let mut origin = Vec3::ZERO;
        if !element["rotation"].is_null() {
            let r = &element["rotation"];
            origin = vector(&r["origin"])? / 16.;
            if let Some(axis) = r["axis"].as_str() {
                let angle = coordinate(&r["angle"]).map_err(|_| "element angle")?;
                er = match axis {
                    "x" => Mat3::from_rotation_x(angle.to_radians()),
                    "y" => Mat3::from_rotation_y(angle.to_radians()),
                    "z" => Mat3::from_rotation_z(angle.to_radians()),
                    _ => return Err("element rotation axis".into()),
                };
            } else {
                let angles = if r["angles"].is_null() {
                    Vec3::new(coordinate(&r["x"])?, coordinate(&r["y"])?, coordinate(&r["z"])?)
                } else {
                    vector(&r["angles"])?
                };
                er = Mat3::from_rotation_x(angles.x.to_radians())
                    * Mat3::from_rotation_y(angles.y.to_radians())
                    * Mat3::from_rotation_z(angles.z.to_radians());
            }
            if r["rescale"].as_bool().unwrap_or(false) {
                let scale = Vec3::new(
                    1. / er.x_axis.abs().max_element(),
                    1. / er.y_axis.abs().max_element(),
                    1. / er.z_axis.abs().max_element(),
                );
                er *= Mat3::from_diagonal(scale);
            }
        }
        for (name, face) in element["faces"]
            .as_object()
            .ok_or("model faces must be object")?
        {
            let face_id = direction(name)?;
            let positions = face_positions(face_id, (min / 16.).to_array(), (max / 16.).to_array())
                .map(|p| {
                    (rotation * (er * (Vec3::from_array(p) - origin) + origin - Vec3::splat(0.5))
                        + Vec3::splat(0.5))
                    .to_array()
                });
            let normal = rotation * er * Vec3::from_array(DIRECTIONS[face_id as usize]);
            let transformed_face = nearest_direction(normal);
            let cullface = face["cullface"]
                .as_str()
                .map(direction)
                .transpose()?
                .map(|d| nearest_direction(rotation * Vec3::from_array(DIRECTIONS[d as usize])));
            let mut bounds = default_uv(face_id, min, max);
            if let Some(a) = face["uv"].as_array() {
                if a.len() != 4 {
                    return Err("face UV must have 4 values".into());
                }
                for i in 0..4 {
                    bounds[i] = a[i]
                        .as_f64()
                        .filter(|v| v.is_finite() && (-64. ..=64.).contains(v))
                        .ok_or("face UV bound")? as f32;
                }
            }
            let face_rotation = face["rotation"].as_i64().unwrap_or(0);
            if ![0, 90, 180, 270].contains(&face_rotation) {
                return Err("face rotation quadrant".into());
            }
            let corners = [
                [bounds[0] / 16., bounds[1] / 16.],
                [bounds[0] / 16., bounds[3] / 16.],
                [bounds[2] / 16., bounds[3] / 16.],
                [bounds[2] / 16., bounds[1] / 16.],
            ];
            let mut uv = std::array::from_fn(|i| corners[(i + face_rotation as usize / 90) % 4]);
            if uvlock {
                let (u, v) = uv_basis(face_id);
                let (tu, tv) = uv_basis(nearest_direction(
                    rotation * Vec3::from_array(DIRECTIONS[face_id as usize]),
                ));
                for tex in &mut uv {
                    let p = rotation * (u * (tex[0] - 0.5) + v * (tex[1] - 0.5));
                    *tex = [0.5 + p.dot(tu), 0.5 + p.dot(tv)];
                }
            }
            let (texture, force_translucent) = texture_ref(
                model,
                face["texture"]
                    .as_str()
                    .ok_or("face missing texture")?
                    .to_owned(),
            )?;
            let index = face["tintindex"].as_i64().unwrap_or(-1);
            let tint = if index < 0 {
                TintKind::None
            } else if index == 0 && matches!(tint, TintKind::None) {
                TintKind::None
            } else if matches!(index, 0 | 1) {
                tint
            } else {
                return Err(format!("unregistered tint index {index}"));
            };
            out.push(RawQuad {
                texture,
                force_translucent,
                quad: BakedQuad {
                    positions,
                    uv,
                    tint,
                    tint_index: (index >= 0).then_some(index as u32),
                    cullface,
                    face: transformed_face,
                    shade: element["shade"].as_bool().unwrap_or(true),
                    ambient_occlusion: model["ambientocclusion"].as_bool().unwrap_or(true),
                    layer: RenderLayer::Opaque,
                    alpha: 1.,
                    texture_layer: 0,
                },
            });
        }
    }
    Ok(out)
}
fn display_transforms(model: &Value) -> Result<BTreeMap<String, ModelTransform>> {
    let mut out = BTreeMap::new();
    if let Some(display) = model["display"].as_object() {
        for (context, value) in display {
            let mut transform = ModelTransform::default();
            if !value["rotation"].is_null() {
                transform.rotation = vector(&value["rotation"])?.to_array();
            }
            if !value["translation"].is_null() {
                transform.translation = (vector(&value["translation"])? / 16.).to_array();
            }
            if !value["scale"].is_null() {
                transform.scale = vector(&value["scale"])?
                    .clamp(Vec3::splat(-4.), Vec3::splat(4.))
                    .to_array();
            }
            out.insert(context.clone(), transform);
        }
    }
    Ok(out)
}
fn generated_elements(source: &ResourceSource, model: &Value) -> Result<Vec<RawQuad>> {
    let mut out = Vec::new();
    for layer in 0..5 {
        let reference = format!("layer{layer}");
        if model["textures"][&reference].is_null() {
            break;
        }
        let (texture, force_translucent) = texture_ref(model, format!("#{reference}"))?;
        let image = super::content::decode_texture(source, &texture)?;
        let metadata = source.texture_metadata(&texture)?;
        let [width, height] =
            super::content::sprite_dimensions([image.width, image.height], &metadata)?;
        let columns = image.width / width;
        let count = columns * (image.height / height);
        let frames = if let Some(frames) = metadata["animation"]["frames"].as_array() {
            frames
                .iter()
                .map(|frame| {
                    frame
                        .as_u64()
                        .or_else(|| frame["index"].as_u64())
                        .filter(|index| *index < u64::from(count))
                        .map(|index| index as u32)
                        .ok_or("generated sprite animation frame bound".to_owned())
                })
                .collect::<Result<BTreeSet<_>>>()?
        } else {
            (0..count).collect::<BTreeSet<_>>()
        };
        // The source unions exposed edges of used frames, not the opaque silhouettes.
        let mut edges = vec![0u8; (width * height) as usize];
        for frame in frames {
            let fx = (frame % columns) * width;
            let fy = (frame / columns) * height;
            let solid = |x: i32, y: i32| {
                x >= 0
                    && y >= 0
                    && x < width as i32
                    && y < height as i32
                    && image.rgba
                        [((((fy + y as u32) * image.width + fx + x as u32) * 4) + 3) as usize]
                        != 0
            };
            for y in 0..height {
                for x in 0..width {
                    if !solid(x as i32, y as i32) {
                        continue;
                    }
                    for (bit, (dx, dy)) in [(0, (0, 1)), (1, (0, -1)), (2, (-1, 0)), (3, (1, 0))] {
                        if !solid(x as i32 + dx, y as i32 + dy) {
                            edges[(y * width + x) as usize] |= 1 << bit;
                        }
                    }
                }
            }
        }
        let mut emit = |face: u8, min: [f32; 3], max: [f32; 3], uv: [[f32; 2]; 4]| -> Result<()> {
            if source
                .loader_geometry_budget()
                .is_some_and(|limit| out.len() >= limit)
            {
                return Err("Loader generated item geometry exceeds 65536 quads".into());
            }
            out.push(RawQuad {
                texture: texture.clone(),
                force_translucent,
                quad: BakedQuad {
                    positions: face_positions(face, min, max),
                    uv,
                    tint: TintKind::None,
                    tint_index: Some(layer),
                    cullface: None,
                    face,
                    shade: false,
                    ambient_occlusion: false,
                    layer: RenderLayer::Opaque,
                    alpha: 1.,
                    texture_layer: 0,
                },
            });
            Ok(())
        };
        emit(
            2,
            [0., 0., 7.5 / 16.],
            [1., 1., 8.5 / 16.],
            [[1., 0.], [1., 1.], [0., 1.], [0., 0.]],
        )?;
        emit(
            3,
            [0., 0., 7.5 / 16.],
            [1., 1., 8.5 / 16.],
            [[0., 0.], [0., 1.], [1., 1.], [1., 0.]],
        )?;
        // Merge contiguous edge pixels into one side wall per span.
        for face in [0u8, 1, 4, 5] {
            let horizontal = face < 2;
            let rows = if horizontal { height } else { width };
            let columns = if horizontal { width } else { height };
            for row in 0..rows {
                let edge = |column: u32| {
                    let (x, y) = if horizontal {
                        (column as i32, row as i32)
                    } else {
                        (row as i32, column as i32)
                    };
                    let bit = match face {
                        0 => 0,
                        1 => 1,
                        4 => 2,
                        _ => 3,
                    };
                    edges[(y as u32 * width + x as u32) as usize] & (1 << bit) != 0
                };
                let mut column = 0;
                while column < columns {
                    if !edge(column) {
                        column += 1;
                        continue;
                    }
                    let start = column;
                    while column < columns && edge(column) {
                        column += 1;
                    }
                    let (min, max, uv) = if horizontal {
                        let a = start as f32 / width as f32;
                        let b = column as f32 / width as f32;
                        let y = 1. - (row + u32::from(face == 0)) as f32 / height as f32;
                        let v = (row as f32 + 0.5) / height as f32;
                        (
                            [a, y, 7.5 / 16.],
                            [b, y, 8.5 / 16.],
                            [[a, v], [a, v], [b, v], [b, v]],
                        )
                    } else {
                        let a = 1. - column as f32 / height as f32;
                        let b = 1. - start as f32 / height as f32;
                        let x = (row + u32::from(face == 5)) as f32 / width as f32;
                        let u = (row as f32 + 0.5) / width as f32;
                        (
                            [x, a, 7.5 / 16.],
                            [x, b, 8.5 / 16.],
                            [[u, 1. - b], [u, 1. - a], [u, 1. - a], [u, 1. - b]],
                        )
                    };
                    emit(face, min, max, uv)?;
                }
            }
        }
    }
    Ok(out)
}
fn color(raw: u32) -> [f32; 3] {
    [
        ((raw >> 16) & 255) as f32 / 255.,
        ((raw >> 8) & 255) as f32 / 255.,
        (raw & 255) as f32 / 255.,
    ]
}
fn tint_kind(block: &str, properties: &Value) -> TintKind {
    match block.strip_prefix("minecraft:").unwrap_or(block) {
        "grass_block" | "short_grass" | "tall_grass" | "fern" | "large_fern" | "potted_fern"
        | "bush" | "pink_petals" | "wildflowers" | "sugar_cane" => TintKind::Grass,
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" | "vine"
        | "mangrove_leaves" => TintKind::Foliage,
        "spruce_leaves" => TintKind::Constant(color(0x619961)),
        "birch_leaves" => TintKind::Constant(color(0x80a755)),
        "leaf_litter" => TintKind::DryFoliage,
        "water" | "bubble_column" | "water_cauldron" => TintKind::Water,
        "lily_pad" => TintKind::Constant(color(0x208030)),
        "attached_melon_stem" | "attached_pumpkin_stem" => TintKind::Constant(color(0xe0c71c)),
        "redstone_wire" => {
            let power = properties["power"]
                .as_str()
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(0) as f32
                / 15.;
            let channels = [
                power * 0.6 + if power > 0. { 0.4 } else { 0.3 },
                (power * power * 0.7 - 0.5).clamp(0., 1.),
                (power * power * 0.6 - 0.7).clamp(0., 1.),
            ];
            TintKind::Constant(channels.map(|v| (v * 255.).floor() / 255.))
        }
        "melon_stem" | "pumpkin_stem" => {
            let age = properties["age"]
                .as_str()
                .and_then(|v| v.parse::<u32>().ok())
                .unwrap_or(0);
            TintKind::Constant(color((age * 32) << 16 | (255 - age * 8) << 8 | age * 4))
        }
        _ => TintKind::None,
    }
}
pub fn position_seed([x, y, z]: [i32; 3]) -> i64 {
    let n =
        i64::from(x.wrapping_mul(3129871)) ^ i64::from(z).wrapping_mul(116129781) ^ i64::from(y);
    n.wrapping_mul(n)
        .wrapping_mul(42317861)
        .wrapping_add(n.wrapping_mul(11))
        >> 16
}
#[derive(Debug)]
pub(crate) struct JavaRandom {
    state: u64,
}
impl JavaRandom {
    pub(crate) fn new(seed: i64) -> Self {
        Self {
            state: (seed as u64 ^ 0x5deece66d) & ((1 << 48) - 1),
        }
    }
    fn bits(&mut self, n: u32) -> u32 {
        self.state = (self.state.wrapping_mul(0x5deece66d).wrapping_add(11)) & ((1 << 48) - 1);
        (self.state >> (48 - n)) as u32
    }
    pub(crate) fn next_int(&mut self, bound: u32) -> u32 {
        if bound.is_power_of_two() {
            return ((u64::from(bound) * u64::from(self.bits(31))) >> 31) as u32;
        }
        loop {
            let bits = self.bits(31);
            let value = bits % bound;
            if (bits as i32)
                .wrapping_sub(value as i32)
                .wrapping_add(bound as i32 - 1)
                >= 0
            {
                return value;
            }
        }
    }
    pub(crate) fn next_long(&mut self) -> i64 {
        (i64::from(self.bits(32) as i32) << 32).wrapping_add(i64::from(self.bits(32) as i32))
    }
    pub(crate) fn next_float(&mut self) -> f32 {
        self.bits(24) as f32 * 5.9604645E-8
    }
    pub(crate) fn next_double(&mut self) -> f64 {
        ((u64::from(self.bits(26)) << 27) + u64::from(self.bits(27))) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn multipart_nested_and_negation() {
        let p = serde_json::json!({"north":"true","east":"false"});
        assert!(condition_matches(&serde_json::json!({"AND":[{"north":"true"},{"OR":[{"east":"true"},{"east":"!true"}]}]}),&p,0).unwrap());
        assert!(!condition_matches(&serde_json::json!({"north":"false|absent"}), &p, 0).unwrap());
    }
    fn slab() -> Value {
        serde_json::json!({"textures":{"all":"test:slab"},"elements":[{"from":[0,0,0],"to":[16,8,16],"faces":{"up":{"texture":"#all","cullface":"up"},"north":{"texture":"#all"}}}]})
    }
    #[test]
    fn slab_rotation_preserves_partial_bounds_and_cull_direction() {
        let qs = bake_elements(
            &slab(),
            &serde_json::json!({"x":180,"y":90}),
            TintKind::None,
        )
        .unwrap();
        assert!(qs.iter().any(|q| q.quad.cullface == Some(0)));
        assert!(qs
            .iter()
            .flat_map(|q| q.quad.positions)
            .all(|p| p[1] >= 0.49999));
        // Vanilla `Quadrant.fromXYAngles` composes `y.compose(x)`, i.e. the X
        // rotation applies first: for {x:180,y:90} north maps to WEST (face 4),
        // verified against decompiled `OctahedralGroup`/`SymmetricGroup3`
        // (`BLOCK_ROT_X_180`=P123+invertYZ, `BLOCK_ROT_Y_90`=P321+invertX;
        // composed P321+invertY sends (0,0,-1)->(-1,0,0)). East (face 5) here
        // would be the X*Y order, which vanilla does not use.
        assert!(qs.iter().any(|q| q.quad.face == 4));
    }
    #[test]
    fn face_rotation_and_uvlock_keep_world_orientation() {
        let q = bake_elements(
            &slab(),
            &serde_json::json!({"y":90,"uvlock":true}),
            TintKind::None,
        )
        .unwrap();
        let top = q.iter().find(|q| q.quad.face == 1).unwrap();
        for (actual, expected) in top
            .quad
            .uv
            .iter()
            .flatten()
            .zip([[1f32, 0.], [0., 0.], [0., 1.], [1., 1.]].iter().flatten())
        {
            assert!((*actual - *expected).abs() < 1e-6);
        }
    }
    #[test]
    fn element_rotation_rescale_keeps_axis_bounds() {
        let mut v = slab();
        v["elements"][0]["rotation"] =
            serde_json::json!({"origin":[8,8,8],"axis":"y","angle":45,"rescale":true});
        let qs = bake_elements(&v, &serde_json::json!({}), TintKind::None).unwrap();
        let top = qs.iter().find(|q| q.quad.face == 1).unwrap();
        assert!(top.quad.positions.iter().any(|p| p[0] < 0.));
        assert!(top.quad.positions.iter().all(|p| (p[1] - 0.5).abs() < 1e-6));
    }
    #[test]
    fn hanging_sign_element_rotations_bake() {
        let mut v = slab();
        v["elements"][0]["rotation"] =
            serde_json::json!({"origin":[8,0,8],"axis":"y","angle":-67.5});
        assert!(bake_elements(&v, &serde_json::json!({}), TintKind::None).is_ok());
        v["elements"][0]["rotation"]["angle"] = serde_json::json!(-90);
        assert!(bake_elements(&v, &serde_json::json!({}), TintKind::None).is_ok());
        v["elements"][0]["rotation"] = serde_json::json!({"origin":[8,0,8],"axis":"y","angle":0.0001});
        assert!(bake_elements(&v, &serde_json::json!({}), TintKind::None).is_ok());
        v["elements"][0]["rotation"]["angle"] = serde_json::json!(-112.5);
        assert!(bake_elements(&v, &serde_json::json!({}), TintKind::None).is_ok());
        v["elements"][0]["rotation"] =
            serde_json::json!({"origin":[8,0,8],"x":180,"y":-67.5,"z":-180});
        assert!(bake_elements(&v, &serde_json::json!({}), TintKind::None).is_ok());
    }
    #[test]
    fn cyclic_texture_is_rejected() {
        assert!(texture_ref(
            &serde_json::json!({"textures":{"a":"#b","b":"#a"}}),
            "#a".into()
        )
        .is_err());
    }
    #[test]
    fn java_random_weight_choice_matches_known_jdk_sequence() {
        let mut random = JavaRandom::new(0);
        assert_eq!(
            [
                random.next_int(10),
                random.next_int(10),
                random.next_int(10),
                random.next_int(10)
            ],
            [0, 8, 9, 7]
        );
    }
    #[test]
    fn grass_top_and_overlay_tints_are_not_discarded() {
        let v = serde_json::json!({"textures":{"top":"test:top","side":"test:side","overlay":"test:overlay"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"up":{"texture":"#top","tintindex":0},"north":{"texture":"#side"}}},{"from":[0,0,0],"to":[16,16,16],"faces":{"north":{"texture":"#overlay","tintindex":0}}}]});
        let qs = bake_elements(&v, &serde_json::json!({}), TintKind::Grass).unwrap();
        assert_eq!(
            qs.iter().filter(|q| q.quad.tint == TintKind::Grass).count(),
            2
        );
        assert_eq!(
            qs.iter().filter(|q| q.quad.tint == TintKind::None).count(),
            1
        );
    }
    #[test]
    fn lectern_like_tilted_panel_bakes_exact_geometry() {
        // Lectern top slab: fractional partial bounds, single-axis -22.5° element
        // rotation about the block center, and 180° face-uv rotation. Asserts exact
        // baked positions (order = winding), uv corners and surviving face id.
        let model = serde_json::json!({
            "textures": {"top": "test:top"},
            "elements": [{
                "from": [0.5, 12.0, 2.5],
                "to": [15.5, 16.0, 13.5],
                "rotation": {"angle": -22.5, "axis": "x", "origin": [8, 8, 8]},
                "faces": {
                    "up": {"uv": [0, 1, 16, 14], "rotation": 180, "texture": "#top"}
                }
            }]
        });
        let qs = bake_elements(&model, &serde_json::json!({}), TintKind::None).unwrap();
        assert_eq!(qs.len(), 1);
        assert_eq!(qs[0].texture, "test:top");
        let q = &qs[0].quad;
        assert_eq!(q.face, 1);
        assert!(q.cullface.is_none());
        let expected_positions = [
            [0.03125, 0.83039234, -0.00892531],
            [0.03125, 1.0934872, 0.62624187],
            [0.96875, 1.0934872, 0.62624187],
            [0.96875, 0.83039234, -0.00892531],
        ];
        for (actual, want) in q.positions.iter().zip(expected_positions.iter()) {
            assert!(
                actual
                    .iter()
                    .zip(want.iter())
                    .all(|(a, w)| (a - w).abs() < 1e-5),
                "{actual:?} vs {want:?}"
            );
        }
        let expected_uv = [[1.0, 0.875], [1.0, 0.0625], [0.0, 0.0625], [0.0, 0.875]];
        for (actual, want) in q.uv.iter().zip(expected_uv.iter()) {
            assert!(
                actual
                    .iter()
                    .zip(want.iter())
                    .all(|(a, w)| (a - w).abs() < 1e-6),
                "{actual:?} vs {want:?}"
            );
        }
    }
    #[test]
    fn multipart_workstation_selects_conditional_parts() {
        // Brewing-stand-like multipart: unconditional base, single-model part gated
        // on a `|` property alternative, and a weighted-apply part gated on one
        // level. Single-model parts flatten into quads; the apply array keeps one
        // weighted part with both variants.
        struct TempRoot(std::path::PathBuf);
        impl Drop for TempRoot {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let holder = TempRoot(std::env::temp_dir().join(format!(
            "solaris-models-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        )));
        let write = |rel: &str, value: serde_json::Value| {
            let path = holder.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        };
        let cube = |faces: serde_json::Value| {
            serde_json::json!({
                "textures": {"all": "test:tex"},
                "elements": [{"from": [0, 0, 0], "to": [16, 16, 16], "faces": faces}]
            })
        };
        let full = || {
            cube(serde_json::json!({
                "down": {"texture": "#all"}, "up": {"texture": "#all"},
                "north": {"texture": "#all"}, "south": {"texture": "#all"},
                "west": {"texture": "#all"}, "east": {"texture": "#all"}
            }))
        };
        write("assets/test/models/block/bench_base.json", full());
        write(
            "assets/test/models/block/bench_fill.json",
            cube(serde_json::json!({"up": {"texture": "#all"}})),
        );
        write(
            "assets/test/models/block/bench_a.json",
            cube(serde_json::json!({"north": {"texture": "#all"}})),
        );
        write(
            "assets/test/models/block/bench_b.json",
            cube(serde_json::json!({"south": {"texture": "#all"}})),
        );
        let definition = serde_json::json!({"multipart": [
            {"apply": {"model": "test:block/bench_base"}},
            {"apply": {"model": "test:block/bench_fill"}, "when": {"level": "1|2"}},
            {"apply": [{"model": "test:block/bench_a"}, {"model": "test:block/bench_b"}],
             "when": {"level": "2"}}
        ]});
        let source = super::super::content::ResourceSource::new(
            vec![holder.0.clone()],
            None,
            super::super::content::ResourceLimits::default(),
        )
        .unwrap();
        let mut baker = Baker::new(&source);
        let full_id = baker
            .state(
                "test:bench",
                &serde_json::json!({"level": "2"}),
                &definition,
            )
            .unwrap();
        let full_model = &baker.into_models()[full_id];
        assert!(full_model.multipart);
        assert_eq!(full_model.quads.len(), 7);
        assert_eq!(full_model.parts.len(), 1);
        assert_eq!(full_model.parts[0].len(), 2);
        assert!(full_model.parts[0].iter().all(|(w, _)| *w == 1));
        let mut baker = Baker::new(&source);
        let mid_id = baker
            .state(
                "test:bench",
                &serde_json::json!({"level": "1"}),
                &definition,
            )
            .unwrap();
        let mid_model = &baker.into_models()[mid_id];
        assert_eq!(mid_model.quads.len(), 7);
        assert!(mid_model.parts.is_empty());
        let mut baker = Baker::new(&source);
        let base_id = baker
            .state(
                "test:bench",
                &serde_json::json!({"level": "0"}),
                &definition,
            )
            .unwrap();
        let base_model = &baker.into_models()[base_id];
        assert_eq!(base_model.quads.len(), 6);
        assert!(base_model.parts.is_empty());
    }
}
