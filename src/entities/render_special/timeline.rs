use super::{Entity, Metadata, Models, p};
use crate::protocol::models::JavaRandom;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Debug, Clone, Copy)]
enum Kind {
    Other,
    Arrow,
    Boat,
    Cart,
    Tnt,
    Lightning,
    Fangs,
    Item,
    Crystal,
}
#[derive(Debug)]
pub(crate) struct State {
    kind: Kind,
    random: Rc<RefCell<JavaRandom>>,
    pub shake: i32,
    pub hurt: i32,
    pub hurt_direction: i32,
    pub damage: f32,
    pub rowing: [f32; 2],
    pub bubble_multiplier: f32,
    pub bubble_angle: f32,
    pub bubble_angle_old: f32,
    pub fuse: i32,
    pub lightning_seed: i64,
    lightning_life: i32,
    lightning_flashes: i32,
    pub discarded: bool,
    fang_started: bool,
    fang_life: i32,
    pub bob_offset: f32,
    pub crystal_offset: i32,
}
#[derive(Default)]
pub(crate) struct TickEffects {
    pub lightning_sound_pitches: Option<[f32; 2]>,
    pub sky_flash: bool,
}
impl State {
    pub fn new(entity: &Entity, random: Rc<RefCell<JavaRandom>>) -> Self {
        let name = entity.type_name.as_str();
        let kind = match name {
            "minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:trident" => Kind::Arrow,
            "minecraft:tnt" => Kind::Tnt,
            "minecraft:lightning_bolt" => Kind::Lightning,
            "minecraft:evoker_fangs" => Kind::Fangs,
            "minecraft:item" => Kind::Item,
            "minecraft:end_crystal" => Kind::Crystal,
            _ if name.ends_with("_boat") || name.ends_with("_raft") => Kind::Boat,
            _ if name.ends_with("minecart") => Kind::Cart,
            _ => Kind::Other,
        };
        let (lightning_seed, lightning_flashes) = if matches!(kind, Kind::Lightning) {
            let mut source = random.borrow_mut();
            (source.next_long(), source.next_int(3) as i32 + 1)
        } else {
            (0, 0)
        };
        let bob_offset = if matches!(kind, Kind::Item) {
            random.borrow_mut().next_float() * std::f32::consts::TAU
        } else {
            0.
        };
        let crystal_offset = if matches!(kind, Kind::Crystal) {
            random.borrow_mut().next_int(100000) as i32
        } else {
            0
        };
        Self {
            kind,
            random,
            shake: 0,
            hurt: 0,
            hurt_direction: 1,
            damage: 0.,
            rowing: [0.; 2],
            bubble_multiplier: 0.,
            bubble_angle: 0.,
            bubble_angle_old: 0.,
            fuse: 80,
            lightning_seed,
            lightning_life: 2,
            lightning_flashes,
            discarded: false,
            fang_started: false,
            fang_life: 22,
            bob_offset,
            crystal_offset,
        }
    }
    pub fn on_metadata(
        &mut self,
        models: &Models,
        entity: &Entity,
        received: &[bool; 256],
        age: i32,
    ) {
        let changed = |name| {
            models
                .accessor(&entity.type_name, name)
                .is_some_and(|id| received[usize::from(id)])
        };
        if matches!(self.kind, Kind::Arrow)
            && age > 0
            && self.shake <= 0
            && changed("IN_GROUND")
            && p::boolean(models, entity, "IN_GROUND")
        {
            self.shake = 7;
        }
        if matches!(self.kind, Kind::Boat | Kind::Cart) {
            if changed("DATA_ID_HURT") {
                self.hurt = p::number(models, entity, "DATA_ID_HURT").unwrap_or(0);
            }
            if changed("DATA_ID_HURTDIR") {
                self.hurt_direction = p::number(models, entity, "DATA_ID_HURTDIR").unwrap_or(1);
            }
            if changed("DATA_ID_DAMAGE") {
                self.damage = match p::value(models, entity, "DATA_ID_DAMAGE") {
                    Some(Metadata::Float(value)) => *value,
                    _ => 0.,
                };
            }
        }
        if matches!(self.kind, Kind::Tnt) && changed("DATA_FUSE_ID") {
            self.fuse = p::number(models, entity, "DATA_FUSE_ID").unwrap_or(80);
        }
    }
    pub fn on_status(&mut self, status: u8, silent: bool) -> Option<f32> {
        if matches!(self.kind, Kind::Fangs) && status == 4 {
            self.fang_started = true;
            if !silent {
                return Some(self.random.borrow_mut().next_float() * 0.2 + 0.85);
            }
        }
        None
    }
    // The caller invokes this only on the shared Source entity tick, after its
    // physical interpolation. Freeze/step/player exemptions stay in that owner.
    pub fn tick(
        &mut self,
        models: &Models,
        entity: &Entity,
        age: i32,
        has_controlling_passenger: bool,
    ) -> TickEffects {
        let mut effects = TickEffects::default();
        if self.shake > 0 {
            self.shake -= 1;
        }
        if matches!(self.kind, Kind::Boat | Kind::Cart) {
            if self.hurt > 0 {
                self.hurt -= 1;
            }
            if self.damage > 0. {
                self.damage -= 1.;
            }
        }
        match self.kind {
            Kind::Boat => {
                for (side, name) in ["DATA_ID_PADDLE_LEFT", "DATA_ID_PADDLE_RIGHT"]
                    .iter()
                    .enumerate()
                {
                    self.rowing[side] =
                        if has_controlling_passenger && p::boolean(models, entity, name) {
                            self.rowing[side] + 0.3926991
                        } else {
                            0.
                        };
                }
                self.bubble_multiplier = (self.bubble_multiplier
                    + if p::number(models, entity, "DATA_ID_BUBBLE_TIME").unwrap_or(0) > 0 {
                        0.05
                    } else {
                        -0.1
                    })
                .clamp(0., 1.);
                self.bubble_angle_old = self.bubble_angle;
                self.bubble_angle =
                    10. * ((0.5 * f64::from(age)).sin() as f32) * self.bubble_multiplier;
            }
            Kind::Tnt => {
                self.fuse -= 1;
                if self.fuse <= 0 {
                    self.discarded = true;
                }
            }
            Kind::Lightning => {
                if self.lightning_life == 2 {
                    let mut source = self.random.borrow_mut();
                    effects.lightning_sound_pitches = Some([
                        0.8 + source.next_float() * 0.2,
                        0.5 + source.next_float() * 0.2,
                    ]);
                }
                self.lightning_life -= 1;
                if self.lightning_life < 0 {
                    if self.lightning_flashes == 0 {
                        self.discarded = true;
                    } else if self.lightning_life < -(self.random.borrow_mut().next_int(10) as i32)
                    {
                        self.lightning_flashes -= 1;
                        self.lightning_life = 1;
                        self.lightning_seed = self.random.borrow_mut().next_long();
                    }
                }
                effects.sky_flash = self.lightning_life >= 0;
            }
            Kind::Fangs => {
                if self.fang_started {
                    self.fang_life -= 1;
                }
            }
            _ => {}
        }
        effects
    }
    pub fn fang_progress(&self, partial: f32) -> f32 {
        if !self.fang_started {
            0.
        } else {
            let remaining = self.fang_life - 2;
            if remaining <= 0 {
                1.
            } else {
                1. - (remaining as f32 - partial) / 20.
            }
        }
    }
    pub fn rowing_time(&self, side: usize, enabled: bool, partial: f32) -> f32 {
        if enabled {
            self.rowing[side] - 0.3926991 + 0.3926991 * partial.clamp(0., 1.)
        } else {
            0.
        }
    }
}
