//! Authored ambient torch particles: sample nearby blocks and emit brief flame
//! sprites and rising smoke. The rates and motion are local visual choices.
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{Result, content::Content, world::World},
};

const SAMPLES: usize = 384;
const MAX_PARTICLES: usize = 1024;
const SMOKE_FRAMES: u8 = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Flame(&'static str),
    Smoke,
}

#[derive(Clone, Copy)]
struct Particle {
    kind: Kind,
    position: [f64; 3],
    previous: [f64; 3],
    velocity: [f64; 3],
    size: f32,
    gray: f32,
    age: u16,
    lifetime: u16,
}

#[derive(Default)]
pub(super) struct TorchParticles {
    particles: Vec<Particle>,
    state: u64,
}

/// What a block emits and where, relative to the block's minimum corner.
fn emitter(name: &str, facing: Option<&str>) -> Option<(&'static str, [f64; 3])> {
    let flame = match name {
        "minecraft:torch" | "minecraft:wall_torch" => "minecraft:particle/flame",
        "minecraft:soul_torch" | "minecraft:soul_wall_torch" => "minecraft:particle/soul_fire_flame",
        "minecraft:copper_torch" | "minecraft:copper_wall_torch" => "minecraft:particle/copper_fire_flame",
        _ => return None,
    };
    if name.contains("wall_torch") {
        // The flame sits toward the open side: opposite of the block facing.
        let (dx, dz) = match facing? {
            "north" => (0., 1.),
            "south" => (0., -1.),
            "west" => (1., 0.),
            "east" => (-1., 0.),
            _ => return None,
        };
        Some((flame, [0.5 + 0.25 * dx, 0.9, 0.5 + 0.25 * dz]))
    } else {
        Some((flame, [0.5, 0.75, 0.5]))
    }
}

impl TorchParticles {
    pub fn clear(&mut self) {
        self.particles.clear();
    }
    fn random(&mut self) -> f64 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        (self.state >> 11) as f64 / (1u64 << 53) as f64
    }
    fn below(&mut self, limit: i32) -> i32 {
        (self.random() * f64::from(limit)) as i32
    }
    fn spawn(&mut self, flame: &'static str, at: [f64; 3]) {
        if self.particles.len() + 2 > MAX_PARTICLES {
            return;
        }
        for kind in [Kind::Smoke, Kind::Flame(flame)] {
            let lifetime = 10 + (self.random() * 10.) as u16;
            let jitter = |me: &mut Self| (me.random() - 0.5) * 0.04;
            let position = [at[0] + jitter(self), at[1] + jitter(self), at[2] + jitter(self)];
            let rise = if kind == Kind::Smoke { 0.006 } else { 0.002 };
            let velocity = [jitter(self) * 0.1, rise + self.random() * 0.003, jitter(self) * 0.1];
            let size = (0.075 + self.random() * 0.03) as f32
                * if kind == Kind::Smoke { 0.7 } else { 1. };
            let gray = (0.15 + self.random() * 0.2) as f32;
            self.particles.push(Particle {
                kind,
                position,
                previous: position,
                velocity,
                size,
                gray,
                age: 0,
                lifetime,
            });
        }
    }
    /// One game tick: age and move, then sample for new emitters around `player`.
    pub fn tick(&mut self, world: &World, content: &Content, player: [f64; 3], tick: u64) {
        self.particles.retain_mut(|p| {
            p.age += 1;
            if p.age >= p.lifetime {
                return false;
            }
            p.previous = p.position;
            if p.kind == Kind::Smoke {
                p.velocity[1] += 0.0008;
            }
            for axis in 0..3 {
                p.position[axis] += p.velocity[axis];
                p.velocity[axis] *= 0.98;
            }
            true
        });
        self.state ^= tick.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let center = player.map(|v| v.floor() as i32);
        for _ in 0..SAMPLES {
            let position = center.map(|c| c + self.below(12) - self.below(12));
            let Some(state) = world.block(position) else { continue };
            let Some(name) = content.name(state) else { continue };
            if !name.ends_with("torch") {
                continue;
            }
            let facing = content
                .state_properties(state)
                .and_then(|p| p.get("facing"))
                .map(String::as_str);
            if let Some((flame, offset)) = emitter(name, facing) {
                self.spawn(flame, std::array::from_fn(|i| f64::from(position[i]) + offset[i]));
            }
        }
    }
    pub fn append(
        &self,
        world: &World,
        content: &Content,
        origin: [f64; 3],
        [yaw, pitch]: [f32; 2],
        partial: f32,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        let (sy, cy) = yaw.to_radians().sin_cos();
        let (sp, cp) = pitch.to_radians().sin_cos();
        let right = [cy, 0., sy];
        let up = [-sy * sp, cp, cy * sp];
        for p in &self.particles {
            let at = std::array::from_fn::<_, 3, _>(|i| {
                p.previous[i] + (p.position[i] - p.previous[i]) * f64::from(partial)
            });
            let progress = ((f32::from(p.age) + partial) / f32::from(p.lifetime)).clamp(0., 1.);
            let [sky, block] = world.light_levels(at.map(|v| v.floor() as i32));
            let Some((sky, block)) = sky.zip(block) else { continue };
            let (texture, color, material, light, size) = match p.kind {
                Kind::Flame(name) => {
                    // Flames glow: lit by their own fire regardless of the cell.
                    let size = p.size * (1. - progress * 0.3);
                    (name.to_owned(), [1.; 3], EntityMaterial::EntityCutout, [f32::from(sky), f32::from(block).max(14.)], size)
                }
                Kind::Smoke => {
                    let frame = ((progress * f32::from(SMOKE_FRAMES)) as u8).min(SMOKE_FRAMES - 1);
                    (
                        format!("minecraft:particle/generic_{frame}"),
                        [p.gray; 3],
                        EntityMaterial::EntityTranslucent,
                        [f32::from(sky), f32::from(block)],
                        p.size,
                    )
                }
            };
            let region = content
                .texture(&texture)
                .ok_or_else(|| format!("missing mandatory particle texture {texture}"))?;
            let center = std::array::from_fn::<_, 3, _>(|i| (at[i] - origin[i]) as f32);
            let positions = [[-1., -1.], [1., -1.], [1., 1.], [-1., 1.]].map(|[x, y]| {
                std::array::from_fn(|i| center[i] + size * (right[i] * x + up[i] * y))
            });
            let [u0, v0, u1, v1] = region.uv;
            crate::entities::presentation_emit(
                positions,
                [[u0, v1], [u1, v1], [u1, v0], [u0, v0]],
                color,
                region.layer,
                material,
                1.,
                light,
                out,
            )?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floor_and_wall_torches_emit_near_the_wick() {
        let (flame, at) = emitter("minecraft:torch", None).unwrap();
        assert_eq!((flame, at), ("minecraft:particle/flame", [0.5, 0.75, 0.5]));
        // A wall torch facing north hangs on the wall to its south: the flame
        // sits on the wall side, toward the block's south edge.
        let (_, at) = emitter("minecraft:wall_torch", Some("north")).unwrap();
        assert!(at[2] > 0.5 && at[0] == 0.5);
        assert!(emitter("minecraft:wall_torch", Some("up")).is_none());
        assert!(emitter("minecraft:redstone_torch", None).is_none());
        assert_eq!(
            emitter("minecraft:soul_torch", None).unwrap().0,
            "minecraft:particle/soul_fire_flame"
        );
    }

    #[test]
    fn spawning_is_bounded_and_particles_expire() {
        let mut torches = TorchParticles { state: 1, ..Default::default() };
        for _ in 0..MAX_PARTICLES {
            torches.spawn("minecraft:particle/flame", [0.5; 3]);
        }
        assert!(torches.particles.len() <= MAX_PARTICLES);
        let world = World::new(0, 16);
        let content = Content::test_cube();
        for tick in 0..80 {
            torches.tick(&world, &content, [0.; 3], tick);
        }
        assert!(torches.particles.is_empty(), "particles expire");
    }
}
