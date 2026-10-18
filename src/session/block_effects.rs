//! Bounded, client-local terrain fragments. Simulation advances on game ticks,
//! never render frames; sprites come from the broken state's baked block model.
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content, models::TintKind, world::World},
};

impl super::Session {
    pub(super) fn present_block_break(&mut self, event: i32, position: [i32; 3], data: i32) -> Result<bool> {
        if event != 2001 || !self.presentation || data < 0 {
            return Ok(false);
        }
        let content = self.content.as_ref().ok_or("content unavailable")?;
        if content.info(data as u32).is_err() {
            return Ok(false);
        }
        self.block_effects.burst(content, &self.world, position, data as u32, self.client_tick ^ self.packet_sequence)
    }
}

const MAX_FRAGMENTS: usize = 2048;
#[derive(Clone, Copy)]
struct Fragment {
    position: [f64; 3],
    previous: [f64; 3],
    velocity: [f64; 3],
    uv: [[f32; 2]; 4],
    layer: u32,
    color: [f32; 3],
    size: f32,
    remaining: u8,
}
#[derive(Default)]
pub(super) struct BlockEffects {
    fragments: Vec<Fragment>,
}
impl BlockEffects {
    pub fn clear(&mut self) {
        self.fragments.clear();
    }
    pub fn burst(&mut self, content: &Content, world: &World, position: [i32; 3], state: u32, seed: u64) -> Result<bool> {
        if content.block_is_air(state)? {
            return Ok(false);
        }
        let Some(model) = content.model_for(state)? else { return Ok(false) };
        // Prefer an untinted body face over a grass overlay. Every selected UV
        // remains a real fragment of this state, including resource-pack models.
        let mut sprite = None;
        model.for_each_quad(position, |quad| {
            if sprite.is_none() || sprite.is_some_and(|(_, _, tint)| tint != TintKind::None) && quad.tint == TintKind::None {
                sprite = Some((quad.uv, quad.texture_layer, quad.tint));
            }
        });
        let Some((source_uv, layer, tint)) = sprite else { return Ok(false) };
        let color = match tint {
            TintKind::None => [1.; 3],
            TintKind::Constant(color) => color,
            tint => {
                let Some(biome) = world.biome(position) else { return Ok(false) };
                let biome = content.registry_entry("minecraft:worldgen/biome", biome as i32)?;
                let colors = content.biome_colors(biome, f64::from(position[0]), f64::from(position[2]))?;
                match tint {
                    TintKind::Grass => colors.grass,
                    TintKind::Foliage => colors.foliage,
                    TintKind::DryFoliage => colors.dry_foliage,
                    TintKind::Water => colors.water,
                    TintKind::None | TintKind::Constant(_) => unreachable!(),
                }
            }
        }.map(|channel| channel * 0.6);
        let min = source_uv.into_iter().fold([f32::INFINITY; 2], |a, b| [a[0].min(b[0]), a[1].min(b[1])]);
        let max = source_uv.into_iter().fold([f32::NEG_INFINITY; 2], |a, b| [a[0].max(b[0]), a[1].max(b[1])]);
        if self.fragments.len() + 64 > MAX_FRAGMENTS {
            let remove = (self.fragments.len() + 64 - MAX_FRAGMENTS).min(self.fragments.len());
            self.fragments.drain(..remove);
        }
        let mut random = seed ^ 0xa0761d6478bd642f;
        let mut next = || {
            random ^= random << 13;
            random ^= random >> 7;
            random ^= random << 17;
            (random >> 40) as f32 / (1u32 << 24) as f32
        };
        for x in 0..4 {
            for y in 0..4 {
                for z in 0..4 {
                    let offset = [x, y, z].map(|n| (f64::from(n) + 0.5) / 4.);
                    let u = next() * 0.75;
                    let v = next() * 0.75;
                    let uv = [[u, v + 0.25], [u + 0.25, v + 0.25], [u + 0.25, v], [u, v]]
                        .map(|p| [min[0] + p[0] * (max[0] - min[0]), min[1] + p[1] * (max[1] - min[1])]);
                    self.fragments.push(Fragment {
                        position: std::array::from_fn(|i| f64::from(position[i]) + offset[i]),
                        previous: std::array::from_fn(|i| f64::from(position[i]) + offset[i]),
                        velocity: [(offset[0] - 0.5) * 0.22, 0.08 + next() as f64 * 0.12, (offset[2] - 0.5) * 0.22],
                        uv,
                        layer,
                        color,
                        size: 0.05 + next() * 0.05,
                        remaining: 20 + (next() * 20.) as u8,
                    });
                }
            }
        }
        Ok(true)
    }
    pub fn tick(&mut self, world: &World, content: &Content) {
        self.fragments.retain_mut(|fragment| {
            fragment.remaining = fragment.remaining.saturating_sub(1);
            if fragment.remaining == 0 { return false; }
            fragment.velocity[1] -= 0.04;
            let old = fragment.position;
            let mut next = std::array::from_fn(|i| old[i] + fragment.velocity[i]);
            let cell = next.map(|v| v.floor() as i32);
            if fragment.velocity[1] < 0.
                && let Some(state) = world.block(cell)
                && let Some(boxes) = content.collision_boxes(state)
            {
                for bounds in boxes {
                    let top = f64::from(cell[1]) + bounds[4];
                    if old[1] >= top && next[1] <= top
                        && next[0] >= f64::from(cell[0]) + bounds[0]
                        && next[0] <= f64::from(cell[0]) + bounds[3]
                        && next[2] >= f64::from(cell[2]) + bounds[2]
                        && next[2] <= f64::from(cell[2]) + bounds[5]
                    {
                        next[1] = top + 0.01;
                        fragment.velocity[1] = 0.;
                        fragment.velocity[0] *= 0.7;
                        fragment.velocity[2] *= 0.7;
                    }
                }
            }
            fragment.previous = old;
            fragment.position = next;
            fragment.velocity = fragment.velocity.map(|v| v * 0.98);
            true
        });
    }
    pub fn append(&self, world: &World, origin: [f64; 3], yaw: f32, pitch: f32, partial: f32, out: &mut DynamicGeometry) -> Result<()> {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let right = [cy, 0., sy];
        let up = [-sy * sp, cp, cy * sp];
        for fragment in &self.fragments {
            // Sample between the last two ticks so fragments glide, and shrink
            // them out over the final eight ticks instead of vanishing.
            let at = std::array::from_fn::<_, 3, _>(|i| {
                fragment.previous[i] + (fragment.position[i] - fragment.previous[i]) * f64::from(partial)
            });
            let fade = crate::motion::smoothstep((f32::from(fragment.remaining) - partial) / 8.);
            if fade <= 0. { continue; }
            let [sky, block] = world.light_levels(at.map(|v| v.floor() as i32));
            let Some((sky, block)) = sky.zip(block) else { continue };
            let center = std::array::from_fn::<_, 3, _>(|i| (at[i] - origin[i]) as f32);
            let positions = [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]].map(|[x, y]| {
                std::array::from_fn(|i| center[i] + fragment.size * fade * (right[i] * x + up[i] * y))
            });
            crate::entities::presentation_emit(positions, fragment.uv, fragment.color, fragment.layer,
                EntityMaterial::EntityCutout, 1., [f32::from(sky), f32::from(block)], out)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragment_burst_uses_block_uvs_gravity_and_bounded_lifetime() {
        let content = Content::test_cube();
        let mut effects = BlockEffects::default();
        assert!(effects.burst(&content, &World::new(0, 16), [0, 3, 0], 1, 9).unwrap());
        assert_eq!(effects.fragments.len(), 64);
        let first = effects.fragments[0];
        assert!(first.uv.iter().all(|p| p.iter().all(|v| (0. ..=1.).contains(v))));
        effects.tick(&World::new(0, 16), &content);
        assert!(effects.fragments[0].velocity[1] < first.velocity[1]);
        assert_ne!(effects.fragments[0].position, first.position);
        for _ in 0..40 { effects.tick(&World::new(0, 16), &content); }
        assert!(effects.fragments.is_empty());
    }
    #[test]
    fn burst_budget_and_reset_are_bounded() {
        let content = Content::test_cube();
        let mut effects = BlockEffects::default();
        for seed in 0..40 { effects.burst(&content, &World::new(0, 16), [0, 3, 0], 1, seed).unwrap(); }
        assert_eq!(effects.fragments.len(), MAX_FRAGMENTS);
        assert!(!effects.burst(&content, &World::new(0, 16), [0, 3, 0], 0, 1).unwrap());
        effects.clear();
        assert!(effects.fragments.is_empty());
    }
}
