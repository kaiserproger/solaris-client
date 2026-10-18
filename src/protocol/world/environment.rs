//! Gameplay context published by the received environment's packet/tick owner.
//! A world stores no independent weather/clock authority and never advances it.
use super::{Neighborhood, World, chunk_position};
use crate::protocol::{
    Result,
    content::{Content, normalize_id},
};

#[derive(Default)]
pub(super) struct Context {
    dimension: Option<String>,
    pub(super) biome_zoom_seed: Option<i64>,
    sea_level: Option<i32>,
}
impl World {
    /// Login/respawn metadata: the actual dimension key (not dimension-type key),
    /// the packet's already-hashed biome seed, and its received sea level.
    pub fn configure_environment_context(
        &mut self,
        dimension_id: &str,
        biome_zoom_seed: i64,
        sea_level: i32,
    ) -> Result<()> {
        self.environment_context = Context {
            dimension: Some(normalize_id(dimension_id)?),
            biome_zoom_seed: Some(biome_zoom_seed),
            sea_level: Some(sea_level),
        };
        Ok(())
    }
    pub fn dimension_id(&self) -> Result<&str> {
        self.environment_context
            .dimension
            .as_deref()
            .ok_or("world dimension identifier unreceived".into())
    }
    pub fn water_evaporates(&self, position: [i32; 3], content: &Content) -> Result<bool> {
        self.environment.water_evaporates(self, position, content)
    }
    pub fn creaking_active(&self, position: [i32; 3], content: &Content) -> Result<bool> {
        self.environment.creaking_active(self, position, content)
    }
    pub fn dimension_fast_lava(&self) -> Result<bool> {
        self.environment.fast_lava()
    }
    pub fn is_thundering(&self) -> Result<bool> {
        self.environment.thundering()
    }
    pub fn source_sky_darken(&self) -> Result<u8> {
        self.environment.sky_darken()
    }
    pub fn source_biome(&self, position: [i32; 3]) -> Result<u32> {
        let seed = self
            .environment_context
            .biome_zoom_seed
            .ok_or("source_biome_seed_unreceived")?;
        let position = biome_sample_position(position, seed, self.min_y, self.height)?;
        self.biome(position).ok_or("source_biome_unloaded".into())
    }
    pub fn biome_has_precipitation(&self, position: [i32; 3], content: &Content) -> Result<bool> {
        let biome = self.source_biome(position)?;
        crate::environment::biome::has_precipitation(
            content.registry_entry("minecraft:worldgen/biome", biome as i32)?,
        )
    }
    pub fn is_raining_at(&self, position: [i32; 3], content: &Content) -> Result<bool> {
        if !self.environment.raining()?
            || !self.source_can_see_sky(position)?
            || !self.rain_column_clear(position, content)?
        {
            return Ok(false);
        }
        let biome = self.source_biome(position)?;
        let sea_level = self
            .environment_context
            .sea_level
            .ok_or("world sea level unreceived")?;
        crate::environment::biome::rains(
            content.registry_entry("minecraft:worldgen/biome", biome as i32)?,
            position,
            sea_level,
        )
    }
    /// Level.setBlock uses isInValidBounds, NOT the ±30M world-border helper.
    /// Pinned ChunkPos.MAX_COORDINATE_VALUE = 2097061 (source static oracle).
    /// Missing client chunks stay unknown instead of pretending an air chunk.
    pub fn can_set_block(&self, position: [i32; 3]) -> Result<bool> {
        if self.height <= 0 {
            return Err("can_set_block_dimension_unreceived".into());
        }
        let chunk = chunk_position(position);
        if position[1] < self.min_y
            || i64::from(position[1]) >= i64::from(self.min_y) + i64::from(self.height)
            || chunk
                .iter()
                .any(|coordinate| !(-2_097_061..=2_097_061).contains(coordinate))
        {
            return Ok(false);
        }
        self.chunk(chunk).ok_or("can_set_block_chunk_unloaded")?;
        Ok(true)
    }
    /// Source isInsideCloseToBorder AND the border's rounded collision shape.
    pub fn border_collides(
        &self,
        entity_position: [f64; 3],
        bounds: crate::player::Bounds,
    ) -> Result<bool> {
        let [min_x, min_z, max_x, max_z] = self.border.bounds(0.);
        let width = (bounds.max[0] - bounds.min[0])
            .abs()
            .max((bounds.max[2] - bounds.min[2]).abs())
            .max(1.);
        let distance = (entity_position[0] - min_x)
            .min(max_x - entity_position[0])
            .min(entity_position[2] - min_z)
            .min(max_z - entity_position[2]);
        let close = distance < width * 2.
            && entity_position[0] >= min_x - width
            && entity_position[0] < max_x + width
            && entity_position[2] >= min_z - width
            && entity_position[2] < max_z + width;
        if !close || (0..3).any(|i| bounds.max[i] - bounds.min[i] < 1e-7) {
            return Ok(false);
        }
        // Shapes' index merge coalesces coordinates within its 1e-7 epsilon.
        Ok(bounds.min[0] + 1e-7 < min_x.floor()
            || bounds.max[0] - 1e-7 > max_x.ceil()
            || bounds.min[2] + 1e-7 < min_z.floor()
            || bounds.max[2] - 1e-7 > max_z.ceil())
    }
    /// BlockAndLightGetter.canSeeSky compares raw SKY brightness, not combined
    /// block light and not the sky-darkened local-brightness convenience getter.
    pub fn source_can_see_sky(&self, position: [i32; 3]) -> Result<bool> {
        Ok(self.source_sky_light(position)? >= 15)
    }
    fn source_sky_light(&self, position: [i32; 3]) -> Result<u8> {
        if !self.environment.has_skylight()? {
            return Ok(0);
        }
        let chunk = self
            .chunk(chunk_position(position))
            .ok_or("source_sky_light_unloaded")?;
        let highest = *chunk
            .sky
            .last_key_value()
            .ok_or("source_sky_light_unknown")?
            .0 as i64;
        let layer = (i64::from(position[1]) - i64::from(self.min_y)).div_euclid(16) + 1;
        if layer > highest {
            return Ok(15);
        }
        let direct = usize::try_from(layer)
            .ok()
            .and_then(|layer| chunk.sky.get(&layer));
        let (bytes, y) = if let Some(bytes) = direct {
            (bytes, position[1])
        } else {
            // Absent lower storage samples the next layer's bottom plane.
            let start = usize::try_from(layer.max(0)).map_err(|_| "source light layer overflow")?;
            let bytes = chunk
                .sky
                .range(start..)
                .next()
                .ok_or("source_sky_light_unknown")?
                .1;
            (bytes, 0)
        };
        let index = super::block_index([position[0], y, position[2]]);
        Ok((bytes[index / 2] >> ((index % 2) * 4)) & 15)
    }
    /// Source light-engine storage query, including absent-layer and outside-height fallback.
    pub fn source_light_levels(&self, position: [i32; 3]) -> Result<[u8; 2]> {
        let sky = self.source_sky_light(position)?;
        let chunk = self
            .chunk(chunk_position(position))
            .ok_or("source_block_light_unloaded")?;
        let layer = (i64::from(position[1]) - i64::from(self.min_y)).div_euclid(16) + 1;
        let block = usize::try_from(layer)
            .ok()
            .and_then(|layer| chunk.block.get(&layer))
            .map_or(0, |bytes| {
                let index = super::block_index(position);
                (bytes[index / 2] >> ((index % 2) * 4)) & 15
            });
        Ok([sky, block])
    }
    /// Exact getRawBrightness(position, 0); does not implicitly subtract skyDarken.
    pub fn source_raw_brightness(&self, position: [i32; 3]) -> Result<u8> {
        let [sky, block] = self.source_light_levels(position)?;
        Ok(sky.max(block))
    }
    pub fn source_is_in_world_bounds(&self, position: [i32; 3]) -> bool {
        i64::from(position[1]) >= i64::from(self.min_y)
            && i64::from(position[1]) < i64::from(self.min_y) + i64::from(self.height)
            && (-30_000_000..30_000_000).contains(&position[0])
            && (-30_000_000..30_000_000).contains(&position[2])
    }
    /// Tests only the obstruction half of Level.precipitationAt's
    /// MOTION_BLOCKING heightmap comparison, using actual received block states.
    /// No camera Gaussian, surface-heightmap substitute or full-world snapshot.
    fn rain_column_clear(&self, position: [i32; 3], content: &Content) -> Result<bool> {
        let chunk = self
            .chunk(chunk_position(position))
            .ok_or("rain column unloaded")?;
        if position[1] < self.min_y {
            return Ok(false);
        }
        let top = self
            .min_y
            .checked_add(self.height)
            .ok_or("dimension height overflow")?;
        for y in position[1]..top {
            let section = ((y - self.min_y) / 16) as usize;
            let state = chunk
                .sections
                .get(section)
                .ok_or("rain column section missing")?
                .blocks
                .get(super::block_index([position[0], y, position[2]]));
            if content.block_blocks_motion(state)? || content.info(state)?.fluid.is_some() {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

impl Neighborhood {
    pub fn source_biome(&self, position: [i32; 3]) -> Result<u32> {
        let seed = self.biome_zoom_seed.ok_or("source_biome_seed_unreceived")?;
        let position = biome_sample_position(position, seed, self.min_y, self.height)?;
        self.biome(position).ok_or("source_biome_unloaded".into())
    }
}
fn biome_sample_position(
    position: [i32; 3],
    seed: i64,
    min_y: i32,
    height: i32,
) -> Result<[i32; 3]> {
    let top = min_y
        .checked_add(height)
        .filter(|_| height > 0)
        .ok_or("source_biome_dimension_unknown")?
        - 1;
    let quart = selected_quart(position, seed);
    Ok([
        quart[0].wrapping_mul(4),
        quart[1].clamp(min_y >> 2, top >> 2).wrapping_mul(4),
        quart[2].wrapping_mul(4),
    ])
}
fn selected_quart(position: [i32; 3], seed: i64) -> [i32; 3] {
    let shifted = position.map(|value| value.wrapping_sub(2));
    let base = shifted.map(|value| value >> 2);
    let fraction = shifted.map(|value| f64::from(value & 3) / 4.);
    let mut result = base;
    let mut closest = f64::INFINITY;
    for corner in 0..8 {
        let offset = [corner >> 2 & 1, corner >> 1 & 1, corner & 1];
        let quart = std::array::from_fn(|i| base[i] + offset[i]);
        let delta = std::array::from_fn::<_, 3, _>(|i| fraction[i] - f64::from(offset[i]));
        let mut random = seed;
        for coordinate in quart.into_iter().chain(quart) {
            random = next(random, i64::from(coordinate));
        }
        let x = delta[0] + fiddle(random);
        random = next(random, seed);
        let y = delta[1] + fiddle(random);
        random = next(random, seed);
        let z = delta[2] + fiddle(random);
        let distance = z * z + y * y + x * x;
        if distance < closest {
            closest = distance;
            result = quart;
        }
    }
    result
}
fn next(state: i64, salt: i64) -> i64 {
    state
        .wrapping_mul(
            state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407),
        )
        .wrapping_add(salt)
}
fn fiddle(state: i64) -> f64 {
    (f64::from((state >> 24).rem_euclid(1024) as i32) / 1024. - 0.5) * 0.9
}
