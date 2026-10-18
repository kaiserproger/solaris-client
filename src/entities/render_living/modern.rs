use super::state::Timer;
use crate::player::math;

#[derive(Debug)]
pub(crate) struct BinaryAnimator {
    length: i32,
    ticks: i32,
    ticks_old: i32,
}
impl BinaryAnimator {
    const fn new(length: i32) -> Self {
        Self {
            length,
            ticks: 0,
            ticks_old: 0,
        }
    }
    pub fn tick(&mut self, active: bool) {
        self.ticks_old = self.ticks;
        if active {
            if self.ticks < self.length {
                self.ticks += 1;
            }
        } else if self.ticks > 0 {
            self.ticks -= 1;
        }
    }
    pub fn factor(&self, partial: f32) -> f32 {
        let factor = (self.ticks_old as f32 + partial * (self.ticks - self.ticks_old) as f32)
            / self.length as f32;
        -(math::cos(f64::from(std::f32::consts::PI * factor)) - 1.) / 2.
    }
}
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum AxolotlAnimation {
    Swim,
    Walk,
    WalkUnderWater,
    IdleUnderWater,
    IdleUnderWaterOnGround,
    IdleOnGround,
    PlayDead,
}
#[derive(Debug)]
pub(crate) struct Axolotl {
    animations: [Timer; 7],
    pub playing_dead: BinaryAnimator,
    pub in_water: BinaryAnimator,
    pub on_ground: BinaryAnimator,
    pub moving: BinaryAnimator,
}
impl Default for Axolotl {
    fn default() -> Self {
        Self {
            animations: [Timer::default(); 7],
            playing_dead: BinaryAnimator::new(10),
            in_water: BinaryAnimator::new(10),
            on_ground: BinaryAnimator::new(10),
            moving: BinaryAnimator::new(10),
        }
    }
}
impl Axolotl {
    pub fn animation(&self, kind: AxolotlAnimation) -> Timer {
        self.animations[kind as usize]
    }
    pub fn tick(
        &mut self,
        age: i32,
        baby: bool,
        playing_dead: bool,
        in_water: bool,
        on_ground: bool,
        moving: bool,
    ) {
        self.moving.tick(moving);
        if baby {
            use AxolotlAnimation::*;
            let selected = if playing_dead {
                PlayDead
            } else if moving {
                if in_water && !on_ground {
                    Swim
                } else if !in_water && on_ground {
                    Walk
                } else {
                    WalkUnderWater
                }
            } else if in_water && !on_ground {
                IdleUnderWater
            } else if in_water && on_ground {
                IdleUnderWaterOnGround
            } else {
                IdleOnGround
            } as usize;
            for (index, animation) in self.animations.iter_mut().enumerate() {
                animation.animate_when(index == selected, age);
            }
        } else {
            self.playing_dead.tick(playing_dead);
            self.in_water.tick(!playing_dead && in_water);
            self.on_ground.tick(!playing_dead && !in_water && on_ground);
        }
    }
}
#[derive(Debug, Default)]
pub(crate) struct Fox {
    interested: f32,
    interested_old: f32,
    crouch: f32,
    crouch_old: f32,
}
impl Fox {
    pub fn tick(&mut self, flags: i32) {
        self.interested_old = self.interested;
        self.interested += (if flags & 8 != 0 { 1. } else { 0. } - self.interested) * 0.4;
        self.crouch_old = self.crouch;
        self.crouch = if flags & 4 != 0 {
            (self.crouch + 0.2).min(5.)
        } else {
            0.
        };
    }
    pub fn head_roll(&self, partial: f32) -> f32 {
        (self.interested_old + partial * (self.interested - self.interested_old))
            * 0.11
            * std::f32::consts::PI
    }
    pub fn crouch(&self, partial: f32) -> f32 {
        self.crouch_old + partial * (self.crouch - self.crouch_old)
    }
}
