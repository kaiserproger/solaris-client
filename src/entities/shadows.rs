//! Authored approximation of the entity discs visible in our Java oracle shots.
//! The runtime resource pack supplies `misc/shadow`; range, fade and radius
//! formulae below are our tuning choices, not extracted game constants.
use super::{presentation as p, Entity, Models, Store};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{content::Content, world::World, Result},
};

/// Shadows fade with camera distance and are gone at this range, in blocks.
const SHADOW_RANGE: f64 = 16.;
/// Opacity of a shadow cast onto the block directly under the feet.
const SHADOW_OPACITY: f32 = 0.5;
/// Height above a block over which a shadow fades out, in blocks.
const SHADOW_DROP: f64 = 2.;
/// Quads below this opacity are not worth submitting.
const MIN_ALPHA: f32 = 1. / 256.;

/// Authored radius fit for the villager and golem discs in our oracle shots.
/// The width-based fit for other mobs has not been individually validated.
fn base_radius(kind: &str, width: f32) -> f32 {
    match kind {
        "minecraft:iron_golem" => 0.7,
        _ => 0.55 * width + 0.17,
    }
}

/// Shadow radius of `entity`, or `None` when it casts no blob shadow.
/// An extra baby-size factor approximates the small child-villager disc in the
/// oracle, in addition to the entity's ordinary presentation scale.
pub(super) fn radius(models: &Models, entity: &Entity) -> Result<Option<f32>> {
    let living = entity
        .source
        .as_ref()
        .is_some_and(|source| source.living.is_some());
    let invisible = p::number(models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & 32 != 0;
    if !living || invisible || entity.type_name == "minecraft:armor_stand" {
        return Ok(None);
    }
    let width = models
        .schemas
        .get(&entity.type_name)
        .ok_or("entity shadow schema missing")?
        .dimensions
        .width;
    let (_, scale) = models.dimensions(entity)?;
    let baby = if p::boolean(models, entity, "DATA_BABY_ID") {
        0.5
    } else {
        1.
    };
    Ok(Some(base_radius(&entity.type_name, width) * scale * baby))
}

/// Opacity of a shadow cast onto a block top `height` blocks below the feet,
/// `distance_squared` from the camera, under raw light `level` (0..=15).
pub(super) fn opacity(height: f64, distance_squared: f64, level: u8) -> f32 {
    let height = (1. - height / SHADOW_DROP).clamp(0., 1.) as f32;
    let distance = (1. - distance_squared / (SHADOW_RANGE * SHADOW_RANGE)).clamp(0., 1.) as f32;
    let light = f32::from(level.min(15)) / 15.;
    SHADOW_OPACITY * height * distance * light * light
}

impl Store {
    /// Appends blob shadows for living entities near the camera onto the full
    /// block tops under them. Returns the number of entities that cast one.
    #[expect(clippy::too_many_arguments, reason = "Shares the scene extraction inputs")]
    pub(crate) fn append_shadow_geometry(
        &self,
        content: &Content,
        world: &World,
        origin: [f64; 3],
        camera: [f64; 3],
        tick: u64,
        partial: f32,
        out: &mut DynamicGeometry,
    ) -> Result<usize> {
        let Some(models) = self.models.as_deref() else {
            return Ok(0);
        };
        // A resource set without the disc texture simply casts no shadows.
        let Some(texture) = content.texture("minecraft:misc/shadow") else {
            return Ok(0);
        };
        let darken = world.source_sky_darken().unwrap_or(0);
        let min = [
            ((camera[0] - SHADOW_RANGE) / 16.).floor() as i32,
            ((camera[2] - SHADOW_RANGE) / 16.).floor() as i32,
        ];
        let max = [
            ((camera[0] + SHADOW_RANGE) / 16.).floor() as i32,
            ((camera[2] + SHADOW_RANGE) / 16.).floor() as i32,
        ];
        let mut casters = 0;
        for (_, bucket) in self
            .spatial
            .range([min[0], i32::MIN]..=[max[0], i32::MAX])
            .filter(|(cell, _)| cell[1] >= min[1] && cell[1] <= max[1])
        {
            for id in bucket {
                let entity = &self.slots[self.ids[id]];
                let position = entity.presented_transform(tick, partial).position;
                let distance_squared = position
                    .iter()
                    .zip(camera)
                    .map(|(a, b)| (a - b) * (a - b))
                    .sum::<f64>();
                if distance_squared >= SHADOW_RANGE * SHADOW_RANGE {
                    continue;
                }
                let Some(radius) = radius(models, entity)? else {
                    continue;
                };
                let before = out.vertices.len();
                append_disc(
                    content,
                    world,
                    texture,
                    origin,
                    position,
                    f64::from(radius),
                    distance_squared,
                    darken,
                    out,
                )?;
                casters += usize::from(out.vertices.len() > before);
            }
        }
        Ok(casters)
    }
}

#[expect(clippy::too_many_arguments, reason = "One disc's inputs")]
fn append_disc(
    content: &Content,
    world: &World,
    texture: crate::protocol::content::TextureRegion,
    origin: [f64; 3],
    position: [f64; 3],
    radius: f64,
    distance_squared: f64,
    darken: u8,
    out: &mut DynamicGeometry,
) -> Result<()> {
    if radius <= 0. || radius.is_nan() {
        return Ok(());
    }
    let low = [position[0] - radius, position[2] - radius];
    let high = [position[0] + radius, position[2] + radius];
    // Receivers: full blocks whose top is at or below the feet and less than
    // SHADOW_DROP below them.
    let top_y = (position[1] - 1. + 1e-4).floor() as i32;
    let bottom_y = (position[1] - SHADOW_DROP).floor() as i32;
    for x in low[0].floor() as i32..=high[0].floor() as i32 {
        for z in low[1].floor() as i32..=high[1].floor() as i32 {
            for y in bottom_y..=top_y {
                let Some(state) = world.block([x, y, z]) else {
                    continue;
                };
                if !content.info(state)?.collision_full {
                    continue;
                }
                let surface = f64::from(y + 1);
                let [sky, block] = world.light_levels([x, y + 1, z]);
                let level = block
                    .unwrap_or(0)
                    .max(sky.unwrap_or(0).saturating_sub(darken));
                let alpha = opacity(position[1] - surface, distance_squared, level);
                if alpha < MIN_ALPHA {
                    continue;
                }
                let x0 = f64::from(x).max(low[0]);
                let x1 = f64::from(x + 1).min(high[0]);
                let z0 = f64::from(z).max(low[1]);
                let z1 = f64::from(z + 1).min(high[1]);
                quad(
                    texture,
                    [x0, x1],
                    surface,
                    [z0, z1],
                    low,
                    2. * radius,
                    origin,
                    alpha,
                    out,
                )?;
            }
        }
    }
    Ok(())
}

/// One upward-facing piece of the disc; `uv` spans the whole disc square.
#[expect(clippy::too_many_arguments, reason = "One clipped disc piece")]
fn quad(
    texture: crate::protocol::content::TextureRegion,
    x: [f64; 2],
    y: f64,
    z: [f64; 2],
    disc: [f64; 2],
    size: f64,
    origin: [f64; 3],
    alpha: f32,
    out: &mut DynamicGeometry,
) -> Result<()> {
    // Counter-clockwise seen from above: (p1 - p0) x (p2 - p0) points up.
    let corners = [(x[0], z[0]), (x[0], z[1]), (x[1], z[1]), (x[1], z[0])];
    p::emit(
        corners.map(|(px, pz)| {
            [
                (px - origin[0]) as f32,
                (y - origin[1]) as f32,
                (pz - origin[2]) as f32,
            ]
        }),
        corners.map(|(px, pz)| [((px - disc[0]) / size) as f32, ((pz - disc[1]) / size) as f32]),
        [1.; 3],
        texture.layer,
        EntityMaterial::EntityShadow,
        alpha,
        [15.; 2],
        out,
    )?;
    // The shadow material samples a standalone clamped texture: the region
    // origin and size travel in the overlay channels.
    let first = out.vertices.len() - 4;
    for vertex in &mut out.vertices[first..] {
        vertex.overlay_uv = [texture.uv[0], texture.uv[1]];
        vertex.overlay_tint = [texture.uv[2] - texture.uv[0], texture.uv[3] - texture.uv[1], 0.];
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_opacity_fades_with_height_distance_and_darkness() {
        let full = opacity(0., 0., 15);
        assert!((full - SHADOW_OPACITY).abs() < 1e-6, "{full}");
        // Oracle golem-front: camera 8 blocks away darkens grass to ~0.63.
        let golem = opacity(0., 8. * 8. + 1.62 * 1.62, 15);
        assert!((golem - 0.37).abs() < 0.02, "{golem}");
        assert!(opacity(1., 0., 15) < full && opacity(2., 0., 15) == 0.);
        assert_eq!(opacity(0., SHADOW_RANGE * SHADOW_RANGE, 15), 0.);
        assert!(opacity(0., 0., 4) < 0.1 * full, "night ground hardly shows a shadow");
        assert_eq!(opacity(0., 0., 0), 0.);
    }

    #[test]
    fn shadow_radius_follows_the_oracle_sizes() {
        assert!((base_radius("minecraft:villager", 0.6) - 0.5).abs() < 1e-6);
        assert!((base_radius("minecraft:iron_golem", 1.4) - 0.7).abs() < 1e-6);
    }
}
