use super::{
    resources::{self, BoneRole, Rig},
    Entity, Metadata, Models, Store,
};
use crate::{
    api::{DynamicGeometry, EntityMaterial, RenderLayer, Vertex},
    inventory::Stack,
    protocol::{
        content::{Content, TextureRegion},
        models::{BakedModel, BakedQuad, TintKind},
        Result,
    },
};
use glam::{Mat4, Vec3};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_DYNAMIC_VERTICES: usize = 2_000_000;
#[derive(Debug)]
pub(crate) struct Binding {
    pub rig: String,
    pub texture: TextureRegion,
    pub style: RootStyle,
    pub(super) layers: Option<super::villagers::Layers>,
}
#[derive(Debug, Clone, Copy)]
pub(crate) enum RootStyle {
    Living,
    Boat,
    Minecart,
    Arrow,
    Object,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct PresentationStats {
    pub state_revision: String,
    pub known: usize,
    pub eligible: usize,
    pub submitted: usize,
    pub animated: usize,
    pub block_entities: usize,
    pub vertices: usize,
    /// Disjoint extraction contributions; materials share GPU draw calls.
    pub entity_indices: usize,
    pub block_entity_indices: usize,
    pub lighting_pending: usize,
    pub profile_textures_pending: usize,
    pub unrendered: BTreeMap<String, String>,
}
pub(super) fn value<'a>(models: &Models, e: &'a Entity, name: &str) -> Option<&'a Metadata> {
    models
        .accessor(&e.type_name, name)
        .and_then(|i| e.metadata_value(i))
}
pub(super) fn number(models: &Models, e: &Entity, name: &str) -> Option<i32> {
    value(models, e, name).and_then(Metadata::int)
}
pub(super) fn boolean(models: &Models, e: &Entity, name: &str) -> bool {
    value(models, e, name)
        .and_then(Metadata::boolean)
        .unwrap_or(false)
}
pub(super) fn texture(content: &Content, name: &str) -> Result<TextureRegion> {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    let path = path.strip_prefix("textures/").unwrap_or(path);
    let path = path.strip_suffix(".png").unwrap_or(path);
    let key;
    let key = if namespace == "minecraft" {
        path
    } else {
        key = format!("{namespace}:{path}");
        &key
    };
    content
        .texture(key)
        .ok_or_else(|| format!("entity texture not activated: {name}"))
}
pub(super) fn sampled(
    light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    point: [f64; 3],
) -> Result<Option<[f32; 2]>> {
    let value = light(point)?;
    if value.is_some_and(|levels| {
        levels
            .iter()
            .any(|v| !v.is_finite() || !(0. ..=15.).contains(v))
    }) {
        return Err("invalid sampled entity lighting".into());
    }
    Ok(value)
}
pub(super) fn entity_light(
    e: &Entity,
    models: &Models,
    tick: u64,
    partial: f32,
    dimensions: &resources::Dimensions,
    scale: f32,
    light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
) -> Result<Option<[f32; 2]>> {
    let t = e.presented_transform(tick, partial);
    if e.type_name == "minecraft:armor_stand"
        && number(models, e, "DATA_CLIENT_FLAGS").unwrap_or(0) & 16 != 0
    {
        let schema = &models.schemas[&e.type_name];
        let small = number(models, e, "DATA_CLIENT_FLAGS").unwrap_or(0) & 1 != 0;
        let dims = if small {
            match schema.dimension_constants.get("BABY_DIMENSIONS") {
                Some(resources::DimensionConstant::Dimensions(d)) => d,
                _ => return Err("small armor stand dimensions missing".into()),
            }
        } else {
            &schema.dimensions
        };
        let t = e.presented_transform(tick, 0.);
        let half = f64::from(dims.width) * 0.5;
        let low = [
            (t.position[0] - half).floor() as i32,
            t.position[1].floor() as i32,
            (t.position[2] - half).floor() as i32,
        ];
        let high = [
            (t.position[0] + half).floor() as i32,
            (t.position[1] + f64::from(dims.height)).floor() as i32,
            (t.position[2] + half).floor() as i32,
        ];
        let mut best = None;
        let mut brightness = -1.;
        for z in low[2]..=high[2] {
            for y in low[1]..=high[1] {
                for x in low[0]..=high[0] {
                    let Some(levels) = sampled(
                        light,
                        [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5],
                    )?
                    else {
                        return Ok(None);
                    };
                    let current = levels[0].max(levels[1]);
                    if current > brightness {
                        brightness = current;
                        best = Some(levels);
                    }
                    if current == 15. {
                        return Ok(Some(levels));
                    }
                }
            }
        }
        return Ok(best);
    }
    sampled(
        light,
        [
            t.position[0],
            t.position[1] + f64::from(dimensions.eye_height * scale),
            t.position[2],
        ],
    )
}
fn variant<'a>(store: &'a Store, models: &'a Models, e: &Entity) -> Option<&'a Value> {
    let registry = match e.type_name.as_str() {
        "minecraft:cat" => "minecraft:cat_variant",
        "minecraft:cow" => "minecraft:cow_variant",
        "minecraft:chicken" => "minecraft:chicken_variant",
        "minecraft:pig" => "minecraft:pig_variant",
        "minecraft:frog" => "minecraft:frog_variant",
        "minecraft:wolf" => "minecraft:wolf_variant",
        "minecraft:zombie_nautilus" => "minecraft:zombie_nautilus_variant",
        _ => return None,
    };
    let entries = store.registries.get(registry)?;
    let received = value(models, e, "DATA_VARIANT_ID").or_else(|| value(models, e, "DATA_VARIANT"));
    let (name, data) = match received {
        Some(holder) => entries.get(usize::try_from(holder.int()?).ok()?)?,
        // Cow.defineSynchedData uses VariantUtils.getDefaultOrAny(TEMPERATE).
        // Unchanged synchronized defaults need not be sent with entity pairing.
        // Resolve that default only inside the received registry, never by a
        // guessed numeric ID or an unadvertised built-in holder.
        None if e.type_name == "minecraft:cow" => entries
            .iter()
            .find(|(name, _)| name == "minecraft:temperate")
            .or_else(|| entries.first())?,
        None => return None,
    };
    if data.is_null() {
        models.builtin_variants.get(registry)?.get(name)
    } else {
        Some(data)
    }
}
pub(super) fn material(
    store: &Store,
    models: &Models,
    e: &Entity,
    content: &Content,
) -> Result<Binding> {
    if e.type_name == "minecraft:player" {
        let default = models.default_skin(Some(&e.uuid))?;
        let profile = store
            .profiles
            .get(&e.uuid)
            .and_then(|profile| profile.textures);
        let model = profile.and_then(|p| p.model).unwrap_or(default.model);
        return Ok(Binding {
            rig: if model == super::PlayerModel::Slim {
                "minecraft:player_slim#main"
            } else {
                "minecraft:player#main"
            }
            .into(),
            texture: match profile.and_then(|p| p.skin) {
                Some(region) => region,
                None => texture(content, &default.texture)?,
            },
            style: RootStyle::Living,
            layers: None,
        });
    }
    let family = models
        .families
        .get(&e.type_name)
        .ok_or_else(|| format!("no activated rig family for {}", e.type_name))?;
    let baby = boolean(models, e, "DATA_BABY_ID");
    let mut rig = if baby {
        family.baby_rig.as_ref().unwrap_or(&family.rig)
    } else {
        &family.rig
    }
    .clone();
    let mut selected = family.texture.clone();
    if let Some(data) = variant(store, models, e) {
        let model = data["model"].as_str();
        if let Some(model) = model {
            if model != "normal" && model != "temperate" {
                let candidate = format!(
                    "minecraft:{model}_{}{}#main",
                    e.type_name.split(':').nth(1).unwrap_or(""),
                    if baby { "_baby" } else { "" }
                );
                if models.has_authored_rig(&candidate) {
                    rig = candidate;
                }
            }
        }
        let asset = if baby {
            data["baby_asset_id"].as_str().or(data["asset_id"].as_str())
        } else {
            data["asset_id"].as_str()
        };
        if let Some(asset) = asset {
            selected = Some(asset.to_owned());
        }
        if data["assets"].is_object() {
            let tame = number(models, e, "DATA_FLAGS_ID").unwrap_or(0) & 4 != 0;
            let angry = number(models, e, "DATA_REMAINING_ANGER_TIME").unwrap_or(0) > 0;
            let key = if angry {
                "angry"
            } else if tame {
                "tame"
            } else {
                "wild"
            };
            let object = if baby && data["baby_assets"].is_object() {
                &data["baby_assets"]
            } else {
                &data["assets"]
            };
            selected = object[key].as_str().map(str::to_owned);
        }
    }
    if baby {
        if let Some(base) = selected.as_ref() {
            if let Some(candidate) = family.textures.iter().find(|t| {
                t.ends_with(&format!(
                    "{}_baby.png",
                    base.rsplit('/')
                        .next()
                        .unwrap_or("")
                        .trim_end_matches(".png")
                ))
            }) {
                selected = Some(candidate.clone());
            }
        }
    }
    // State-specific materials are selected only from the renderer's exported literal inventory.
    if e.type_name == "minecraft:bee" {
        let flags = number(models, e, "DATA_FLAGS_ID").unwrap_or(0);
        let angry = number(models, e, "DATA_REMAINING_ANGER_TIME").unwrap_or(0) > 0;
        let suffix = format!(
            "bee{}{}{}.png",
            if angry { "_angry" } else { "" },
            if flags & 8 != 0 { "_nectar" } else { "" },
            if baby { "_baby" } else { "" }
        );
        selected = family
            .textures
            .iter()
            .find(|t| t.ends_with(&suffix))
            .cloned();
    }
    if e.type_name == "minecraft:ghast" && boolean(models, e, "DATA_IS_CHARGING") {
        selected = family
            .textures
            .iter()
            .find(|t| t.ends_with("ghast_shooting.png"))
            .cloned();
    }
    let style = if e.type_name.ends_with("_boat") || e.type_name.ends_with("_raft") {
        RootStyle::Boat
    } else if e.type_name.ends_with("minecart") {
        RootStyle::Minecart
    } else if matches!(
        e.type_name.as_str(),
        "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:trident"
    ) {
        RootStyle::Arrow
    } else if matches!(
        e.type_name.as_str(),
        "minecraft:end_crystal"
            | "minecraft:leash_knot"
            | "minecraft:shulker_bullet"
            | "minecraft:llama_spit"
            | "minecraft:wither_skull"
            | "minecraft:wind_charge"
            | "minecraft:breeze_wind_charge"
    ) {
        RootStyle::Object
    } else {
        RootStyle::Living
    };
    let resource = selected
        .ok_or_else(|| format!("{} variant material not yet received/resolved", e.type_name))?;
    let texture = texture(content, &resource)?;
    Ok(Binding {
        rig,
        texture,
        style,
        layers: super::villagers::layers(store, models, e, content)?,
    })
}
pub fn append(
    store: &mut Store,
    content: &Content,
    origin: [f64; 3],
    camera: [f64; 3],
    camera_rotation: [f32; 2],
    radius: f32,
    tick: u64,
    partial: f32,
    light_source: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    bed_direction: &dyn Fn([i32; 3]) -> Option<[i32; 2]>,
    out: &mut DynamicGeometry,
) -> Result<PresentationStats> {
    if !radius.is_finite() || radius <= 0. || radius > 4096. {
        return Err("entity presentation radius bound".into());
    }
    let models = store
        .models
        .clone()
        .ok_or("entity model cache not activated")?;
    let mut stats = PresentationStats {
        state_revision: store.revision.to_string(),
        known: store.len(),
        ..Default::default()
    };
    let query_radius = f64::from(radius) + store.query_padding();
    let min = [
        ((camera[0] - query_radius) / 16.).floor() as i32,
        ((camera[2] - query_radius) / 16.).floor() as i32,
    ];
    store.frame_partial = partial.clamp(0., 1.);
    store.frame_tick = tick;
    let max = [
        ((camera[0] + query_radius) / 16.).floor() as i32,
        ((camera[2] + query_radius) / 16.).floor() as i32,
    ];
    // Bucket queries restrict extraction to visible-eligible neighborhood, not the whole roster.
    let mut visible = std::mem::take(&mut store.visible_ids);
    visible.clear();
    for (_, bucket) in store
        .spatial
        .range([min[0], i32::MIN]..=[max[0], i32::MAX])
        .filter(|(cell, _)| cell[1] >= min[1] && cell[1] <= max[1])
    {
        visible.extend(bucket.iter().copied());
    }
    for index in 0..visible.len() + usize::from(store.local_render_slot.is_some()) {
        let slot = if index == visible.len() {
            store.local_render_slot.unwrap()
        } else {
            store.ids[&visible[index]]
        };
        let entity = &store.slots[slot];
        let distance = entity
            .presented_transform(tick, partial)
            .position
            .iter()
            .zip(camera)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>();
        if distance > f64::from(radius) * f64::from(radius) {
            continue;
        }
        stats.eligible += 1;
        if matches!(
            entity.type_name.as_str(),
            "minecraft:marker" | "minecraft:interaction"
        ) {
            continue;
        }
        let start = out.vertices.len();
        match super::render_special::append(store, slot, &models, content, origin,
            tick, partial, light_source, out) {
            Ok(super::render_special::Handled::Unhandled) => {}
            Ok(super::render_special::Handled::Rendered | super::render_special::Handled::NoGeometry) => {
                if out.vertices.len() > start {
                    stats.submitted += 1;
                    store.slots[slot].submitted_revision = Some(store.revision);
                }
                continue;
            }
            Ok(super::render_special::Handled::PendingLighting) => {
                stats.lighting_pending += 1;
                continue;
            }
            Ok(super::render_special::Handled::PendingWorld) => continue,
            Err(reason) => {
                stats.unrendered.insert(store.slots[slot].type_name.clone(), reason);
                continue;
            }
        }
        let entity = &store.slots[slot];
        let (dimensions, scale) = match models.dimensions(entity) {
            Ok(value) => value,
            Err(reason) => {
                stats.unrendered.insert(entity.type_name.clone(), reason);
                continue;
            }
        };
        let Some(light) = entity_light(
            entity,
            &models,
            tick,
            partial,
            dimensions,
            scale,
            light_source,
        )?
        else {
            stats.lighting_pending += 1;
            continue;
        };
        if entity.type_name == "minecraft:player"
            && store.profiles.get(&entity.uuid).is_some_and(|p| {
                p.textures.is_none() && p.properties.iter().any(|p| p.name == "textures")
            })
        {
            stats.profile_textures_pending += 1;
        }
        if store.slots[slot].binding.is_none() {
            let binding = material(store, &models, &store.slots[slot], content);
            store.slots[slot].binding = Some(binding);
        }
        let entity = &store.slots[slot];
        match entity.binding.as_ref().unwrap() {
            Ok(binding) => {
                let rig = models.rig(&binding.rig);
                let tex = binding.texture;
                let t = entity.presented_transform(tick, partial);
                let body_yaw = entity
                    .source
                    .as_ref()
                    .and_then(|source| source.living.as_ref())
                    .map_or(t.yaw, |living| living.body(partial));
                let relative = Vec3::new(
                    (t.position[0] - origin[0]) as f32,
                    (t.position[1] - origin[1]) as f32,
                    (t.position[2] - origin[2]) as f32,
                );
                let mut matrix = match binding.style {
                    RootStyle::Living => {
                        Mat4::from_translation(relative)
                            * Mat4::from_rotation_y((180. - body_yaw).to_radians())
                            * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                            * Mat4::from_translation(Vec3::new(0., -1.501, 0.))
                    }
                    RootStyle::Arrow => {
                        Mat4::from_translation(relative)
                            * Mat4::from_rotation_y((t.yaw - 90.).to_radians())
                            * Mat4::from_rotation_z(t.pitch.to_radians())
                    }
                    RootStyle::Boat => {
                        Mat4::from_translation(relative + Vec3::new(0., 0.375, 0.))
                            * Mat4::from_rotation_y((180. - t.yaw).to_radians())
                            * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                            * Mat4::from_rotation_y(std::f32::consts::FRAC_PI_2)
                    }
                    RootStyle::Minecart => {
                        Mat4::from_translation(relative + Vec3::new(0., 0.375, 0.))
                            * Mat4::from_rotation_y((180. - t.yaw).to_radians())
                            * Mat4::from_rotation_z(-t.pitch.to_radians())
                            * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                            // Authored rigs place feet at y=24 pixels. Convert
                            // that baseline around the cart's 6-pixel roll pivot.
                            * Mat4::from_translation(Vec3::new(0., -18. / 16., 0.))
                    }
                    RootStyle::Object => {
                        Mat4::from_translation(relative) * Mat4::from_rotation_y(t.yaw.to_radians())
                    }
                };
                let pose = number(&models, entity, "DATA_POSE").unwrap_or(0);
                if pose == 2 && matches!(binding.style, RootStyle::Living) {
                    // Lay the rig along the bed axis, with its head at the received bed head.
                    let direction = match value(&models, entity, "SLEEPING_POS_ID") {
                        Some(Metadata::OptionalBlockPosition(Some(position))) => bed_direction(*position),
                        _ => None,
                    };
                    let (offset, yaw) = match direction {
                        Some([x, z]) => {
                            let angle: f32 = match (x, z) {
                                (0, 1) => 0., (0, -1) => 180.,
                                (-1, 0) => -90., (1, 0) => 90.,
                                _ => body_yaw,
                            };
                            let schema = &models.schemas[&entity.type_name];
                            let eye = match (boolean(&models, entity, "DATA_BABY_ID"), schema.dimension_constants.get("BABY_DIMENSIONS")) {
                                (true, Some(resources::DimensionConstant::Dimensions(d))) => d.eye_height,
                                _ => schema.dimensions.eye_height,
                            };
                            (Vec3::new(-(x as f32) * eye, 0., -(z as f32) * eye), angle)
                        }
                        None => (Vec3::ZERO, body_yaw),
                    };
                    matrix = Mat4::from_translation(relative + offset)
                        * Mat4::from_rotation_y(yaw.to_radians())
                        * Mat4::from_rotation_x(std::f32::consts::FRAC_PI_2)
                        * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                        * Mat4::from_translation(Vec3::new(0., -1.501, 0.));
                }
                if pose == 3 || pose == 1 || pose == 4 {
                    matrix = Mat4::from_translation(relative + Vec3::new(0., 0.35, 0.))
                        * Mat4::from_rotation_y((180. - t.yaw).to_radians())
                        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
                        * Mat4::from_scale(Vec3::new(-1., -1., 1.))
                        * Mat4::from_translation(Vec3::new(0., -1.501, 0.));
                }
                if matches!(binding.style, RootStyle::Living)
                    && entity.type_name != "minecraft:armor_stand"
                    && entity.source.as_ref().is_some_and(|s| s.living.is_some())
                {
                    // Landing squash + idle breathing, scaled about the feet.
                    let breathe = if entity.death_tick.is_none() && pose == 0 {
                        crate::motion::breathing(
                            ((tick % 1200) as f32 + partial) * 0.05,
                            entity.handle.network_id as f32 * 1.618,
                        )
                    } else {
                        0.
                    };
                    let s = breathe - entity.physical.motion.squash(tick, partial);
                    if s != 0. {
                        let xz = 1. - s * 0.5;
                        matrix = Mat4::from_translation(relative)
                            * Mat4::from_scale(Vec3::new(xz, 1. + s, xz))
                            * Mat4::from_translation(-relative)
                            * matrix;
                    }
                    // Short backward lean when struck; the hit flash rides along.
                    if let (Some(damage), None) = (entity.damage_tick, entity.death_tick) {
                        let lean =
                            crate::motion::hurt_lean(tick.saturating_sub(damage) as f32 + partial);
                        if lean > 0. {
                            let (sin, cos) = body_yaw.to_radians().sin_cos();
                            matrix = Mat4::from_translation(relative)
                                * Mat4::from_axis_angle(
                                    Vec3::new(cos, 0., sin),
                                    -lean.to_radians(),
                                )
                                * Mat4::from_translation(-relative)
                                * matrix;
                        }
                    }
                }
                if let Some(death) = entity.death_tick {
                    let a = ((tick.saturating_sub(death) as f32 + partial) / 20.).clamp(0., 1.);
                    matrix *= Mat4::from_rotation_z(a.sqrt() * std::f32::consts::FRAC_PI_2);
                }
                let invisible =
                    number(&models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & 32 != 0;
                evaluate_rig(
                    rig,
                    entity,
                    &models,
                    tick,
                    partial,
                    matrix,
                    &mut store.transforms,
                );
                if !invisible {
                    emit_rig(
                        rig,
                        &store.transforms,
                        tex,
                        [1.; 3],
                        EntityMaterial::EntityCutout,
                        1.,
                        light,
                        out,
                    )?;
                    super::villagers::append(
                        binding.layers.as_ref(),
                        rig,
                        &store.transforms,
                        light,
                        out,
                    )?;
                    if matches!(binding.style, RootStyle::Living)
                        && (entity
                            .damage_tick
                            .is_some_and(|damage| tick.saturating_sub(damage) < 10)
                            || entity.death_tick.is_some())
                    {
                        for vertex in &mut out.vertices[start..] {
                            vertex.overlay_uv = [0., 3.];
                        }
                    }
                }
                stats.animated += 1;
                append_equipment(
                    entity,
                    rig,
                    &store.transforms,
                    &mut store.items,
                    &mut store.equipment_cache,
                    matrix,
                    content,
                    &models,
                    tick,
                    partial,
                    light,
                    out,
                    &mut stats,
                )?;
                if !invisible {
                    super::sheep::append(
                        entity,
                        &models,
                        content,
                        boolean(&models, entity, "DATA_BABY_ID"),
                        rig,
                        tick,
                        partial,
                        matrix,
                        &mut store.transforms,
                        light,
                        out,
                    )?;
                }
            }
            Err(reason) => {
                if !append_item_entity(
                    entity,
                    &models,
                    &mut store.items,
                    content,
                    origin,
                    tick,
                    partial,
                    light,
                    out,
                    &mut stats,
                )? {
                    stats
                        .unrendered
                        .insert(entity.type_name.clone(), reason.clone());
                }
            }
        }
        if out.vertices.len() > start {
            stats.submitted += 1;
            store.slots[slot].submitted_revision = Some(store.revision);
        }
    }
    visible.clear();
    store.visible_ids = visible;
    stats.vertices = out.vertices.len();
    Ok(stats)
}
pub(super) fn evaluate_rig(
    rig: &Rig,
    e: &Entity,
    models: &Models,
    tick: u64,
    partial: f32,
    root: Mat4,
    out: &mut Vec<Mat4>,
) {
    out.clear();
    out.reserve(rig.bones.len());
    let living = e.source.as_ref().and_then(|source| source.living.as_ref());
    // Relative-position packets do not carry useful velocity for locomotion.
    // The source owner advances WalkRule from interpolated position deltas.
    let phase = living.map_or(0., |state| state.walk.position(partial) * 0.6662);
    let amplitude = living.map_or(0., |state| state.walk.speed(partial));
    // VillagerModel (villager, wandering trader, witch) swings legs at half the humanoid amplitude.
    let leg_swing = if matches!(
        e.type_name.as_str(),
        "minecraft:villager" | "minecraft:wandering_trader" | "minecraft:witch"
    ) {
        0.5
    } else {
        1.
    };
    let t = e.presented_transform(tick, partial);
    let body_yaw = living.map_or(t.yaw, |state| state.body(partial));
    let head_yaw = living.map_or(t.head_yaw, |state| state.head(partial));
    let crouch = number(models, e, "DATA_POSE") == Some(5);
    let flags = number(models, e, "DATA_LIVING_ENTITY_FLAGS").unwrap_or(0);
    let using = flags & 1 != 0;
    let attack = e
        .animation
        .map(|(_, start)| (tick.saturating_sub(start) as f32 + partial) / 6.)
        .filter(|f| *f < 1.);
    for bone in &rig.bones {
        let mut rotation = bone.rotation;
        let mut pivot = bone.pivot;
        match bone.role {
            BoneRole::Head => {
                let (look_yaw, look_pitch, weight) = e.physical.motion.look(partial);
                let mut yaw_rel =
                    ((head_yaw - body_yaw + 180.).rem_euclid(360.) - 180.).clamp(-85., 85.);
                let mut pitch = t.pitch;
                if weight > 0. {
                    // Idle mobs glance at a nearby player; weight eases in/out per tick.
                    let desired = crate::motion::angle_delta(body_yaw, look_yaw).clamp(
                        -crate::motion::LOOK_HEAD_LIMIT,
                        crate::motion::LOOK_HEAD_LIMIT,
                    );
                    yaw_rel = crate::motion::lerp(yaw_rel, desired, weight);
                    pitch = crate::motion::lerp(pitch, look_pitch, weight);
                }
                rotation.x += pitch.to_radians();
                rotation.y += yaw_rel.to_radians();
            }
            BoneRole::RightLeg => rotation.x += phase.cos() * 1.4 * amplitude * leg_swing,
            BoneRole::LeftLeg => {
                rotation.x += (phase + std::f32::consts::PI).cos() * 1.4 * amplitude * leg_swing
            }
            BoneRole::RightArm => {
                rotation.x += (phase + std::f32::consts::PI).cos() * amplitude;
                if using && flags & 2 == 0 {
                    rotation.x = -1.35;
                }
                if let Some(a) = attack {
                    rotation.x -= (a * std::f32::consts::PI).sin() * 1.2;
                }
            }
            BoneRole::LeftArm => {
                rotation.x += phase.cos() * amplitude;
                if using && flags & 2 != 0 {
                    rotation.x = -1.35;
                }
            }
            BoneRole::Wing => rotation.z += (tick as f32 * 0.3 + partial).sin() * 0.3,
            // Source `WitchModel.setupAnim`: a held item pulls the nose forward and up.
            BoneRole::Nose if e.equipment_value(0).is_some() => {
                pivot = Vec3::new(0., 1., -1.5) / 16.;
                rotation.x = -0.9;
            }
            BoneRole::Nose | BoneRole::CrossedArms | BoneRole::Fixed => {}
        }
        if crouch && bone.name == "body" {
            rotation.x += 0.5;
            pivot.y += 0.2;
        }
        if e.vehicle.is_some() {
            match bone.role {
                BoneRole::RightLeg | BoneRole::LeftLeg => rotation.x = -1.4137167,
                _ => {}
            }
        }
        if e.type_name == "minecraft:armor_stand" {
            let field = match bone.name.as_str() {
                "head" => Some("DATA_HEAD_POSE"),
                "body" => Some("DATA_BODY_POSE"),
                "left_arm" => Some("DATA_LEFT_ARM_POSE"),
                "right_arm" => Some("DATA_RIGHT_ARM_POSE"),
                "left_leg" => Some("DATA_LEFT_LEG_POSE"),
                "right_leg" => Some("DATA_RIGHT_LEG_POSE"),
                _ => None,
            };
            if let Some(Metadata::Rotations(r)) = field.and_then(|field| value(models, e, field)) {
                rotation = Vec3::from_array(*r) * std::f32::consts::PI / 180.;
            }
        }
        let parent = bone.parent.map(|i| out[i]).unwrap_or(root);
        out.push(parent * resources::transform(pivot, rotation, bone.scale));
    }
}
pub(super) fn emit_rig(
    rig: &Rig,
    transforms: &[Mat4],
    tex: TextureRegion,
    color: [f32; 3],
    layer: EntityMaterial,
    alpha: f32,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    if rig.bones.len() != transforms.len() {
        return Err("entity rig transform cardinality mismatch".into());
    }
    // A missing model must be unmistakable: placeholder boxes are tinted magenta.
    let color = if rig.is_placeholder() {
        [1., 0., 1.]
    } else {
        color
    };
    for (bone, matrix) in rig.bones.iter().zip(transforms) {
        let normal_matrix = matrix.inverse().transpose();
        for (quad, normal) in bone.quads.iter().zip(&bone.normals) {
            let positions = quad.map(|v| {
                matrix
                    .transform_point3(Vec3::new(v[0], v[1], v[2]))
                    .to_array()
            });
            let uv = quad.map(|v| {
                [
                    tex.uv[0] + v[3] * (tex.uv[2] - tex.uv[0]),
                    tex.uv[1] + v[4] * (tex.uv[3] - tex.uv[1]),
                ]
            });
            emit(positions, uv, color, tex.layer, layer, alpha, light, out)?;
            let normal = normal_matrix
                .transform_vector3(Vec3::from_array(*normal))
                .normalize_or_zero()
                .to_array();
            let first = out.vertices.len() - 4;
            for vertex in &mut out.vertices[first..] {
                vertex.normal = normal;
            }
        }
    }
    Ok(())
}
pub(crate) fn emit(
    positions: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    color: [f32; 3],
    texture_layer: u32,
    layer: EntityMaterial,
    alpha: f32,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    if out.vertices.len() + 4 > MAX_DYNAMIC_VERTICES {
        return Err("dynamic entity geometry byte/vertex budget reached".into());
    }
    let first = u32::try_from(out.vertices.len()).map_err(|_| "dynamic geometry index overflow")?;
    let normal = (Vec3::from_array(positions[1]) - Vec3::from_array(positions[0]))
        .cross(Vec3::from_array(positions[2]) - Vec3::from_array(positions[0]))
        .normalize_or_zero()
        .to_array();
    for i in 0..4 {
        out.vertices.push(Vertex {
            position: positions[i],
            color,
            uv: uv[i],
            // OverlayTexture.NO_OVERLAY uses integer texels, not atlas UVs.
            overlay_uv: [0., 10.],
            overlay_tint: [1.; 3],
            alpha,
            texture_layer,
            light,
            normal,
        });
    }
    out.indices[layer as usize].extend([first, first + 1, first + 2, first + 2, first + 3, first]);
    Ok(())
}
pub(super) fn append_baked(
    model: &BakedModel,
    matrix: Mat4,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    for q in &model.quads {
        let color = if let TintKind::Constant(color) = q.tint {
            color
        } else {
            [1.; 3]
        };
        emit(
            q.positions
                .map(|p| matrix.transform_point3(Vec3::from_array(p)).to_array()),
            q.uv,
            color,
            q.texture_layer,
            match q.layer {
                RenderLayer::Opaque | RenderLayer::Cutout => EntityMaterial::ItemCutout,
                RenderLayer::Translucent => EntityMaterial::ItemTranslucent,
            },
            q.alpha,
            light,
            out,
        )?;
    }
    Ok(())
}
pub(super) fn item_matrix(model: &BakedModel, context: &str) -> Mat4 {
    crate::ui::items::display_matrix(model, context)
}
/// Source `ItemInHandLayer` pose from an arm (or head) bone transform: rotate X -90 and
/// Y 180, then translate (+-1/16, 0.125, -0.625). Folded through the two rotations
/// that is (-+1/16, 0.625, -0.125) in the bone frame; heads take no sideways shift.
pub(super) fn held_item_attachment(bone: Mat4, slot: usize) -> Mat4 {
    let side = match slot {
        0 => -1.,
        1 => 1.,
        _ => 0.,
    } / 16.;
    bone * Mat4::from_translation(Vec3::new(side, 0.625, -0.125))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
        * Mat4::from_rotation_y(std::f32::consts::PI)
}
/// Source `CrossedArmsItemLayer.applyTranslation`: from the arms bone, rotate X 0.75 rad,
/// scale 1.07, translate (0, 0.13, -0.34), then flip X by pi.
pub(super) fn crossed_arms_attachment(arms: Mat4) -> Mat4 {
    arms * Mat4::from_rotation_x(0.75)
        * Mat4::from_scale(Vec3::splat(1.07))
        * Mat4::from_translation(Vec3::new(0., 0.13, -0.34))
        * Mat4::from_rotation_x(std::f32::consts::PI)
}
/// Source `WitchItemLayer.applyTranslation` for a held potion: from the nose bone, translate
/// (1/16, 0.25, 0), then Z 180, X 140, Z 10 and X 180 degrees.
pub(super) fn witch_potion_attachment(nose: Mat4) -> Mat4 {
    nose * Mat4::from_translation(Vec3::new(0.0625, 0.25, 0.))
        * Mat4::from_rotation_z(std::f32::consts::PI)
        * Mat4::from_rotation_x(140f32.to_radians())
        * Mat4::from_rotation_z(10f32.to_radians())
        * Mat4::from_rotation_x(std::f32::consts::PI)
}
pub(super) fn append_equipment(
    e: &Entity,
    rig: &Rig,
    transforms: &[Mat4],
    items: &mut crate::ui::items::ItemModels,
    equipment: &mut super::equipment::Cache,
    root: Mat4,
    content: &Content,
    models: &Models,
    tick: u64,
    partial: f32,
    light: [f32; 2],
    out: &mut DynamicGeometry,
    stats: &mut PresentationStats,
) -> Result<()> {
    for slot in [4, 3, 2, 5, 0, 1, 6, 7] {
        let Some(stack) = e.equipment_value(slot) else {
            continue;
        };
        match equipment.append(
            e, slot, stack, models, content, root, tick, partial, light, out,
        ) {
            Ok(true) => continue,
            Ok(false) => {}
            Err(reason) => {
                stats
                    .unrendered
                    .insert(format!("{}:equipment:{slot}", e.type_name), reason);
                continue;
            }
        }
        // Villager-family rigs hold only the main-hand item, across the crossed arms (or on
        // the witch's nose for a potion), displayed as a ground item (`HoldingEntityRenderState`).
        if rig.bones.iter().any(|b| b.role == BoneRole::CrossedArms) && slot < 2 {
            if slot == 0 {
                let potion = e.type_name == "minecraft:witch"
                    && content.item_names.get(&stack.id).map(String::as_str)
                        == Some("minecraft:potion");
                let role = if potion {
                    BoneRole::Nose
                } else {
                    BoneRole::CrossedArms
                };
                let Some(bone) = rig
                    .bones
                    .iter()
                    .zip(transforms)
                    .find_map(|(b, m)| (b.role == role).then_some(*m))
                else {
                    stats.unrendered.insert(
                        format!("{}:equipment:0", e.type_name),
                        "rig has no matching equipment attachment".into(),
                    );
                    continue;
                };
                let mut state = crate::ui::items::ItemContext::default();
                state.display = "ground".into();
                match items.model_with(content, stack, &state) {
                    Ok(model) => {
                        let attachment = if potion {
                            witch_potion_attachment(bone)
                        } else {
                            crossed_arms_attachment(bone)
                        };
                        append_baked(model, attachment * item_matrix(model, "ground"), light, out)?;
                    }
                    Err(reason) => {
                        stats
                            .unrendered
                            .insert(format!("{}:equipment:0", e.type_name), reason);
                    }
                }
            }
            continue;
        }
        let target = match slot {
            0 => BoneRole::RightArm,
            1 => BoneRole::LeftArm,
            5 => BoneRole::Head,
            _ => {
                stats.unrendered.insert(
                    format!("{}:equipment:{slot}", e.type_name),
                    "armor/body/saddle layer requires item equippable model attachment".into(),
                );
                continue;
            }
        };
        let Some((bone, matrix)) = rig
            .bones
            .iter()
            .zip(transforms)
            .find(|(b, _)| b.role == target)
        else {
            stats.unrendered.insert(
                format!("{}:equipment:{slot}", e.type_name),
                "rig has no matching equipment attachment".into(),
            );
            continue;
        };
        let _ = bone;
        let context = if slot == 0 {
            "thirdperson_righthand"
        } else if slot == 1 {
            "thirdperson_lefthand"
        } else {
            "head"
        };
        let mut state = crate::ui::items::ItemContext::default();
        state.display = context.into();
        let flags = number(models, e, "DATA_LIVING_ENTITY_FLAGS").unwrap_or(0);
        state.using_item = flags & 1 != 0 && ((flags & 2 != 0) == (slot == 1));
        state.use_ticks = tick.saturating_sub(e.observed_tick) as f32;
        match items.model_with(content, stack, &state) {
            Ok(model) => {
                let attachment = held_item_attachment(*matrix, slot);
                append_baked(model, attachment * item_matrix(model, context), light, out)?;
            }
            Err(reason) => {
                stats
                    .unrendered
                    .insert(format!("{}:equipment:{slot}", e.type_name), reason);
            }
        }
    }
    Ok(())
}
fn append_item_entity(
    e: &Entity,
    models: &Models,
    items: &mut crate::ui::items::ItemModels,
    content: &Content,
    origin: [f64; 3],
    tick: u64,
    partial: f32,
    light: [f32; 2],
    out: &mut DynamicGeometry,
    stats: &mut PresentationStats,
) -> Result<bool> {
    let accessor = match e.type_name.as_str() {
        "minecraft:item" => "DATA_ITEM",
        "minecraft:item_display" => "DATA_ITEM_STACK_ID",
        _ => return Ok(false),
    };
    let Some(Metadata::Item(stack)) = value(models, e, accessor) else {
        return Ok(false);
    };
    if stack.empty() {
        return Ok(true);
    }
    let context = if e.type_name == "minecraft:item" {
        "ground"
    } else {
        "fixed"
    };
    let t = e.presented_transform(tick, partial);
    let mut pos = Vec3::new(
        (t.position[0] - origin[0]) as f32,
        (t.position[1] - origin[1]) as f32,
        (t.position[2] - origin[2]) as f32,
    );
    let age = tick as f32 + partial;
    if context == "ground" {
        pos.y += (age / 10.).sin() * 0.1 + 0.1;
    }
    match items.model(content, stack, context) {
        Ok(model) => {
            let matrix = Mat4::from_translation(pos)
                * Mat4::from_rotation_y(if context == "ground" {
                    age / 20.
                } else {
                    t.yaw.to_radians()
                });
            append_baked(model, matrix * item_matrix(model, context), light, out)?;
        }
        Err(e) => {
            stats.unrendered.insert(e.clone(), e);
        }
    }
    Ok(true)
}
pub fn special_item(
    models: &Models,
    content: &Content,
    _stack: &Stack,
    special: &Value,
) -> Result<BakedModel> {
    let kind = special["type"]
        .as_str()
        .ok_or("special item missing type")?
        .trim_start_matches("minecraft:");
    let (rig, texture_name) = match kind {
        "shield" => ("minecraft:shield#main", "entity/shield_base".to_owned()),
        "trident" => ("minecraft:trident#main", "entity/trident".to_owned()),
        "chest" => (
            "minecraft:chest#main",
            format!(
                "entity/chest/{}",
                special["texture"]
                    .as_str()
                    .unwrap_or("normal")
                    .trim_start_matches("minecraft:")
            ),
        ),
        "shulker_box" => (
            "minecraft:shulker_box#main",
            format!(
                "entity/shulker/{}",
                special["texture"]
                    .as_str()
                    .unwrap_or("shulker")
                    .trim_start_matches("minecraft:")
            ),
        ),
        "conduit" => (
            "minecraft:conduit_shell#main",
            "entity/conduit/base".to_owned(),
        ),
        "decorated_pot" => (
            "minecraft:decorated_pot_base#main",
            "entity/decorated_pot/decorated_pot_base".to_owned(),
        ),
        "banner" => (
            "minecraft:standing_banner#main",
            "entity/banner_base".to_owned(),
        ),
        "bed" => (
            "minecraft:bed_head#main",
            format!(
                "entity/bed/{}",
                special["texture"]
                    .as_str()
                    .unwrap_or("red")
                    .trim_start_matches("minecraft:")
            ),
        ),
        "head" | "player_head" => {
            let kind = special["kind"].as_str().unwrap_or("player");
            match kind {
                "skeleton" => (
                    "minecraft:skeleton_skull#main",
                    "entity/skeleton/skeleton".to_owned(),
                ),
                "wither_skeleton" => (
                    "minecraft:wither_skeleton_skull#main",
                    "entity/skeleton/wither_skeleton".to_owned(),
                ),
                "zombie" => (
                    "minecraft:zombie_head#main",
                    "entity/zombie/zombie".to_owned(),
                ),
                "creeper" => (
                    "minecraft:creeper_head#main",
                    "entity/creeper/creeper".to_owned(),
                ),
                "piglin" => (
                    "minecraft:piglin_head#main",
                    "entity/piglin/piglin".to_owned(),
                ),
                "dragon" => (
                    "minecraft:dragon_skull#main",
                    "entity/enderdragon/dragon".to_owned(),
                ),
                "player" => (
                    "minecraft:player_head#main",
                    "entity/player/wide/steve".to_owned(),
                ),
                _ => return Err(format!("unknown special head family {kind}")),
            }
        }
        "copper_golem_statue" => (
            "minecraft:copper_golem#main",
            format!(
                "entity/copper_golem/{}",
                special["texture"]
                    .as_str()
                    .unwrap_or("copper_golem")
                    .trim_start_matches("minecraft:")
            ),
        ),
        _ => return Err(format!("unknown special item render family {kind}")),
    };
    // Solaris-authored rigs live in the embedded catalog, not in `models.rigs`; `Models::rig`
    // resolves both. A key with neither renders the placeholder box (logged once) rather than
    // failing the GUI/hand item bake and with it the session.
    if !models.has_authored_rig(rig) {
        static REPORTED: std::sync::Mutex<BTreeSet<String>> = std::sync::Mutex::new(BTreeSet::new());
        if REPORTED
            .lock()
            .map_or(true, |mut seen| seen.insert(rig.to_owned()))
        {
            eprintln!("special model rig not activated: {rig}; using placeholder");
        }
    }
    let rig = models.rig(rig);
    let tex = texture(content, &texture_name)?;
    let mut model = BakedModel::default();
    let mut matrices = Vec::with_capacity(rig.bones.len());
    for bone in &rig.bones {
        let matrix = bone.parent.map(|i| matrices[i]).unwrap_or(Mat4::IDENTITY)
            * resources::transform(bone.pivot, bone.rotation, bone.scale);
        matrices.push(matrix);
        for q in &bone.quads {
            model.quads.push(BakedQuad {
                positions: q.map(|v| {
                    matrix
                        .transform_point3(Vec3::new(v[0], v[1], v[2]))
                        .to_array()
                }),
                uv: q.map(|v| {
                    [
                        tex.uv[0] + v[3] * (tex.uv[2] - tex.uv[0]),
                        tex.uv[1] + v[4] * (tex.uv[3] - tex.uv[1]),
                    ]
                }),
                tint: TintKind::None,
                tint_index: None,
                cullface: None,
                face: 1,
                shade: true,
                ambient_occlusion: false,
                layer: RenderLayer::Cutout,
                alpha: 1.,
                texture_layer: tex.layer,
            });
        }
    }
    Ok(model)
}
