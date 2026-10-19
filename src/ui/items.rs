//! Resource item graphs evaluated against actual stack and presentation context.
use crate::{
    entities,
    inventory::Stack,
    protocol::{
        Result,
        content::Content,
        models::{BakedModel, TintKind},
    },
};
use glam::{Mat4, Vec3};
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

#[derive(Clone, Debug)]
pub struct ItemContext {
    pub display: String,
    /// Exact ItemModelResolver seed supplied by the Source presentation owner.
    pub seed: i32,
    pub using_item: bool,
    pub use_ticks: f32,
    pub crossbow_pull: f32,
    pub fishing_cast: bool,
    pub dimension: String,
    pub daytime: f32,
    pub random_time: f32,
    pub spawn_compass: f32,
    pub lodestone_compass: f32,
    pub recovery_compass: f32,
    pub local_date: String,
    pub selected_bundle: Option<usize>,
}
impl Default for ItemContext {
    fn default() -> Self {
        Self {
            display: "gui".into(),
            seed: 0,
            using_item: false,
            use_ticks: 0.,
            crossbow_pull: 0.,
            fishing_cast: false,
            dimension: "minecraft:overworld".into(),
            daytime: 0.,
            random_time: 0.,
            spawn_compass: 0.,
            lodestone_compass: 0.,
            recovery_compass: 0.,
            local_date: String::new(),
            selected_bundle: None,
        }
    }
}
#[derive(Default)]
pub struct ItemModels {
    definitions: BTreeMap<String, Arc<Value>>,
    baked: BTreeMap<String, BakedModel>,
    selected: BTreeMap<
        (u32, usize, String, Vec<usize>),
        (Arc<BTreeMap<String, Value>>, BakedModel, [f32; 6]),
    >,
    entities: Option<Arc<entities::Models>>,
}
impl ItemModels {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_entity_models(&mut self, models: Arc<entities::Models>) {
        self.entities = Some(models);
        self.selected.clear();
    }
    /// Registers `model` under `id` with a one-node item graph (test fixtures only).
    #[cfg(test)]
    pub(crate) fn insert_test_model(&mut self, id: &str, model: BakedModel) {
        self.definitions.insert(
            id.into(),
            Arc::new(serde_json::json!({"model":{"type":"minecraft:model","model":id}})),
        );
        self.baked.insert(id.into(), model);
    }
    pub fn clear(&mut self) {
        self.definitions.clear();
        self.baked.clear();
        self.selected.clear();
    }
    pub fn model<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        display: &str,
    ) -> Result<&'a BakedModel> {
        let ctx = ItemContext {
            display: display.into(),
            ..ItemContext::default()
        };
        self.model_with(c, s, &ctx)
    }
    pub fn model_with<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        ctx: &ItemContext,
    ) -> Result<&'a BakedModel> {
        self.model_with_bounds(c, s, ctx).map(|(model, _)| model)
    }
    pub fn model_with_bounds<'a>(
        &'a mut self,
        c: &Content,
        s: &Stack,
        ctx: &ItemContext,
    ) -> Result<(&'a BakedModel, [f32; 6])> {
        if s.empty() {
            return Err("empty stack has no item model".into());
        }
        let id = s
            .components
            .get("minecraft:item_model")
            .and_then(Value::as_str)
            .or_else(|| c.item_names.get(&s.id).map(String::as_str))
            .ok_or("unknown item model ID")?;
        if !self.definitions.contains_key(id) {
            let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
            let bytes = c.resource_bytes(&format!("assets/{ns}/items/{path}.json"))?;
            let v: Value =
                serde_json::from_slice(&bytes).map_err(|e| format!("item graph {id}: {e}"))?;
            self.definitions.insert(id.into(), Arc::new(v));
        }
        let definition = Arc::clone(&self.definitions[id]);
        let graph = &definition["model"];
        let mut nodes = Vec::new();
        evaluate(graph, s, ctx, 0, &mut nodes)?;
        // Retain the immutable component handle alongside its identity, so address reuse cannot
        // alias a different stack. The selected resource-node identities capture dynamic branches.
        let mut identities = Vec::with_capacity(nodes.len() + 1);
        identities.push(ctx.seed as u32 as usize);
        identities.extend(nodes.iter().map(|node| *node as *const Value as usize));
        if nodes.iter().any(|node| {
            node["type"]
                .as_str()
                .is_some_and(|ty| short(ty) == "bundle/selected_item")
        }) {
            identities.extend([
                ctx.selected_bundle.unwrap_or(usize::MAX),
                ctx.using_item as usize,
                ctx.fishing_cast as usize,
                ctx.use_ticks.to_bits() as usize,
                ctx.crossbow_pull.to_bits() as usize,
                ctx.daytime.to_bits() as usize,
                ctx.random_time.to_bits() as usize,
                ctx.spawn_compass.to_bits() as usize,
                ctx.lodestone_compass.to_bits() as usize,
                ctx.recovery_compass.to_bits() as usize,
            ]);
            identities.extend(ctx.dimension.bytes().map(usize::from));
            identities.push(usize::MAX);
            identities.extend(ctx.local_date.bytes().map(usize::from));
        }
        let key = (
            s.id,
            Arc::as_ptr(&s.components) as usize,
            ctx.display.clone(),
            identities,
        );
        if !self.selected.contains_key(&key) {
            let mut result = BakedModel::default();
            for node in nodes {
                let mut model = match short(node["type"].as_str().ok_or("item graph type missing")?)
                {
                    "model" => {
                        let model_id = node["model"]
                            .as_str()
                            .ok_or("item graph model ID missing")?;
                        if !self.baked.contains_key(model_id) {
                            self.baked.insert(model_id.into(), c.bake_model(model_id)?);
                        }
                        self.baked[model_id].clone()
                    }
                    "special" => {
                        let models = self
                            .entities
                            .as_ref()
                            .ok_or("entity model resources not activated")?;
                        let mut special = entities::special_item(models, c, s, &node["model"])?;
                        let base = c.bake_model(
                            node["base"].as_str().ok_or("special item base missing")?,
                        )?;
                        special.display = base.display;
                        special
                    }
                    "bundle/selected_item" => {
                        let nested = crate::inventory::bundle_item(
                            s,
                            ctx.selected_bundle.ok_or("bundle_selection_required")?,
                            c,
                        )?;
                        let nested_context = ItemContext {
                            selected_bundle: None,
                            ..ctx.clone()
                        };
                        self.model_with(c, &nested, &nested_context)?.clone()
                    }
                    _ => return Err("invalid evaluated item node".into()),
                };
                let tints = node["tints"].as_array();
                for q in &mut model.quads {
                    // Vanilla leaves a layer untinted when its index has no tint source.
                    // Potion declares only layer0; its generated bottle layer1 has no tint.
                    q.tint = match q
                        .tint_index
                        .and_then(|index| tints.and_then(|tints| tints.get(index as usize)))
                    {
                        Some(tint) => TintKind::Constant(tint_color(c, tint, s)?),
                        None => TintKind::None,
                    };
                }
                apply_graph_transform(&mut model, &node["transformation"])?;
                result.quads.extend(model.quads);
                result.display.extend(model.display);
            }
            // Bound retained dynamic variants: old geometry can be recreated from immutable resources.
            if self.selected.len() >= 4096 {
                self.selected.clear();
            }
            let bounds = model_bounds(&result, display_matrix(&result, &ctx.display));
            let selected =
                self.selected
                    .entry(key)
                    .or_insert((Arc::clone(&s.components), result, bounds));
            return Ok((&selected.1, selected.2));
        }
        let selected = &self.selected[&key];
        Ok((&selected.1, selected.2))
    }
}
/// Source `ItemTransform.apply`: translate, rotate by the quaternion
/// `rotationXYZ(rx, ry, rz)` (matrix `Rx * Ry * Rz`, so a vertex is turned about Z
/// first and X last), scale, then recentre the unit-cube model on the origin.
pub(crate) fn display_matrix(model: &BakedModel, context: &str) -> Mat4 {
    if let Some(t) = model.display.get(context) {
        Mat4::from_translation(Vec3::from_array(t.translation))
            * Mat4::from_rotation_x(t.rotation[0].to_radians())
            * Mat4::from_rotation_y(t.rotation[1].to_radians())
            * Mat4::from_rotation_z(t.rotation[2].to_radians())
            * Mat4::from_scale(Vec3::from_array(t.scale))
            * Mat4::from_translation(Vec3::splat(-0.5))
    } else {
        Mat4::from_translation(Vec3::splat(-0.5))
    }
}
fn model_bounds(model: &BakedModel, matrix: Mat4) -> [f32; 6] {
    let mut low = Vec3::splat(f32::INFINITY);
    let mut high = Vec3::splat(f32::NEG_INFINITY);
    for quad in &model.quads {
        for position in &quad.positions {
            let point = matrix.transform_point3(Vec3::from_array(*position));
            low = low.min(point);
            high = high.max(point);
        }
    }
    if model.quads.is_empty() {
        [0.; 6]
    } else {
        [low.x, low.y, low.z, high.x, high.y, high.z]
    }
}
fn short(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}
fn evaluate<'a>(
    v: &'a Value,
    s: &Stack,
    c: &ItemContext,
    depth: u8,
    out: &mut Vec<&'a Value>,
) -> Result<()> {
    if depth > 64 || out.len() > 256 {
        return Err("item graph resource bound".into());
    }
    match short(v["type"].as_str().ok_or("item graph type missing")?) {
        "model" | "special" => out.push(v),
        "empty" => {}
        "composite" => {
            for node in v["models"]
                .as_array()
                .ok_or("composite item models missing")?
            {
                evaluate(node, s, c, depth + 1, out)?;
            }
        }
        "condition" => {
            let b = condition(v, s, c)?;
            evaluate(
                &v[if b { "on_true" } else { "on_false" }],
                s,
                c,
                depth + 1,
                out,
            )?;
        }
        "select" => {
            let selected = selection(v, s, c)?;
            let mut chosen = &v["fallback"];
            for case in v["cases"].as_array().ok_or("item select cases missing")? {
                let when = &case["when"];
                if when.as_str() == Some(selected.as_str())
                    || when
                        .as_array()
                        .is_some_and(|xs| xs.iter().any(|x| x.as_str() == Some(selected.as_str())))
                {
                    chosen = &case["model"];
                    break;
                }
            }
            if !chosen.is_null() {
                evaluate(chosen, s, c, depth + 1, out)?;
            }
        }
        "range_dispatch" => {
            let n = number(v, s, c)? * v["scale"].as_f64().unwrap_or(1.) as f32;
            let mut chosen = &v["fallback"];
            let mut threshold = f32::NEG_INFINITY;
            for entry in v["entries"]
                .as_array()
                .ok_or("item range entries missing")?
            {
                let t = entry["threshold"]
                    .as_f64()
                    .ok_or("item range threshold missing")? as f32;
                if t <= n && t >= threshold {
                    threshold = t;
                    chosen = &entry["model"];
                }
            }
            if !chosen.is_null() {
                evaluate(chosen, s, c, depth + 1, out)?;
            }
        }
        "bundle/selected_item" => {
            if c.selected_bundle.is_some() {
                out.push(v);
            }
        }
        other => return Err(format!("unsupported item graph node {other}")),
    }
    Ok(())
}
fn component<'a>(s: &'a Stack, k: &str) -> Option<&'a Value> {
    s.component(k)
}
fn condition(v: &Value, s: &Stack, c: &ItemContext) -> Result<bool> {
    Ok(
        match short(
            v["property"]
                .as_str()
                .ok_or("item condition property missing")?,
        ) {
            "using_item" => c.using_item,
            "fishing_rod/cast" => c.fishing_cast,
            "has_component" => component(
                s,
                v["component"]
                    .as_str()
                    .ok_or("component condition missing ID")?,
            )
            .is_some(),
            "broken" => {
                component(s, "minecraft:damage")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    >= component(s, "minecraft:max_damage")
                        .and_then(Value::as_i64)
                        .unwrap_or(i64::MAX)
                        - 1
            }
            "bundle/has_selected_item" => c.selected_bundle.is_some(),
            other => return Err(format!("unsupported item condition {other}")),
        },
    )
}
fn selection(v: &Value, s: &Stack, c: &ItemContext) -> Result<String> {
    Ok(
        match short(
            v["property"]
                .as_str()
                .ok_or("item selection property missing")?,
        ) {
            "display_context" => c.display.clone(),
            "context_dimension" => c.dimension.clone(),
            "local_time" => {
                if v["pattern"].as_str() != Some("MM-dd") {
                    return Err("unsupported item local-time pattern".into());
                }
                c.local_date.clone()
            }
            "block_state" => component(s, "minecraft:block_state")
                .and_then(|x| x.get(v["block_state_property"].as_str().unwrap_or("")))
                .and_then(Value::as_str)
                .unwrap_or("")
                .into(),
            "trim_material" => component(s, "minecraft:trim")
                .and_then(|x| x["material"].as_str())
                .unwrap_or("")
                .into(),
            "charge_type" => {
                let projectiles =
                    component(s, "minecraft:charged_projectiles").and_then(Value::as_array);
                if projectiles.is_some_and(|x| !x.is_empty()) {
                    if projectiles
                        .unwrap()
                        .iter()
                        .any(|x| x["item"].as_str() == Some("minecraft:firework_rocket"))
                    {
                        "rocket"
                    } else {
                        "arrow"
                    }
                } else {
                    "none"
                }
                .into()
            }
            other => return Err(format!("unsupported item select property {other}")),
        },
    )
}
fn number(v: &Value, _s: &Stack, c: &ItemContext) -> Result<f32> {
    Ok(
        match short(
            v["property"]
                .as_str()
                .ok_or("item range property missing")?,
        ) {
            "compass" => match v["target"].as_str() {
                Some("spawn") => c.spawn_compass,
                Some("lodestone") => c.lodestone_compass,
                Some("recovery") => c.recovery_compass,
                _ => return Err("compass target missing".into()),
            },
            "time" => match v["source"].as_str() {
                Some("daytime") => c.daytime,
                Some("random") => c.random_time,
                _ => return Err("item time source missing".into()),
            },
            "use_duration" => {
                if c.using_item {
                    c.use_ticks
                } else {
                    0.
                }
            }
            "use_cycle" => {
                if c.using_item {
                    c.use_ticks
                        .rem_euclid(v["period"].as_f64().unwrap_or(1.) as f32)
                } else {
                    0.
                }
            }
            "crossbow/pull" => {
                if c.using_item {
                    c.crossbow_pull
                } else {
                    0.
                }
            }
            other => return Err(format!("unsupported item range property {other}")),
        },
    )
}
fn tint_color(c: &Content, v: &Value, s: &Stack) -> Result<[f32; 3]> {
    let default = v["default"].as_i64().unwrap_or(0xffffff) as u32;
    let color = match short(v["type"].as_str().ok_or("item tint type missing")?) {
        "constant" => v["value"].as_i64().ok_or("constant item tint missing")? as u32,
        "dye" => component(s, "minecraft:dyed_color")
            .and_then(Value::as_i64)
            .map(|x| x as u32)
            .unwrap_or(default),
        "map_color" => component(s, "minecraft:map_color")
            .and_then(Value::as_i64)
            .map(|x| x as u32)
            .unwrap_or(default),
        "potion" => super::tint::potion(component(s, "minecraft:potion_contents"), default)?,
        "firework" => {
            let colors =
                component(s, "minecraft:firework_explosion").and_then(|x| x["colors"].as_array());
            if let Some(xs) = colors.filter(|xs| !xs.is_empty()) {
                let mut sums = [0u64; 3];
                for x in xs {
                    let n = x.as_u64().ok_or("invalid firework color")?;
                    for (i, shift) in [16, 8, 0].into_iter().enumerate() {
                        sums[i] += (n >> shift) & 255;
                    }
                }
                ((sums[0] / xs.len() as u64) << 16
                    | (sums[1] / xs.len() as u64) << 8
                    | sums[2] / xs.len() as u64) as u32
            } else {
                default
            }
        }
        "grass" => {
            let temperature = v["temperature"]
                .as_f64()
                .ok_or("grass tint temperature missing")? as f32;
            let downfall = v["downfall"]
                .as_f64()
                .ok_or("grass tint downfall missing")? as f32;
            if !temperature.is_finite() || !downfall.is_finite() {
                return Err("grass tint range".into());
            }
            return Ok(c.grass_tint(temperature, downfall));
        }
        other => return Err(format!("unsupported item tint {other}")),
    };
    Ok([
        ((color >> 16) & 255) as f32 / 255.,
        ((color >> 8) & 255) as f32 / 255.,
        (color & 255) as f32 / 255.,
    ])
}
fn apply_graph_transform(m: &mut BakedModel, t: &Value) -> Result<()> {
    if t.is_null() {
        return Ok(());
    }
    fn vec3(t: &Value, k: &str, default: [f32; 3]) -> Result<glam::Vec3> {
        if t[k].is_null() {
            return Ok(glam::Vec3::from_array(default));
        }
        let a = t[k].as_array().ok_or("invalid item transform")?;
        if a.len() != 3 {
            return Err("invalid item transform vector".into());
        }
        Ok(glam::Vec3::new(
            a[0].as_f64().ok_or("item transform value")? as f32,
            a[1].as_f64().ok_or("item transform value")? as f32,
            a[2].as_f64().ok_or("item transform value")? as f32,
        ))
    }
    fn quat(t: &Value, k: &str) -> Result<glam::Quat> {
        if t[k].is_null() {
            return Ok(glam::Quat::IDENTITY);
        }
        let a = t[k].as_array().ok_or("invalid item transform quaternion")?;
        if a.len() != 4 {
            return Err("invalid item transform quaternion".into());
        }
        Ok(glam::Quat::from_xyzw(
            a[0].as_f64().ok_or("item quaternion")? as f32,
            a[1].as_f64().ok_or("item quaternion")? as f32,
            a[2].as_f64().ok_or("item quaternion")? as f32,
            a[3].as_f64().ok_or("item quaternion")? as f32,
        ))
    }
    let translation = vec3(t, "translation", [0.; 3])?;
    let scale = vec3(t, "scale", [1.; 3])?;
    let left = quat(t, "left_rotation")?;
    let right = quat(t, "right_rotation")?;
    for q in &mut m.quads {
        for p in &mut q.positions {
            *p = (translation + left * (scale * (right * glam::Vec3::from_array(*p)))).to_array();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::models::ModelTransform;
    use serde_json::json;

    fn display_fixture(context: &str, transform: ModelTransform) -> BakedModel {
        let mut model = BakedModel::default();
        model.display.insert(context.into(), transform);
        model
    }

    /// Source `ItemTransform.apply` rotates by `rotationXYZ`, i.e. a vertex is turned about
    /// Z first and X last. The handheld first-person pose therefore leaves the sprite's
    /// right edge pointing up and out of the screen plane, not along the view axis.
    #[test]
    fn display_rotation_turns_about_z_first_then_y_then_x() {
        let handheld = ModelTransform {
            rotation: [0., -90., 25.],
            translation: [0.; 3],
            scale: [0.68; 3],
        };
        let model = display_fixture("firstperson_righthand", handheld);
        let sprite_x = display_matrix(&model, "firstperson_righthand").transform_vector3(Vec3::X);
        let angle = 25f32.to_radians();
        let expected = Vec3::new(0., angle.sin(), angle.cos()) * 0.68;
        assert!(
            (sprite_x - expected).length() < 1e-5,
            "sprite x axis {sprite_x:?} != {expected:?}"
        );
        // Both axes at once make the order observable on a second basis vector.
        let both = display_fixture(
            "head",
            ModelTransform {
                rotation: [30., 90., 0.],
                translation: [0.; 3],
                scale: [1.; 3],
            },
        );
        let z_axis = display_matrix(&both, "head").transform_vector3(Vec3::Z);
        // Ry(90) sends +Z to +X, then Rx(30) leaves +X alone.
        assert!((z_axis - Vec3::X).length() < 1e-5, "{z_axis:?}");
    }

    fn potion_tint_fixture(contents: Value) -> (Content, ItemModels, Stack) {
        let content = Content::test_cube();
        let mut baked = content.models[0].clone();
        for quad in &mut baked.quads {
            quad.tint = TintKind::Constant([0.1, 0.2, 0.3]);
        }
        baked.quads[0].tint_index = Some(0);
        baked.quads[1].tint_index = Some(1);
        baked.quads[2].tint_index = Some(u32::MAX);
        let mut models = ItemModels::new();
        models.baked.insert("minecraft:item/potion".into(), baked);
        models.definitions.insert(
            "test:potion".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:model",
                "model":"minecraft:item/potion",
                "tints":[{"type":"minecraft:potion","default":-13083194}]
            }})),
        );
        let mut stack = Stack::default();
        stack.id = 1;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([
            ("minecraft:item_model".into(), json!("test:potion")),
            ("minecraft:potion_contents".into(), contents),
        ]));
        (content, models, stack)
    }

    fn assert_potion_layers(model: &BakedModel, color: [f32; 3]) {
        assert_eq!(model.quads.len(), 6);
        assert_eq!(model.quads[0].tint, TintKind::Constant(color));
        for quad in &model.quads[1..] {
            assert_eq!(quad.tint, TintKind::None);
        }
    }

    #[test]
    fn potion_tint_water_uses_graph_default_and_missing_layers_are_untinted() {
        let (content, mut models, stack) = potion_tint_fixture(json!({"potion":"minecraft:water"}));
        let model = models.model(&content, &stack, "gui").unwrap();
        assert_potion_layers(model, [56. / 255., 93. / 255., 198. / 255.]);
    }

    #[test]
    fn potion_tint_custom_color_overrides_water_default() {
        let (content, mut models, stack) =
            potion_tint_fixture(json!({"potion":"minecraft:water","custom_color":0x123456}));
        let model = models.model(&content, &stack, "gui").unwrap();
        assert_potion_layers(model, [18. / 255., 52. / 255., 86. / 255.]);
    }

    #[test]
    fn potion_tint_missing_graph_sources_leave_all_layers_untinted() {
        let (content, mut models, stack) = potion_tint_fixture(json!({"potion":"minecraft:water"}));
        models.definitions.insert(
            "test:potion".into(),
            Arc::new(json!({"model":{
                "type":"minecraft:model","model":"minecraft:item/potion"
            }})),
        );
        let model = models.model(&content, &stack, "gui").unwrap();
        assert_eq!(model.quads.len(), 6);
        assert!(model.quads.iter().all(|quad| quad.tint == TintKind::None));
    }
}
