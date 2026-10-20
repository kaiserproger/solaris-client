use super::catalog::Catalog;
use crate::{
    api::{DynamicGeometry, EntityMaterial, MeshGeometry, RenderLayer, Vertex},
    protocol::{
        content::TintKind,
        models::{
            lighting::{quad_lighting, LightSample},
            BakedQuad,
        },
        world::Neighborhood,
        Result,
    },
};
use glam::{Mat4, Vec3};
use std::sync::atomic::{AtomicU64, Ordering};
mod facing;
mod fluid;
mod greedy;

/// GPU shader tag carried in `Vertex.normal` magnitude (parent shader does
/// `round(length(normal))`; Off path never decodes it). Class 1 is ordinary
/// geometry (unit normals, unchanged). Class 2 is rooted waving vegetation
/// (grass/flowers/crops) whose length is `2.0 + 0.2 * local_y` (root 0, tip 1)
/// so the shader recovers per-vertex sway amplitude
/// `clamp((length - 2) * 5, 0, 1)`; whole upper halves of two-block plants pin
/// 2.2 (weight 1) to meet the lower half's waving tip. Class 3 is whole-leaf
/// gentle motion (uniform length 3). Class 4 is WATER only (length 4, real
/// side/top/bottom normals preserved); lava stays class 1. Derived once per
/// state at catalog capture from block identity, never tint/color guesses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ShaderClass {
    #[default]
    Ordinary = 1,
    RootedVegetation = 2,
    Leaves = 3,
    Water = 4,
}

impl ShaderClass {
    fn normal(self, base: [f32; 3], local_y: f32, upper_half: bool) -> [f32; 3] {
        let magnitude = match self {
            Self::Ordinary => 1.,
            Self::Leaves => 3.,
            Self::Water => 4.,
            Self::RootedVegetation if upper_half => 2.2,
            Self::RootedVegetation => 2. + 0.2 * local_y.clamp(0., 1.),
        };
        base.map(|component| component * magnitude)
    }
}

/// Classify authored vanilla identities once, never tint/color guesses.
pub(super) fn shader_class(name: &str, upper_half: bool) -> (ShaderClass, bool) {
    let short = match name.split_once(':') {
        Some(("minecraft", short)) => short,
        Some(_) => return (ShaderClass::Ordinary, false),
        None => name,
    };
    if short == "water" || short == "bubble_column" {
        return (ShaderClass::Water, false);
    }
    if short.ends_with("_leaves") {
        return (ShaderClass::Leaves, false);
    }
    let rooted = matches!(
        short,
        "short_grass"
            | "tall_grass"
            | "fern"
            | "large_fern"
            | "short_dry_grass"
            | "tall_dry_grass"
            | "seagrass"
            | "tall_seagrass"
            | "pink_petals"
            | "wildflowers"
            | "allium"
            | "azure_bluet"
            | "blue_orchid"
            | "cornflower"
            | "dandelion"
            | "golden_dandelion"
            | "lily_of_the_valley"
            | "orange_tulip"
            | "oxeye_daisy"
            | "pink_tulip"
            | "poppy"
            | "red_tulip"
            | "white_tulip"
            | "torchflower"
            | "wither_rose"
            | "open_eyeblossom"
            | "closed_eyeblossom"
            | "spore_blossom"
            | "cactus_flower"
            | "chorus_flower"
            | "sunflower"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "pitcher_plant"
            | "sweet_berry_bush"
            | "cocoa"
            | "nether_wart"
            | "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "melon_stem"
            | "pumpkin_stem"
            | "attached_melon_stem"
            | "attached_pumpkin_stem"
            | "pitcher_crop"
            | "torchflower_crop"
    );
    if rooted {
        (ShaderClass::RootedVegetation, upper_half)
    } else {
        (ShaderClass::Ordinary, false)
    }
}

#[derive(Clone, Copy, PartialEq)]
pub(super) struct Rules {
    pub directional_shade: [f32; 6],
    pub ambient_occlusion: bool,
    pub biome_blend_radius: u8,
    pub fade_duration_ms: Option<u64>,
}

pub(super) const NORMALS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];
pub(super) fn add(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
pub(super) struct BuiltSection {
    pub geometry: MeshGeometry,
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    pub visibility: u64,
    pub unknown_light_samples: u64,
    pub unknown_biome_samples: u64,
}

pub(super) struct Samples<'a> {
    pub world: &'a Neighborhood,
    pub catalog: &'a Catalog,
    unknown_light: u64,
    unknown_biome: u64,
    /// Blocks temporarily drawn by a presentation animation instead of the
    /// section mesh; they meshed as air so neighbors expose their faces.
    hidden: &'a [[i32; 3]],
}
impl Samples<'_> {
    pub fn state(&self, position: [i32; 3]) -> Result<u32> {
        // Unknown neighboring columns are not occluders. Their boundary faces
        // are remeshed when the packet owner invalidates arrival/unload.
        if !self.hidden.is_empty() && self.hidden.contains(&position) {
            return Ok(self.catalog.air);
        }
        Ok(self.world.block(position).unwrap_or(self.catalog.air))
    }
    pub fn light(&mut self, position: [i32; 3]) -> Result<[f32; 2]> {
        let emission = self.catalog.state(self.state(position)?)?.light_emission;
        let [sky, block] = self.world.light_levels(position);
        let (Some(sky), Some(block)) = (sky, block) else {
            self.unknown_light += 1;
            return Ok([-1.; 2]);
        };
        Ok([f32::from(sky), f32::from(block.max(emission))])
    }
    pub fn light_sample(&mut self, position: [i32; 3]) -> Result<LightSample> {
        let light = self.light(position)?;
        let fact = &self.catalog.state(self.state(position)?)?.fact;
        Ok(LightSample {
            shade_brightness: fact.shade_brightness,
            view_blocking: fact.view_blocking,
            light_dampening: fact.light_dampening,
            solid_render: fact.solid_render,
            collision_full: fact.collision_full,
            light,
        })
    }
    pub fn tint(&mut self, tint: TintKind, position: [i32; 3], radius: u8) -> Result<[f32; 3]> {
        match tint {
            TintKind::None => return Ok([1.; 3]),
            TintKind::Constant(color) => return Ok(color),
            _ => {}
        }
        let radius = i32::from(radius);
        let mut total = [0u32; 3];
        let mut count = 0u32;
        for z in position[2] - radius..=position[2] + radius {
            for x in position[0] - radius..=position[0] + radius {
                let p = [x, position[1], z];
                let biome = match self.world.source_biome(p) {
                    Ok(biome) => biome,
                    Err(error) if error == "source_biome_unloaded" => {
                        self.unknown_biome += 1;
                        continue;
                    }
                    Err(error) => return Err(error),
                };
                let color = self.catalog.biome_color(biome, tint, p)?;
                for axis in 0..3 {
                    total[axis] += (color[axis] * 255.).round() as u32;
                }
                count += 1;
            }
        }
        if count == 0 {
            return Err("terrain_biome_color_neighborhood_unavailable".into());
        }
        Ok(total.map(|v| (v / count) as f32 / 255.))
    }
}
pub(super) fn geometry_bytes(geometry: &MeshGeometry) -> usize {
    geometry.vertices.len() * std::mem::size_of::<Vertex>()
        + geometry.indices.iter().map(|i| i.len() * 4).sum::<usize>()
}
fn quantize(color: [f32; 3], shade: f32) -> [f32; 3] {
    color.map(|v| (v * 255. * shade).floor().clamp(0., 255.) / 255.)
}
fn model_color(tint: [f32; 3], ao: f32, shade: f32) -> [f32; 3] {
    // Source packs AO gray first, scales that byte by cardinal light, then
    // applies the tint with integer ARGB.multiply (not float tint * AO).
    let gray = ((ao * 255.).round() * shade).floor().clamp(0., 255.) as u32;
    tint.map(|channel| (((channel * 255.).round() as u32 * gray) / 255) as f32 / 255.)
}
/// Leaf layers (counted along a face's outward normal) whose faces stay
/// drawn. Fancy leaves render every face of every block, so a canopy carries
/// six faces per block although the cutout holes only ever reveal the first
/// layers behind the surface. A face with this many leaf or opaque blocks in
/// front of it cannot be seen through the outer layers' holes and is dropped
/// (the usual "cull leaves" depth rule). Edits invalidate two blocks around
/// the changed cell, which covers this look-ahead.
const LEAF_VISIBLE_DEPTH: i32 = 2;

fn buried_leaf_face(samples: &Samples<'_>, position: [i32; 3], face: u8) -> Result<bool> {
    let normal = NORMALS[face as usize];
    for depth in 1..=LEAF_VISIBLE_DEPTH {
        let cell = add(position, normal.map(|v| v * depth));
        let info = samples.catalog.state(samples.state(cell)?)?;
        let opaque = info.fact.solid_render && info.fact.can_occlude && info.fact.collision_full;
        if info.shader != ShaderClass::Leaves && !opaque {
            return Ok(false);
        }
    }
    Ok(true)
}
fn model_quad(
    samples: &mut Samples<'_>,
    state: u32,
    position: [i32; 3],
    quad: &BakedQuad,
    rules: Rules,
    cull: bool,
    mut emit: impl FnMut([Vertex; 4], RenderLayer) -> Result<()>,
) -> Result<()> {
    if cull {
        if let Some(face) = quad.cullface {
            let neighbor = samples.state(add(position, NORMALS[face as usize]))?;
            if !samples.catalog.face_visible(state, neighbor, face)? {
                return Ok(());
            }
            if samples.catalog.state(state)?.shader == ShaderClass::Leaves
                && buried_leaf_face(samples, position, face)?
            {
                return Ok(());
            }
        }
    }
    let light = quad_lighting(
        quad,
        position,
        rules.ambient_occlusion && samples.catalog.state(state)?.light_emission == 0,
        |p| samples.light_sample(p),
    )?;
    let tint = samples.tint(quad.tint, position, rules.biome_blend_radius)?;
    let shade = if quad.shade {
        rules.directional_shade[quad.face as usize]
    } else {
        rules.directional_shade[1]
    };
    let tagged = samples.catalog.state(state)?;
    let offset = tagged.fact.offset.at(position).map(|value| value as f32);
    let normal_base = NORMALS[quad.face as usize].map(|v| v as f32);
    // Axis normals preserve direction; length carries the material and rooted tip weight.
    emit(
        std::array::from_fn(|i| Vertex {
            position: std::array::from_fn(|axis| quad.positions[i][axis] + offset[axis]),
            color: model_color(tint, light[i].ao, shade),
            uv: quad.uv[i],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            alpha: quad.alpha,
            texture_layer: quad.texture_layer,
            light: light[i].light,
            normal: tagged
                .shader
                .normal(normal_base, quad.positions[i][1], tagged.shader_upper),
        }),
        quad.layer,
    )
}
pub(super) fn append_quad(
    geometry: &mut MeshGeometry,
    vertices: [Vertex; 4],
    layer: RenderLayer,
    backward: bool,
    byte_limit: usize,
) -> Result<()> {
    if geometry_bytes(geometry) + 4 * std::mem::size_of::<Vertex>() + if backward { 48 } else { 24 }
        > byte_limit
    {
        return Err("terrain_single_section_geometry_byte_bound_reached".into());
    }
    let first = geometry.vertices.len() as u32;
    geometry.vertices.extend(vertices);
    geometry.indices[layer as usize].extend_from_slice(&[
        first,
        first + 1,
        first + 2,
        first,
        first + 2,
        first + 3,
    ]);
    if backward {
        geometry.indices[layer as usize].extend_from_slice(&[
            first + 3,
            first + 2,
            first,
            first + 2,
            first + 1,
            first,
        ]);
    }
    Ok(())
}
#[cfg(test)]
pub(super) fn build(
    world: &Neighborhood,
    catalog: &Catalog,
    key: [i32; 3],
    rules: Rules,
    cancellation: &AtomicU64,
    generation: u64,
    byte_limit: usize,
) -> Result<Option<BuiltSection>> {
    build_hiding(
        world,
        catalog,
        key,
        rules,
        cancellation,
        generation,
        byte_limit,
        &[],
    )
}
#[allow(clippy::too_many_arguments)]
pub(super) fn build_hiding(
    world: &Neighborhood,
    catalog: &Catalog,
    key: [i32; 3],
    rules: Rules,
    cancellation: &AtomicU64,
    generation: u64,
    byte_limit: usize,
    hidden: &[[i32; 3]],
) -> Result<Option<BuiltSection>> {
    let mut samples = Samples {
        world,
        catalog,
        unknown_light: 0,
        unknown_biome: 0,
        hidden,
    };
    let Some(chunk) = world.center_chunk() else {
        return Ok(None);
    };
    let index = key[1] - world.min_y.div_euclid(16);
    let section = chunk
        .sections
        .get(index as usize)
        .ok_or("terrain_job_section_outside_dimension")?;
    let mut geometry = MeshGeometry::default();
    let mut greedy = greedy::Greedy::new();
    let mut fluid_tops = greedy::Greedy::fluid();
    let greedy_enabled = greedy::enabled();
    let mut opaque = [false; 4096];
    let base = key.map(|v| v * 16);
    for y in 0..16 {
        if cancellation.load(Ordering::Acquire) != generation {
            return Ok(None);
        }
        for z in 0..16 {
            for x in 0..16 {
                let position = add(base, [x, y, z]);
                let state = if !hidden.is_empty() && hidden.contains(&position) {
                    catalog.air
                } else {
                    section.blocks.get((y * 256 + z * 16 + x) as usize)
                };
                let info = catalog.state(state)?;
                opaque[(y * 256 + z * 16 + x) as usize] =
                    info.fact.solid_render && info.fact.can_occlude && info.fact.collision_full;
                if let Some(model) = info.model {
                    let mut result = Ok(());
                    catalog.models[model].for_each_quad(position, |quad| {
                        if result.is_err() {
                            return;
                        }
                        result = model_quad(
                            &mut samples,
                            state,
                            position,
                            quad,
                            rules,
                            true,
                            |mut vertices, layer| {
                                for vertex in &mut vertices {
                                    for axis in 0..3 {
                                        vertex.position[axis] += [x, y, z][axis] as f32;
                                    }
                                }
                                if greedy_enabled
                                    && opaque[(y * 256 + z * 16 + x) as usize]
                                    && layer == RenderLayer::Opaque
                                    && quad.cullface == Some(quad.face)
                                    && greedy.push(vertices, [x, y, z], quad.face as usize)
                                {
                                    return Ok(());
                                }
                                append_quad(&mut geometry, vertices, layer, false, byte_limit)
                            },
                        );
                    });
                    result?;
                }
                if let Some(fluid) = info.fluid {
                    fluid::emit(
                        &mut samples,
                        state,
                        position,
                        [x as f32, y as f32, z as f32],
                        fluid,
                        rules,
                        &mut geometry,
                        &mut fluid_tops,
                        byte_limit,
                    )?;
                }
            }
        }
    }
    greedy.append(&mut geometry, byte_limit)?;
    fluid_tops.append(&mut geometry, byte_limit)?;
    facing::apply(&mut geometry);
    let (mut bounds_min, mut bounds_max) = ([0.; 3], [16.; 3]);
    for vertex in &geometry.vertices {
        for axis in 0..3 {
            bounds_min[axis] = f32::min(bounds_min[axis], vertex.position[axis]);
            bounds_max[axis] = f32::max(bounds_max[axis], vertex.position[axis]);
        }
    }
    Ok(Some(BuiltSection {
        geometry,
        bounds_min,
        bounds_max,
        visibility: super::visibility::build(&opaque),
        unknown_light_samples: samples.unknown_light,
        unknown_biome_samples: samples.unknown_biome,
    }))
}
pub(super) fn moving_block(
    world: &Neighborhood,
    catalog: &Catalog,
    rules: Rules,
    state: u32,
    position: [i32; 3],
    transform: Mat4,
    out: &mut DynamicGeometry,
) -> Result<bool> {
    let Some(model) = catalog.state(state)?.model else {
        return Ok(false);
    };
    let mut samples = Samples {
        world,
        catalog,
        unknown_light: 0,
        unknown_biome: 0,
        hidden: &[],
    };
    let mut result = Ok(());
    let mut emitted = false;
    let normals = transform.inverse().transpose();
    catalog.models[model].for_each_quad(position, |quad| {
        if result.is_err() {
            return;
        }
        result = model_quad(
            &mut samples,
            state,
            position,
            quad,
            rules,
            false,
            |mut vertices, layer| {
                if (out.vertices.len() + 4) * std::mem::size_of::<Vertex>() > 32 * 1024 * 1024 {
                    return Err("moving_block_dynamic_geometry_byte_bound_reached".into());
                }
                let first = out.vertices.len() as u32;
                for vertex in &mut vertices {
                    vertex.position = transform
                        .transform_point3(Vec3::from_array(vertex.position))
                        .to_array();
                    // Dynamic materials use unit normals for vanilla cardinal lighting.
                    vertex.normal = normals
                        .transform_vector3(Vec3::from_array(vertex.normal))
                        .normalize_or_zero()
                        .to_array();
                }
                out.vertices.extend(vertices);
                let material = match layer {
                    RenderLayer::Opaque => EntityMaterial::SolidMovingBlock,
                    RenderLayer::Cutout => EntityMaterial::CutoutMovingBlock,
                    RenderLayer::Translucent => EntityMaterial::TranslucentMovingBlock,
                };
                out.indices[material as usize].extend_from_slice(&[
                    first,
                    first + 1,
                    first + 2,
                    first,
                    first + 2,
                    first + 3,
                ]);
                emitted = true;
                Ok(())
            },
        );
    });
    result?;
    Ok(emitted)
}

#[cfg(test)]
mod tests {
    use super::{model_color, shader_class, ShaderClass};

    #[test]
    fn model_lighting_packs_ao_then_shade_before_integer_tint_multiplication() {
        let tint = [61., 140., 255.].map(|v| v / 255.);
        assert_eq!(
            model_color(tint, 127. / 255., 0.6),
            [18., 41., 76.].map(|v| v / 255.)
        );
        assert_eq!(
            model_color(tint, 1., 0.9),
            [54., 125., 229.].map(|v| v / 255.)
        );
    }

    fn length(normal: [f32; 3]) -> f32 {
        normal.iter().map(|v| v * v).sum::<f32>().sqrt()
    }

    #[test]
    fn vegetation_classification_uses_block_identity_not_color() {
        // All named vegetation groups land in class 2 (rooted) or 3 (leaves).
        for name in [
            "minecraft:short_grass",
            "minecraft:tall_grass",
            "minecraft:fern",
            "minecraft:large_fern",
            "minecraft:dandelion",
            "minecraft:poppy",
            "minecraft:sunflower",
            "minecraft:lilac",
            "minecraft:wheat",
            "minecraft:carrots",
            "minecraft:potatoes",
            "minecraft:beetroots",
            "minecraft:melon_stem",
            "minecraft:pitcher_crop",
            "minecraft:sweet_berry_bush",
            "minecraft:seagrass",
            "minecraft:cocoa",
            "minecraft:nether_wart",
        ] {
            let (class, _) = shader_class(name, false);
            assert_eq!(class, ShaderClass::RootedVegetation, "{name}");
        }
        for name in [
            "minecraft:oak_leaves",
            "minecraft:birch_leaves",
            "minecraft:spruce_leaves",
            "minecraft:jungle_leaves",
            "minecraft:acacia_leaves",
            "minecraft:dark_oak_leaves",
            "minecraft:mangrove_leaves",
            "minecraft:cherry_leaves",
            "minecraft:pale_oak_leaves",
            "minecraft:azalea_leaves",
        ] {
            let (class, _) = shader_class(name, false);
            assert_eq!(class, ShaderClass::Leaves, "{name}");
        }
    }

    #[test]
    fn water_and_lava_are_distinct_with_ordinary_stone_excluded() {
        assert_eq!(shader_class("minecraft:water", false).0, ShaderClass::Water);
        assert_eq!(
            shader_class("minecraft:bubble_column", false).0,
            ShaderClass::Water
        );
        // Lava must NOT carry the water tag (stays ordinary, unit normals).
        assert_eq!(
            shader_class("minecraft:lava", false).0,
            ShaderClass::Ordinary
        );
        for name in [
            "minecraft:stone",
            "minecraft:oak_log",
            "minecraft:grass_block",
            "minecraft:flower_pot",
            "minecraft:cactus",
            "minecraft:chorus_plant",
            "minecraft:lily_pad",
            "minecraft:oak_planks",
            "custom:oak_leaves",
        ] {
            assert_eq!(shader_class(name, false).0, ShaderClass::Ordinary, "{name}");
        }
    }

    #[test]
    fn class_lengths_decode_with_round_and_finite_normals() {
        for class in [
            ShaderClass::Ordinary,
            ShaderClass::RootedVegetation,
            ShaderClass::Leaves,
            ShaderClass::Water,
        ] {
            for base in super::NORMALS.map(|normal| normal.map(|v| v as f32)) {
                let tagged = class.normal(base, 0.5, false);
                let magnitude = length(tagged);
                assert!(tagged.iter().all(|v| v.is_finite()));
                assert_eq!(magnitude.round() as u8, class as u8);
                assert_eq!(tagged.map(|v| v / magnitude), base);
            }
        }
        for local_y in [-0.5f32, 0., 0.5, 1., 1.5] {
            let tagged = ShaderClass::RootedVegetation.normal([0., 1., 0.], local_y, false);
            let amplitude = ((length(tagged) - 2.) * 5.).clamp(0., 1.);
            assert!((amplitude - local_y.clamp(0., 1.)).abs() < 1e-6);
        }
    }

    #[test]
    fn upper_half_of_two_block_plant_pins_full_sway_weight() {
        // Continuity: upper half whole quad at 2.2 meets lower half tip 2.2.
        let (class, upper) = shader_class("minecraft:sunflower", true);
        assert_eq!(class, ShaderClass::RootedVegetation);
        assert!(upper);
        let upper_base = class.normal([0., 1., 0.], 0., upper);
        let upper_tip = class.normal([0., 1., 0.], 1., upper);
        assert_eq!(upper_base, upper_tip);
        // Lower half emits the 2.0..2.2 ramp keyed by local quad height.
        let (class, upper) = shader_class("minecraft:sunflower", false);
        assert_eq!(class, ShaderClass::RootedVegetation);
        assert!(!upper);
        assert_eq!(class.normal([0., 1., 0.], 1., upper), upper_base);
    }
}
