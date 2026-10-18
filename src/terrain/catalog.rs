use super::mesher::ShaderClass;
use crate::protocol::{
    Result,
    content::{
        BakedModel, BiomeColors, Content, FluidKind, FluidState, FluidTextures, Simplex2, TintKind,
    },
    models::shapes::{TerrainFact, rectangles_cover},
};

pub(super) struct State {
    pub model: Option<usize>,
    pub fluid: Option<FluidState>,
    pub light_emission: u8,
    pub fluid_overlay: bool,
    pub fact: TerrainFact,
    /// Fancy-shader tag class, precomputed once per state from block identity
    /// (see mesher::shader_class); no per-face name lookup or allocation.
    pub shader: ShaderClass,
    /// Upper half of a two-block plant (`half == "upper"`): whole quad pins
    /// class-2 length 2.2 so the seam meets the lower half's waving tip.
    pub shader_upper: bool,
}
struct Biome {
    colors: BiomeColors,
    swamp: bool,
}
pub(super) struct Catalog {
    pub air: u32,
    pub states: Vec<State>,
    pub models: Vec<BakedModel>,
    pub fluids: [FluidTextures; 2],
    biomes: Vec<Biome>,
    swamp_noise: Simplex2,
}
impl Catalog {
    pub fn capture(content: &Content) -> Result<Self> {
        let facts = content.terrain_facts();
        if facts.len() != content.states.len() {
            return Err("terrain_catalog_state_fact_count_mismatch".into());
        }
        let air = content
            .states
            .iter()
            .position(|state| state.is_air)
            .ok_or("terrain_catalog_missing_source_air_state")? as u32;
        let states = content
            .states
            .iter()
            .enumerate()
            .map(|(id, state)| {
                let upper = content
                    .state_properties(id as u32)
                    .is_some_and(|properties| {
                        properties.get("half").is_some_and(|half| half == "upper")
                    });
                let (shader, shader_upper) =
                    super::mesher::shader_class(content.name(id as u32).unwrap_or(""), upper);
                Ok(State {
                    model: state.model,
                    fluid: state.fluid,
                    light_emission: state.light_emission,
                    fluid_overlay: content.block_fluid_overlay(id as u32)?,
                    fact: facts[id].clone(),
                    shader,
                    shader_upper,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let mut biomes = Vec::new();
        for (_, source) in content.registry_entries("minecraft:worldgen/biome")? {
            biomes.push(Biome {
                colors: content.biome_colors(source, 0., 0.)?,
                swamp: source["effects"]["grass_color_modifier"].as_str() == Some("swamp"),
            });
        }
        if biomes.is_empty() {
            return Err("terrain_catalog_missing_received_biomes".into());
        }
        Ok(Self {
            air,
            states,
            models: content.models.clone(),
            fluids: [
                content.fluid_textures(FluidKind::Water)?,
                content.fluid_textures(FluidKind::Lava)?,
            ],
            biomes,
            swamp_noise: Simplex2::new(2345),
        })
    }
    pub fn state(&self, state: u32) -> Result<&State> {
        self.states
            .get(state as usize)
            .ok_or_else(|| format!("unknown_mandatory_terrain_state {state}"))
    }
    pub fn face_visible(&self, state: u32, neighbor: u32, face: u8) -> Result<bool> {
        if face >= 6 {
            return Err("terrain_face_direction_bound".into());
        }
        let current = &self.state(state)?.fact;
        let neighboring = &self.state(neighbor)?.fact;
        let target = &neighboring.faces[(face ^ 1) as usize];
        if rectangles_cover(target, &[[0., 0., 1., 1.]])
            || current.skip[face as usize].binary_search(&neighbor).is_ok()
        {
            return Ok(false);
        }
        Ok(target.is_empty()
            || current.faces[face as usize].is_empty()
            || !rectangles_cover(target, &current.faces[face as usize]))
    }
    pub fn biome_color(&self, id: u32, tint: TintKind, position: [i32; 3]) -> Result<[f32; 3]> {
        let biome = self
            .biomes
            .get(id as usize)
            .ok_or("unknown_received_terrain_biome_holder")?;
        Ok(match tint {
            TintKind::None => [1.; 3],
            TintKind::Constant(color) => color,
            TintKind::Grass if biome.swamp => {
                let packed = if self.swamp_noise.value(
                    f64::from(position[0]) * 0.0225,
                    f64::from(position[2]) * 0.0225,
                ) < -0.1
                {
                    0x4c763c
                } else {
                    0x6a7039
                };
                [
                    (packed >> 16 & 255) as f32 / 255.,
                    (packed >> 8 & 255) as f32 / 255.,
                    (packed & 255) as f32 / 255.,
                ]
            }
            TintKind::Grass => biome.colors.grass,
            TintKind::Foliage => biome.colors.foliage,
            TintKind::DryFoliage => biome.colors.dry_foliage,
            TintKind::Water => biome.colors.water,
        })
    }
}
