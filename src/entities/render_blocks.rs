//! Original 26.1.2 block-entity factory presentation. Received NBT selects content;
//! the shared block tick owner supplies animation and collision progress.
#[path = "render_blocks/cache.rs"]
mod cache;
pub(crate) use cache::Cache;
#[path = "render_blocks/items.rs"]
mod items;
#[path = "render_blocks/math.rs"]
pub(crate) mod math;
#[path = "render_blocks/signals.rs"]
pub(crate) mod signals;
#[path = "render_blocks/signs.rs"]
mod signs;
#[path = "render_blocks/state.rs"]
pub(crate) mod state;
#[path = "render_blocks/surfaces.rs"]
mod surfaces;
#[cfg(test)]
#[path = "render_blocks/village_tests.rs"]
mod village_tests;
use super::{
    blocks,
    presentation::{emit_rig, texture},
    resources::{self, Rig},
    BlockEntity, Models,
};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{
        content::{Content, TextureRegion},
        world::World,
        Result,
    },
    ui::render::TextRenderer,
};
use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use std::f32::consts::{FRAC_PI_2, PI};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BlockRenderStatus {
    Rendered,
    SourceNoFactory,
    SourceHidden,
    ProfilePending,
    WorldPending,
}
pub(crate) type ItemPainter<'a> =
    dyn FnMut(&Value, &str, i32, Mat4, [f32; 2], &mut DynamicGeometry) -> Result<()> + 'a;
pub(crate) type EntityPainter<'a> =
    dyn FnMut(&Value, Mat4, [f32; 2], &mut DynamicGeometry) -> Result<BlockRenderStatus> + 'a;
pub(crate) type MovingBlockPainter<'a> = dyn FnMut(u32, [i32; 3], Mat4, [f32; 2], &mut DynamicGeometry) -> Result<BlockRenderStatus>
    + 'a;
pub(crate) struct BlockFrame<'a, 'paint> {
    pub origin: [f64; 3],
    pub camera: [f64; 3],
    pub camera_rotation: [f32; 2],
    pub game_time: i64,
    pub partial: f32,
    pub light: [f32; 2],
    pub world: &'a World,
    pub blocks: &'a std::collections::BTreeMap<[i32; 3], BlockEntity>,
    pub text: Option<&'a TextRenderer>,
    pub cache: &'a mut Cache,
    pub skin: &'a dyn Fn(&Value) -> Result<Option<TextureRegion>>,
    pub item: &'a mut ItemPainter<'paint>,
    pub entity: &'a mut EntityPainter<'paint>,
    pub moving_block: &'a mut MovingBlockPainter<'paint>,
    pub sample_light: &'a dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    pub game_master: bool,
    pub spectator: bool,
    pub scoping: bool,
    pub text_filtering: bool,
    pub christmas: bool,
}
/// Geometry-only factories need no item/entity painters.
struct StaticBlockFrame<'a> {
    world: &'a World,
    partial: f32,
    light: [f32; 2],
    blocks: &'a std::collections::BTreeMap<[i32; 3], BlockEntity>,
    sample_light: &'a dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    christmas: bool,
}
impl<'a> From<&BlockFrame<'a, '_>> for StaticBlockFrame<'a> {
    fn from(frame: &BlockFrame<'a, '_>) -> Self {
        Self {
            world: frame.world,
            partial: frame.partial,
            light: frame.light,
            blocks: frame.blocks,
            sample_light: frame.sample_light,
            christmas: frame.christmas,
        }
    }
}
pub(crate) struct BlockPresentation {
    pub origin: [f64; 3],
    pub camera: [f64; 3],
    pub camera_rotation: [f32; 2],
    pub radius: f32,
    pub game_time: i64,
    pub partial: f32,
    pub game_master: bool,
    pub spectator: bool,
    pub scoping: bool,
    pub text_filtering: bool,
    pub christmas: bool,
}
impl super::Store {
    pub(crate) fn tick_block_sources(
        &mut self,
        world: &World,
        content: &Content,
        game_time: i64,
        players: &[[f64; 3]],
        frozen: bool,
    ) -> Result<()> {
        if frozen {
            return Ok(());
        }
        for block in self.block_entities.values_mut() {
            state::tick(block, world, content, game_time, players)?;
        }
        Ok(())
    }
    pub(crate) fn append_block_geometry<'a>(
        &mut self,
        content: &Content,
        world: &World,
        options: &BlockPresentation,
        sample_light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
        skin: &dyn Fn(&Value) -> Result<Option<TextureRegion>>,
        entity: &mut EntityPainter<'a>,
        moving_block: &mut MovingBlockPainter<'a>,
        out: &mut DynamicGeometry,
    ) -> Result<super::PresentationStats> {
        if !options.radius.is_finite() || !(0. ..=4096.).contains(&options.radius) {
            return Err("block presentation radius bound".into());
        }
        let models = self
            .models
            .as_ref()
            .ok_or("entity model cache not activated")?;
        let mut stats = super::PresentationStats {
            state_revision: self.revision.to_string(),
            ..Default::default()
        };
        let initial = out.vertices.len();
        let mut item = |observed: &Value,
                        context: &str,
                        seed: i32,
                        root: Mat4,
                        light: [f32; 2],
                        out: &mut DynamicGeometry| {
            super::render_special::append_observed_item(
                content,
                observed,
                context,
                seed,
                root,
                light,
                &mut self.items,
                &mut self.special_render_cache,
                out,
            )
        };
        for block in self.block_entities.values() {
            let distance = (0..3)
                .map(|i| (f64::from(block.position[i]) + 0.5 - options.camera[i]).powi(2))
                .sum::<f64>();
            if distance > f64::from(options.radius).powi(2) {
                continue;
            }
            let Some(light) = super::presentation::sampled(
                sample_light,
                block.position.map(|v| f64::from(v) + 0.5),
            )?
            else {
                stats.lighting_pending += 1;
                continue;
            };
            let mut frame = BlockFrame {
                origin: options.origin,
                camera: options.camera,
                camera_rotation: options.camera_rotation,
                game_time: options.game_time,
                partial: options.partial,
                light,
                world,
                blocks: &self.block_entities,
                text: self.text.as_deref(),
                cache: &mut self.block_render_cache,
                skin,
                item: &mut item,
                entity,
                moving_block,
                sample_light,
                game_master: options.game_master,
                spectator: options.spectator,
                scoping: options.scoping,
                text_filtering: options.text_filtering,
                christmas: options.christmas,
            };
            match append_block_entity(
                block,
                models,
                content,
                &mut frame,
                &mut self.transforms,
                out,
            ) {
                Ok(BlockRenderStatus::Rendered) => stats.block_entities += 1,
                Ok(BlockRenderStatus::ProfilePending) => stats.profile_textures_pending += 1,
                Ok(BlockRenderStatus::WorldPending) => stats.lighting_pending += 1,
                Ok(BlockRenderStatus::SourceNoFactory | BlockRenderStatus::SourceHidden) => {}
                Err(reason) => {
                    stats.unrendered.insert(block.type_name.clone(), reason);
                }
            }
        }
        stats.vertices = out.vertices.len() - initial;
        Ok(stats)
    }
    /// Native scene factories whose geometry depends only on received block state.
    /// Other factories retain their real item/entity painters in the full pass.
    pub(crate) fn append_static_block_geometry(
        &mut self,
        content: &Content,
        world: &World,
        origin: [f64; 3],
        camera: [f64; 3],
        radius: f32,
        scoping: bool,
        text_filtering: bool,
        partial: f32,
        sample_light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
        out: &mut DynamicGeometry,
    ) -> Result<super::PresentationStats> {
        if !radius.is_finite() || !(0. ..=4096.).contains(&radius) {
            return Err("block presentation radius bound".into());
        }
        let models = self
            .models
            .as_ref()
            .ok_or("entity model cache not activated")?;
        let mut stats = super::PresentationStats {
            state_revision: self.revision.to_string(),
            ..Default::default()
        };
        let initial = out.vertices.len();
        let radius_squared = f64::from(radius).powi(2);
        for block in self.block_entities.values().filter(|block| {
            matches!(
                block.type_name.as_str(),
                "minecraft:sign"
                    | "minecraft:hanging_sign"
                    | "minecraft:chest"
                    | "minecraft:trapped_chest"
                    | "minecraft:ender_chest"
                    | "minecraft:bed"
                    | "minecraft:bell"
                    | "minecraft:lectern"
            )
        }) {
            let distance = (0..3)
                .map(|i| (f64::from(block.position[i]) + 0.5 - camera[i]).powi(2))
                .sum::<f64>();
            if distance > radius_squared {
                continue;
            }
            let Some(light) = super::presentation::sampled(
                sample_light,
                block.position.map(|v| f64::from(v) + 0.5),
            )?
            else {
                stats.lighting_pending += 1;
                continue;
            };
            let root = relative_position(block, origin);
            let frame = StaticBlockFrame {
                world,
                partial,
                light,
                blocks: &self.block_entities,
                sample_light,
                christmas: false,
            };
            let vertices = out.vertices.len();
            let indices = out.indices.each_ref().map(Vec::len);
            let result = match block.type_name.as_str() {
                "minecraft:sign" | "minecraft:hanging_sign" => signs::append(
                    block,
                    models,
                    content,
                    &mut signs::Frame {
                        root,
                        camera,
                        light,
                        text: self.text.as_deref(),
                        cache: &mut self.block_render_cache,
                        scoping,
                        text_filtering,
                    },
                    &mut self.transforms,
                    out,
                )
                .map(|()| BlockRenderStatus::Rendered),
                "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:ender_chest" => chest(
                    block,
                    models,
                    content,
                    &frame,
                    root,
                    &mut self.transforms,
                    out,
                ),
                "minecraft:bed" => bed(
                    block,
                    models,
                    content,
                    &frame,
                    root,
                    &mut self.transforms,
                    out,
                ),
                "minecraft:bell" => bell(
                    block,
                    models,
                    content,
                    &frame,
                    root,
                    &mut self.transforms,
                    out,
                ),
                "minecraft:lectern" => book(
                    block,
                    models,
                    content,
                    &frame,
                    root,
                    &mut self.transforms,
                    out,
                ),
                _ => unreachable!(),
            };
            match result {
                Ok(BlockRenderStatus::Rendered) => stats.block_entities += 1,
                Ok(BlockRenderStatus::WorldPending) => stats.lighting_pending += 1,
                Ok(_) => {}
                Err(reason) => {
                    out.vertices.truncate(vertices);
                    for (list, length) in out.indices.iter_mut().zip(indices) {
                        list.truncate(length);
                    }
                    stats.unrendered.insert(block.type_name.clone(), reason);
                }
            }
        }
        stats.vertices = out.vertices.len() - initial;
        Ok(stats)
    }
}
// Enumerated from BlockEntityRenderers registration, not the entity type registry.
pub(crate) fn has_factory(name: &str) -> bool {
    matches!(
        name,
        "minecraft:sign"
            | "minecraft:hanging_sign"
            | "minecraft:mob_spawner"
            | "minecraft:piston"
            | "minecraft:chest"
            | "minecraft:ender_chest"
            | "minecraft:trapped_chest"
            | "minecraft:enchanting_table"
            | "minecraft:lectern"
            | "minecraft:end_portal"
            | "minecraft:end_gateway"
            | "minecraft:beacon"
            | "minecraft:skull"
            | "minecraft:banner"
            | "minecraft:structure_block"
            | "minecraft:test_instance_block"
            | "minecraft:shulker_box"
            | "minecraft:bed"
            | "minecraft:conduit"
            | "minecraft:bell"
            | "minecraft:campfire"
            | "minecraft:brushable_block"
            | "minecraft:decorated_pot"
            | "minecraft:trial_spawner"
            | "minecraft:vault"
            | "minecraft:copper_golem_statue"
            | "minecraft:shelf"
    )
}
pub(crate) fn source_no_factory(name: &str) -> bool {
    // Reflected against the pinned original PROVIDERS map and complete registry.
    matches!(
        name,
        "minecraft:furnace"
            | "minecraft:jukebox"
            | "minecraft:dispenser"
            | "minecraft:dropper"
            | "minecraft:creaking_heart"
            | "minecraft:brewing_stand"
            | "minecraft:daylight_detector"
            | "minecraft:hopper"
            | "minecraft:comparator"
            | "minecraft:command_block"
            | "minecraft:barrel"
            | "minecraft:smoker"
            | "minecraft:blast_furnace"
            | "minecraft:jigsaw"
            | "minecraft:beehive"
            | "minecraft:sculk_sensor"
            | "minecraft:calibrated_sculk_sensor"
            | "minecraft:sculk_catalyst"
            | "minecraft:sculk_shrieker"
            | "minecraft:chiseled_bookshelf"
            | "minecraft:crafter"
            | "minecraft:test_block"
    )
}
fn prop<'a>(e: &BlockEntity, c: &'a Content, key: &str) -> Option<&'a str> {
    e.state
        .and_then(|s| c.state_properties(s))
        .and_then(|p| p.get(key))
        .map(String::as_str)
}
fn block_name<'a>(e: &BlockEntity, c: &'a Content) -> Result<&'a str> {
    e.state
        .and_then(|s| c.name(s))
        .ok_or("block entity block state not observed".into())
}
fn yaw(facing: &str) -> f32 {
    match facing {
        "south" => 0.,
        "west" => 90.,
        "north" => 180.,
        "east" => 270.,
        _ => 0.,
    }
}
fn base(e: &BlockEntity, f: &BlockFrame) -> Mat4 {
    relative_position(e, f.origin)
}
fn relative_position(e: &BlockEntity, origin: [f64; 3]) -> Mat4 {
    Mat4::from_translation(Vec3::from_array(std::array::from_fn(|i| {
        (f64::from(e.position[i]) - origin[i]) as f32
    })))
}
fn around(point: Vec3, rotation: Mat4) -> Mat4 {
    Mat4::from_translation(point) * rotation * Mat4::from_translation(-point)
}
fn rig<'a>(m: &'a Models, key: &str) -> Result<&'a Rig> {
    m.rigs
        .get(key)
        .ok_or_else(|| format!("source block entity rig not activated: {key}"))
}
fn paint_rig(
    r: &Rig,
    root: Mat4,
    tex: TextureRegion,
    color: [f32; 3],
    layer: EntityMaterial,
    light: [f32; 2],
    transforms: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
    mut pose: impl FnMut(&str, &mut Vec3, &mut Vec3),
) -> Result<()> {
    transforms.clear();
    transforms.reserve(r.bones.len());
    for bone in &r.bones {
        let mut p = bone.pivot;
        let mut rotation = bone.rotation;
        pose(&bone.name, &mut p, &mut rotation);
        let parent = bone.parent.map(|i| transforms[i]).unwrap_or(root);
        transforms.push(parent * resources::transform(p, rotation, bone.scale));
    }
    emit_rig(r, transforms, tex, color, layer, 1., light, out)
}
fn rgb(color: u32) -> [f32; 3] {
    [
        (color >> 16 & 255) as f32 / 255.,
        (color >> 8 & 255) as f32 / 255.,
        (color & 255) as f32 / 255.,
    ]
}
pub(crate) fn dye(name: &str) -> (u32, u32) {
    match name {
        "white" => (0xf9fffe, 0xffffff),
        "orange" => (16351261, 16738335),
        "magenta" => (13061821, 0xff00ff),
        "light_blue" => (3847130, 10141901),
        "yellow" => (16701501, 0xffff00),
        "lime" => (8439583, 0xbfff00),
        "pink" => (15961002, 16738740),
        "gray" => (4673362, 0x808080),
        "light_gray" => (0x9d9d97, 0xd3d3d3),
        "cyan" => (1481884, 65535),
        "purple" => (8991416, 10494192),
        "blue" => (3949738, 255),
        "brown" => (8606770, 9127187),
        "green" => (6192150, 65280),
        "red" => (11546150, 0xff0000),
        _ => (0x1d1d21, 0),
    }
}
pub(crate) fn append_block_entity(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &mut BlockFrame,
    transforms: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let vertices = out.vertices.len();
    let indices = out.indices.each_ref().map(Vec::len);
    let result = append_inner(e, m, c, f, transforms, out);
    if !matches!(&result, Ok(BlockRenderStatus::Rendered)) {
        out.vertices.truncate(vertices);
        for (list, length) in out.indices.iter_mut().zip(indices) {
            list.truncate(length);
        }
    }
    result
}
fn append_inner(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &mut BlockFrame,
    transforms: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    if !has_factory(&e.type_name) {
        if source_no_factory(&e.type_name) {
            return Ok(BlockRenderStatus::SourceNoFactory);
        }
        return Err(format!(
            "unknown block entity factory type: {}",
            e.type_name
        ));
    }
    if e.animation.source.pending_world {
        return Ok(BlockRenderStatus::WorldPending);
    }
    let root = base(e, f);
    match e.type_name.as_str() {
        "minecraft:sign" | "minecraft:hanging_sign" => signs::append(
            e,
            m,
            c,
            &mut signs::Frame {
                root,
                camera: f.camera,
                light: f.light,
                text: f.text,
                cache: f.cache,
                scoping: f.scoping,
                text_filtering: f.text_filtering,
            },
            transforms,
            out,
        )
        .map(|()| BlockRenderStatus::Rendered),
        "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:ender_chest" => {
            chest(e, m, c, &StaticBlockFrame::from(&*f), root, transforms, out)
        }
        "minecraft:shulker_box" => shulker(e, m, c, f, root, transforms, out),
        "minecraft:skull" => skull(e, m, c, f, root, transforms, out),
        "minecraft:enchanting_table" | "minecraft:lectern" => {
            book(e, m, c, &StaticBlockFrame::from(&*f), root, transforms, out)
        }
        "minecraft:conduit" => conduit(e, m, c, f, root, transforms, out),
        "minecraft:banner" => surfaces::banner(e, m, c, f, root, transforms, out),
        "minecraft:decorated_pot" => surfaces::pot(e, m, c, f, root, transforms, out),
        "minecraft:beacon"
        | "minecraft:end_portal"
        | "minecraft:end_gateway"
        | "minecraft:structure_block"
        | "minecraft:test_instance_block" => surfaces::append(e, c, f, root, out),
        "minecraft:campfire"
        | "minecraft:brushable_block"
        | "minecraft:vault"
        | "minecraft:mob_spawner"
        | "minecraft:trial_spawner"
        | "minecraft:shelf" => items::append(e, m, c, f, root, out),
        "minecraft:piston" => piston(e, c, f, root, out),
        "minecraft:bed" => bed(e, m, c, &StaticBlockFrame::from(&*f), root, transforms, out),
        "minecraft:bell" => bell(e, m, c, &StaticBlockFrame::from(&*f), root, transforms, out),
        "minecraft:copper_golem_statue" => {
            let pose = prop(e, c, "pose").unwrap_or("standing");
            let key = match pose {
                "standing" => "minecraft:copper_golem#main",
                "running" => "minecraft:copper_golem_running#main",
                "sitting" => "minecraft:copper_golem_sitting#main",
                "star" => "minecraft:copper_golem_star#main",
                _ => return Err("invalid copper statue pose".into()),
            };
            let name = block_name(e, c)?;
            let suffix = if name.contains("oxidized") {
                "_oxidized"
            } else if name.contains("weathered") {
                "_weathered"
            } else if name.contains("exposed") {
                "_exposed"
            } else {
                ""
            };
            let root = root
                * Mat4::from_translation(Vec3::new(0.5, 0., 0.5))
                * Mat4::from_rotation_y(
                    (180. - yaw(prop(e, c, "facing").unwrap_or("north"))).to_radians(),
                );
            paint_rig(
                rig(m, key)?,
                root,
                texture(c, &format!("entity/copper_golem/copper_golem{suffix}"))?,
                [1.; 3],
                EntityMaterial::EntityCutout,
                f.light,
                transforms,
                out,
                |name, pivot, rotation| {
                    if name == "root" {
                        pivot.y = 0.;
                        rotation.z = PI;
                    }
                },
            )?;
            Ok(BlockRenderStatus::Rendered)
        }
        _ => unreachable!(),
    }
}
fn bell(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &StaticBlockFrame,
    root: Mat4,
    transforms: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let s = &e.animation.source;
    let angle = s
        .bell_direction
        .map(|d| math::bell_angles(s.bell_ticks as f32 + f.partial, d))
        .unwrap_or([0.; 2]);
    paint_rig(
        rig(m, "minecraft:bell#main")?,
        root,
        texture(c, "entity/bell/bell_body")?,
        [1.; 3],
        EntityMaterial::EntitySolid,
        f.light,
        transforms,
        out,
        |name, _, rotation| {
            if name == "bell_body" {
                rotation.x = angle[0];
                rotation.z = angle[1];
            }
        },
    )?;
    Ok(BlockRenderStatus::Rendered)
}
fn chest(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &StaticBlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let part = prop(e, c, "type").unwrap_or("single");
    let key = match part {
        "single" => "minecraft:chest#main",
        "left" => "minecraft:double_chest_left#main",
        "right" => "minecraft:double_chest_right#main",
        _ => return Err("invalid chest type".into()),
    };
    let name = block_name(e, c)?;
    let material = if name.contains("copper_chest") {
        if name.contains("oxidized") {
            "copper_oxidized"
        } else if name.contains("weathered") {
            "copper_weathered"
        } else if name.contains("exposed") {
            "copper_exposed"
        } else {
            "copper"
        }
    } else if e.type_name == "minecraft:ender_chest" {
        "ender"
    } else if f.christmas {
        "christmas"
    } else if e.type_name == "minecraft:trapped_chest" {
        "trapped"
    } else {
        "normal"
    };
    let suffix = match part {
        "left" => "_left",
        "right" => "_right",
        _ => "",
    };
    let mut progress = e.progress(f.partial);
    let mut light = f.light;
    if part != "single" {
        let facing = prop(e, c, "facing").unwrap_or("north");
        let connected = match (facing, part) {
            ("north", "left") | ("south", "right") => [1, 0, 0],
            ("south", "left") | ("north", "right") => [-1, 0, 0],
            ("east", "left") | ("west", "right") => [0, 0, 1],
            _ => [0, 0, -1],
        };
        let position = std::array::from_fn(|i| e.position[i] + connected[i]);
        if let Some(neighbor) = f.blocks.get(&position) {
            if prop(neighbor, c, "facing") == Some(facing)
                && prop(neighbor, c, "type") == Some(if part == "left" { "right" } else { "left" })
                && block_name(neighbor, c)? == name
            {
                progress = progress.max(neighbor.progress(f.partial));
                let Some(other) = (f.sample_light)(position.map(|v| f64::from(v) + 0.5))? else {
                    return Ok(BlockRenderStatus::WorldPending);
                };
                light = std::array::from_fn(|i| light[i].max(other[i]));
            }
        }
    }
    let angle = math::lid_angle(progress);
    let root = root
        * around(
            Vec3::new(0.5, 0., 0.5),
            Mat4::from_rotation_y(-yaw(prop(e, c, "facing").unwrap_or("north")).to_radians()),
        );
    paint_rig(
        rig(m, key)?,
        root,
        texture(c, &format!("entity/chest/{material}{suffix}"))?,
        [1.; 3],
        EntityMaterial::EntityCutoutCull,
        light,
        t,
        out,
        |name, _, r| {
            if name == "lid" || name == "lock" {
                r.x = angle;
            }
        },
    )?;
    Ok(BlockRenderStatus::Rendered)
}
fn shulker(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &mut BlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let direction = blocks::facing(e, c)?.map(|v| v as f32);
    let rotation = match direction {
        [0., 1., 0.] => Quat::IDENTITY,
        [0., -1., 0.] => Quat::from_rotation_x(PI),
        [0., 0., -1.] => Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_z(PI),
        [0., 0., 1.] => Quat::from_rotation_x(FRAC_PI_2),
        [1., 0., 0.] => Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_z(-FRAC_PI_2),
        [-1., 0., 0.] => Quat::from_rotation_x(FRAC_PI_2) * Quat::from_rotation_z(FRAC_PI_2),
        _ => return Err("invalid shulker direction".into()),
    };
    let name = block_name(e, c)?.trim_start_matches("minecraft:");
    let material = if name == "shulker_box" {
        "entity/shulker/shulker".to_owned()
    } else {
        format!(
            "entity/shulker/shulker_{}",
            name.trim_end_matches("_shulker_box")
        )
    };
    let root = root
        * Mat4::from_translation(Vec3::splat(0.5))
        * Mat4::from_scale(Vec3::splat(0.9995))
        * Mat4::from_quat(rotation)
        * Mat4::from_scale(Vec3::new(1., -1., -1.))
        * Mat4::from_translation(Vec3::new(0., -1., 0.));
    let progress = e.progress(f.partial);
    paint_rig(
        rig(m, "minecraft:shulker_box#main")?,
        root,
        texture(c, &material)?,
        [1.; 3],
        EntityMaterial::EntityCutout,
        f.light,
        t,
        out,
        |name, p, r| {
            if name == "lid" {
                p.y = 1.5 - progress * 0.5;
                r.y = progress * 1.5 * PI;
            }
        },
    )?;
    Ok(BlockRenderStatus::Rendered)
}
fn skull(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &mut BlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let name = block_name(e, c)?.trim_start_matches("minecraft:");
    let kind = name.replace("_wall", "");
    let (key, material) = match kind.as_str() {
        "skeleton_skull" => ("minecraft:skeleton_skull#main", "entity/skeleton/skeleton"),
        "wither_skeleton_skull" => (
            "minecraft:wither_skeleton_skull#main",
            "entity/skeleton/wither_skeleton",
        ),
        "zombie_head" => ("minecraft:zombie_head#main", "entity/zombie/zombie"),
        "creeper_head" => ("minecraft:creeper_head#main", "entity/creeper/creeper"),
        "dragon_head" => ("minecraft:dragon_skull#main", "entity/enderdragon/dragon"),
        "piglin_head" => ("minecraft:piglin_head#main", "entity/piglin/piglin"),
        "player_head" => ("minecraft:player_head#main", ""),
        _ => return Err("unknown source skull block".into()),
    };
    let (tex, layer) = if kind == "player_head" {
        if !e.data["profile"].is_null() {
            let Some(tex) = (f.skin)(&e.data["profile"])? else {
                return Ok(BlockRenderStatus::ProfilePending);
            };
            (tex, EntityMaterial::EntityTranslucent)
        } else {
            (
                texture(c, &m.default_skin(None)?.texture)?,
                EntityMaterial::EntityCutoutZOffset,
            )
        }
    } else {
        (texture(c, material)?, EntityMaterial::EntityCutoutZOffset)
    };
    let wall = name.contains("_wall_");
    let angle = if wall {
        180. - yaw(prop(e, c, "facing").unwrap_or("north"))
    } else {
        -prop(e, c, "rotation")
            .unwrap_or("0")
            .parse::<f32>()
            .map_err(|_| "skull rotation")?
            * 22.5
    };
    let offset = if wall {
        let face = prop(e, c, "facing").unwrap_or("north");
        let d = match face {
            "north" => [0., -1.],
            "south" => [0., 1.],
            "west" => [-1., 0.],
            "east" => [1., 0.],
            _ => return Err("skull wall facing".into()),
        };
        Vec3::new(0.5 - d[0] * 0.25, 0.25, 0.5 - d[1] * 0.25)
    } else {
        Vec3::new(0.5, 0., 0.5)
    };
    let root = root
        * Mat4::from_translation(offset)
        * Mat4::from_rotation_y(angle.to_radians())
        * Mat4::from_scale(Vec3::new(-1., -1., 1.));
    let s = &e.animation.source;
    let animation = s.skull_ticks as f32 + if s.skull_active { f.partial } else { 0. };
    paint_rig(
        rig(m, key)?,
        root,
        tex,
        [1.; 3],
        layer,
        f.light,
        t,
        out,
        |name, _, r| match name {
            "jaw" => r.x = (f64::from(animation * PI * 0.2).sin() + 1.) as f32 * 0.2,
            "left_ear" => r.z = -(f64::from(animation * PI * 0.2 * 1.2).cos() + 2.5) as f32 * 0.2,
            "right_ear" => r.z = (f64::from(animation * PI * 0.2).cos() + 2.5) as f32 * 0.2,
            _ => {}
        },
    )?;
    Ok(BlockRenderStatus::Rendered)
}
fn book(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &StaticBlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let lectern = e.type_name == "minecraft:lectern";
    if lectern && prop(e, c, "has_book") != Some("true") {
        return Ok(BlockRenderStatus::SourceHidden);
    }
    let b = &e.animation.source.book;
    let time = e.animation.source.ticks as f32 + f.partial;
    let (root, angles) = if lectern {
        (
            root * Mat4::from_translation(Vec3::new(0.5, 1.0625, 0.5))
                * Mat4::from_rotation_y(
                    -(yaw(prop(e, c, "facing").unwrap_or("north")) + 90.).to_radians(),
                )
                * Mat4::from_rotation_z(67.5f32.to_radians())
                * Mat4::from_translation(Vec3::new(0., -0.125, 0.)),
            [1.5, 0.1, 0.9],
        )
    } else {
        let rotation =
            b.previous_rotation + math::wrap_radians(b.rotation - b.previous_rotation) * f.partial;
        (
            root * Mat4::from_translation(Vec3::new(0.5, 0.85 + math::sin(time * 0.1) * 0.01, 0.5))
                * Mat4::from_rotation_y(-rotation)
                * Mat4::from_rotation_z(80f32.to_radians()),
            math::book_angles(
                time,
                math::lerp(b.previous_flip, b.flip, f.partial),
                math::lerp(b.previous_open, b.open, f.partial),
            ),
        )
    };
    paint_rig(
        rig(m, "minecraft:book#main")?,
        root,
        texture(c, "entity/enchantment/enchanting_table_book")?,
        [1.; 3],
        EntityMaterial::EntitySolid,
        f.light,
        t,
        out,
        |name, p, r| {
            match name {
                "left_lid" => r.y = PI + angles[0],
                "right_lid" => r.y = -angles[0],
                "left_pages" => r.y = angles[0],
                "right_pages" => r.y = -angles[0],
                "flip_page1" => r.y = angles[0] * (1. - 2. * angles[1]),
                "flip_page2" => r.y = angles[0] * (1. - 2. * angles[2]),
                _ => {}
            }
            if matches!(
                name,
                "left_pages" | "right_pages" | "flip_page1" | "flip_page2"
            ) {
                p.x = math::sin(angles[0]) / 16.;
            }
        },
    )?;
    Ok(BlockRenderStatus::Rendered)
}
fn conduit(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &mut BlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let s = &e.animation.source;
    let time = s.ticks as f32 + f.partial;
    let rotation = (s.conduit_rotation + if s.conduit_active { f.partial } else { 0. }) * -0.0375;
    if !s.conduit_active {
        paint_rig(
            rig(m, "minecraft:conduit#shell")?,
            root * Mat4::from_translation(Vec3::splat(0.5))
                * Mat4::from_rotation_y(rotation.to_radians()),
            texture(c, "entity/conduit/base")?,
            [1.; 3],
            EntityMaterial::EntitySolid,
            f.light,
            t,
            out,
            |_, _, _| {},
        )?;
    } else {
        let bob = math::sin(time * 0.1) / 2. + 0.5;
        let y = 0.3 + (bob * bob + bob) * 0.2;
        paint_rig(
            rig(m, "minecraft:conduit#cage")?,
            root * Mat4::from_translation(Vec3::new(0.5, y, 0.5))
                * Mat4::from_quat(Quat::from_axis_angle(
                    Vec3::new(0.5, 1., 0.5).normalize(),
                    rotation,
                )),
            texture(c, "entity/conduit/cage")?,
            [1.; 3],
            EntityMaterial::EntityCutout,
            f.light,
            t,
            out,
            |_, _, _| {},
        )?;
        let phase = s.ticks / 66 % 3;
        let rotate = match phase {
            1 => Mat4::from_rotation_x(FRAC_PI_2),
            2 => Mat4::from_rotation_z(FRAC_PI_2),
            _ => Mat4::IDENTITY,
        };
        let wind = texture(
            c,
            if phase == 1 {
                "entity/conduit/wind_vertical"
            } else {
                "entity/conduit/wind"
            },
        )?;
        let windroot = root * Mat4::from_translation(Vec3::splat(0.5));
        paint_rig(
            rig(m, "minecraft:conduit#wind")?,
            windroot * rotate,
            wind,
            [1.; 3],
            EntityMaterial::EntityCutout,
            f.light,
            t,
            out,
            |_, _, _| {},
        )?;
        paint_rig(
            rig(m, "minecraft:conduit#wind")?,
            windroot
                * Mat4::from_scale(Vec3::splat(0.875))
                * Mat4::from_rotation_x(PI)
                * Mat4::from_rotation_z(PI),
            wind,
            [1.; 3],
            EntityMaterial::EntityCutout,
            f.light,
            t,
            out,
            |_, _, _| {},
        )?;
        let billboard = Mat4::from_rotation_y(-f.camera_rotation[0].to_radians())
            * Mat4::from_rotation_x(f.camera_rotation[1].to_radians());
        paint_rig(
            rig(m, "minecraft:conduit#eye")?,
            root * Mat4::from_translation(Vec3::new(0.5, y, 0.5))
                * Mat4::from_scale(Vec3::splat(2. / 3.))
                * billboard
                * Mat4::from_rotation_z(PI)
                * Mat4::from_rotation_y(PI),
            texture(
                c,
                if s.conduit_hunting {
                    "entity/conduit/open_eye"
                } else {
                    "entity/conduit/closed_eye"
                },
            )?,
            [1.; 3],
            EntityMaterial::EntityCutout,
            f.light,
            t,
            out,
            |_, _, _| {},
        )?;
    }
    Ok(BlockRenderStatus::Rendered)
}
fn bed(
    e: &BlockEntity,
    m: &Models,
    c: &Content,
    f: &StaticBlockFrame,
    root: Mat4,
    t: &mut Vec<Mat4>,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let part = prop(e, c, "part").unwrap_or("foot");
    let key = if part == "head" {
        "minecraft:bed_head#main"
    } else {
        "minecraft:bed_foot#main"
    };
    let name = block_name(e, c)?;
    let color = name
        .trim_start_matches("minecraft:")
        .trim_end_matches("_bed");
    let mut light = f.light;
    let d = match prop(e, c, "facing").unwrap_or("north") {
        "north" => [0, 0, -1],
        "south" => [0, 0, 1],
        "west" => [-1, 0, 0],
        _ => [1, 0, 0],
    };
    let position =
        std::array::from_fn(|i| e.position[i] + d[i] * if part == "head" { -1 } else { 1 });
    let other_state = f.world.block(position);
    let other_properties = other_state.and_then(|s| c.state_properties(s));
    if other_state.and_then(|s| c.name(s)) == Some(name)
        && other_properties
            .and_then(|p| p.get("facing"))
            .map(String::as_str)
            == prop(e, c, "facing")
        && other_properties
            .and_then(|p| p.get("part"))
            .map(String::as_str)
            == Some(if part == "head" { "foot" } else { "head" })
        && f.blocks
            .get(&position)
            .is_some_and(|other| other.type_name == "minecraft:bed")
    {
        let Some(other) = (f.sample_light)(position.map(|v| f64::from(v) + 0.5))? else {
            return Ok(BlockRenderStatus::WorldPending);
        };
        light = std::array::from_fn(|i| light[i].max(other[i]));
    }
    let root = root
        * Mat4::from_translation(Vec3::new(0., 0.5625, 0.))
        * Mat4::from_rotation_x(FRAC_PI_2)
        * around(
            Vec3::splat(0.5),
            Mat4::from_rotation_z(
                (180. + yaw(prop(e, c, "facing").unwrap_or("north"))).to_radians(),
            ),
        );
    paint_rig(
        rig(m, key)?,
        root,
        texture(c, &format!("entity/bed/{color}"))?,
        [1.; 3],
        EntityMaterial::EntitySolid,
        light,
        t,
        out,
        |_, _, _| {},
    )?;
    Ok(BlockRenderStatus::Rendered)
}
fn piston(
    e: &BlockEntity,
    c: &Content,
    f: &mut BlockFrame,
    root: Mat4,
    out: &mut DynamicGeometry,
) -> Result<BlockRenderStatus> {
    let p = e
        .animation
        .piston
        .as_ref()
        .ok_or("moving piston received state missing")?;
    let progress = e.progress(f.partial);
    let mut moved = p.moved_state;
    if c.name(moved) == Some("minecraft:piston_head") {
        moved = c.with_state_property(
            moved,
            "short",
            if progress <= 0.5 { "true" } else { "false" },
        )?;
    } else if p.source && !p.extending {
        moved = if progress >= 0.5 {
            p.head_short
        } else {
            p.head_long
        }
        .ok_or("source piston head missing")?;
    }
    let offset = e.piston_offset(f.partial).unwrap().map(|v| v as f32);
    let source_position = std::array::from_fn(|i| {
        e.position[i] - p.direction[i] as i32 * if p.extending { 1 } else { -1 }
    });
    let status = (f.moving_block)(
        moved,
        source_position,
        root * Mat4::from_translation(Vec3::from_array(offset)),
        f.light,
        out,
    )?;
    if status != BlockRenderStatus::Rendered {
        return Ok(status);
    }
    if let Some(base) = p.base_collision_state {
        let status = (f.moving_block)(base, e.position, root, f.light, out)?;
        if status != BlockRenderStatus::Rendered {
            return Ok(status);
        }
    }
    Ok(BlockRenderStatus::Rendered)
}

#[cfg(test)]
mod tests {
    use super::super::resources::{Bone, BoneRole};
    use super::*;
    use std::{collections::BTreeMap, sync::Arc};

    #[test]
    fn p1_review_native_sign_pass_renders_received_text_and_keeps_entity_geometry() {
        use crate::inventory::{nbt, Tag};
        use crate::protocol::{Reader, Writer};
        for (kind, name, key, texture_name) in [
            (
                "minecraft:sign",
                "minecraft:oak_sign",
                "minecraft:sign/standing/oak#main",
                "minecraft:entity/signs/oak",
            ),
            (
                "minecraft:sign",
                "minecraft:oak_wall_sign",
                "minecraft:sign/wall/oak#main",
                "minecraft:entity/signs/oak",
            ),
            (
                "minecraft:hanging_sign",
                "minecraft:oak_hanging_sign",
                "minecraft:hanging_sign/oak/ceiling#main",
                "minecraft:entity/signs/hanging/oak",
            ),
            (
                "minecraft:hanging_sign",
                "minecraft:oak_wall_hanging_sign",
                "minecraft:hanging_sign/oak/wall#main",
                "minecraft:entity/signs/hanging/oak",
            ),
        ] {
            let mut content = Content::test_cube();
            content.names[1] = name.into();
            content.block_entity_names.insert(7, kind.into());
            content.insert_test_state_properties(
                1,
                BTreeMap::from([
                    ("rotation".into(), "8".into()),
                    ("facing".into(), "north".into()),
                ]),
            );
            content
                .insert_test_placement_metadata(1, serde_json::json!({"block_entity_type": kind}));
            content.insert_test_texture(
                texture_name,
                TextureRegion {
                    layer: 0,
                    uv: [0., 0., 1., 1.],
                    width: 64,
                    height: 32,
                },
            );
            // Owned empty-provider font exercises the real source missing-glyph
            // painter, without copying or depending on proprietary font assets.
            content.insert_test_font("minecraft:default", &serde_json::json!({"providers":[]}));
            content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
            let mut store = super::super::Store::new();
            store.set_text_renderer(Arc::new(TextRenderer::load(&content).unwrap()));
            store.activate_models(Arc::new(Models {
                rigs: BTreeMap::from([(
                    key.into(),
                    Rig {
                        bones: vec![Bone {
                            parent: None,
                            pivot: Vec3::ZERO,
                            rotation: Vec3::ZERO,
                            scale: Vec3::ONE,
                            role: BoneRole::Fixed,
                            name: "fixture-board".into(),
                            quads: vec![[
                                [0., 0., 0., 0., 0.],
                                [1., 0., 0., 1., 0.],
                                [1., 1., 0., 1., 1.],
                                [0., 1., 0., 0., 1.],
                            ]],
                            normals: vec![[0., 0., 1.]],
                        }],
                        part_by_name: BTreeMap::new(),
                    },
                )]),
                schemas: BTreeMap::new(),
                families: BTreeMap::new(),
                builtin_variants: BTreeMap::new(),
                simple_particles: Vec::new(),
                default_skins: Vec::new(),
                poses: BTreeMap::new(),
                attributes: BTreeMap::new(),
                attribute_ids: BTreeMap::new(),
                digest: String::new(),
                presentation: BTreeMap::new(),
                animations: BTreeMap::new(),
                motion: BTreeMap::new(),
            }));
            let position = [192, 77, -446];
            let origin = position.map(f64::from);
            let camera = [192.5, 78.62, -447.5];
            store
                .ensure_sign_block_entity(position, 1, &content, 1)
                .unwrap();
            let mut geometry = DynamicGeometry::default();
            let defaults = store
                .append_static_block_geometry(
                    &content,
                    &World::new(0, 256),
                    origin,
                    camera,
                    32.,
                    false,
                    false,
                    0.,
                    &|_| Ok(Some([7., 3.])),
                    &mut geometry,
                )
                .unwrap();
            assert_eq!(defaults.block_entities, 1);
            assert!(defaults.unrendered.is_empty());
            assert_eq!(geometry.vertices.len(), 4);
            // Exact vanilla-valid seq973 front payload: String-tag literal
            // components, byte glow, omitted optional back_text and extra keys.
            // The compound-component variant traverses the same receive path.
            for compound in [false, true] {
                let initial_vertices = geometry.vertices.len();
                let initial_body_indices =
                    geometry.indices[EntityMaterial::EntityCutout as usize].len();
                let messages = ["P1 PARITY", "", "", ""]
                    .into_iter()
                    .map(|line| {
                        if compound {
                            Tag::record([("text", Tag::string(line))])
                        } else {
                            Tag::string(line)
                        }
                    })
                    .collect();
                let received = Tag::record([
                    (
                        "front_text",
                        Tag::record([
                            ("messages", Tag::List(messages)),
                            ("color", Tag::string("red")),
                            ("has_glowing_text", Tag::Byte(1)),
                        ]),
                    ),
                    ("id", Tag::string(kind)),
                    ("x", Tag::Int(position[0])),
                    ("y", Tag::Int(position[1])),
                    ("z", Tag::Int(position[2])),
                ]);
                let mut packet = Writer::default();
                packet.i64(crate::protocol::pack_pos(position));
                packet.var(7);
                nbt::write(&received, &mut packet).unwrap();
                store
                    .apply(6, &mut Reader::new(&packet.0), &content, 2)
                    .unwrap();
                let data = store.block_entity_data(position).unwrap();
                let font = store.text.as_ref().unwrap();
                assert_eq!(
                    font.language
                        .plain(&data["front_text"]["messages"][0])
                        .unwrap(),
                    "P1 PARITY"
                );
                assert!(data.get("back_text").is_none());
                assert_eq!(store.block_entities[&position].state, Some(1));
                let submitted = store
                    .append_static_block_geometry(
                        &content,
                        &World::new(0, 256),
                        origin,
                        camera,
                        32.,
                        false,
                        false,
                        0.,
                        &|_| Ok(Some([7., 3.])),
                        &mut geometry,
                    )
                    .unwrap();
                assert_eq!(submitted.block_entities, 1);
                assert!(submitted.unrendered.is_empty());
                assert_eq!(
                    submitted.vertices,
                    geometry.vertices.len() - initial_vertices
                );
                assert!(submitted.vertices > 4);
                assert_eq!(
                    geometry.indices[EntityMaterial::EntityCutout as usize].len(),
                    initial_body_indices + 6
                );
                let text = &geometry.indices[EntityMaterial::TextPolygonOffset as usize];
                assert!(!text.is_empty());
                assert!(text
                    .iter()
                    .all(|index| geometry.vertices[*index as usize].light == [15.; 2]));
                let board_z = if kind == "minecraft:sign" && name.contains("_wall_") {
                    0.9375
                } else {
                    0.5
                };
                for triangle in text.chunks_exact(3) {
                    let [a, b, c]: [u32; 3] = triangle.try_into().unwrap();
                    let point =
                        |index: u32| Vec3::from_array(geometry.vertices[index as usize].position);
                    let [a, b, c] = [point(a), point(b), point(c)];
                    // At rotation8/north-facing attachment, glyphs sit outside
                    // the board's north face and their source winding faces north.
                    assert!([a, b, c]
                        .iter()
                        .all(|point| { point.z < board_z && point.z > board_z - 0.1 }));
                    assert!((b - a).cross(c - a).z < 0.);
                }
            }
            let mut pending = DynamicGeometry::default();
            let unavailable_light = store
                .append_static_block_geometry(
                    &content,
                    &World::new(0, 256),
                    origin,
                    camera,
                    32.,
                    false,
                    false,
                    0.,
                    &|_| Ok(None),
                    &mut pending,
                )
                .unwrap();
            assert_eq!(unavailable_light.lighting_pending, 1);
            assert_eq!(unavailable_light.block_entities, 0);
            assert!(pending.vertices.is_empty());
        }
    }
}
