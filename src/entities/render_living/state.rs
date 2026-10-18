use super::super::{presentation as p, Entity, Metadata, Models};
use super::modern::{Axolotl, Fox};
use super::motion::{BodyRule, MotionFacts, WalkRule};
use crate::{
    player::math,
    protocol::{models::JavaRandom, Result},
};
use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

#[derive(Debug, Clone, Copy)]
pub(crate) struct Timer(i32);
impl Default for Timer {
    fn default() -> Self {
        Self(i32::MIN)
    }
}
impl Timer {
    pub fn start(&mut self, age: i32) {
        self.0 = age;
    }
    pub fn stop(&mut self) {
        self.0 = i32::MIN;
    }
    pub fn started(self) -> bool {
        self.0 != i32::MIN
    }
    pub fn start_if_stopped(&mut self, age: i32) {
        if !self.started() {
            self.start(age);
        }
    }
    pub fn animate_when(&mut self, condition: bool, age: i32) {
        if condition {
            self.start_if_stopped(age);
        } else {
            self.stop();
        }
    }
    pub fn fast_forward(&mut self, ticks: i32, scale: f32) {
        if self.started() {
            self.0 = self.0.wrapping_sub((ticks as f32 * scale) as i32);
        }
    }
    pub fn millis(self, age: f32) -> Option<i64> {
        self.started().then(|| ((age - self.0 as f32) * 50.) as i64)
    }
}
#[derive(Debug)]
pub(crate) struct Walk {
    speed_old: f32,
    speed: f32,
    position: f32,
    position_scale: f32,
}
impl Default for Walk {
    fn default() -> Self {
        Self {
            speed_old: 0.,
            speed: 0.,
            position: 0.,
            position_scale: 1.,
        }
    }
}
impl Walk {
    fn update(&mut self, target: f32, factor: f32, scale: f32) {
        self.speed_old = self.speed;
        self.speed += (target - self.speed) * factor;
        self.position += self.speed;
        self.position_scale = scale;
    }
    fn stop(&mut self) {
        self.speed_old = 0.;
        self.speed = 0.;
        self.position = 0.;
    }
    pub fn speed(&self, partial: f32) -> f32 {
        (self.speed_old + partial * (self.speed - self.speed_old)).min(1.)
    }
    pub fn position(&self, partial: f32) -> f32 {
        (self.position - self.speed * (1. - partial)) * self.position_scale
    }
    pub fn moving(&self) -> bool {
        self.speed > 1e-5
    }
}
#[derive(Debug)]
pub(crate) enum Modern {
    None,
    Bat {
        fly: Timer,
        rest: Timer,
    },
    CopperGolem {
        idle_start_tick: i32,
        idle: Timer,
        interactions: [Timer; 4],
    },
    Rabbit {
        idle_timeout: i32,
        jump_ticks: i32,
        jump_duration: i32,
        hop: Timer,
        head_tilt: Timer,
    },
    Axolotl(Axolotl),
    Fox(Fox),
    Frog {
        jump: Timer,
        croak: Timer,
        tongue: Timer,
        swim_idle: Timer,
    },
    Sniffer {
        happy: Timer,
        scent: Timer,
        sniff: Timer,
        dig: Timer,
        rise: Timer,
    },
    Armadillo {
        in_state_ticks: i64,
        peek_received: bool,
        roll_up: Timer,
        roll_out: Timer,
        peek: Timer,
    },
    Camel {
        idle_timeout: i32,
        sit: Timer,
        sit_pose: Timer,
        sit_up: Timer,
        idle: Timer,
        dash: Timer,
    },
    Warden {
        roar: Timer,
        sniff: Timer,
        emerge: Timer,
        dig: Timer,
        attack: Timer,
        sonic: Timer,
    },
    Breeze {
        idle: Timer,
        slide: Timer,
        slide_back: Timer,
        long_jump: Timer,
        shoot: Timer,
        inhale: Timer,
    },
    Creaking {
        attack: Timer,
        invulnerable: Timer,
        death: Timer,
        attack_remaining: i32,
        invulnerable_remaining: i32,
        eyes_glowing: bool,
        next_flicker: i32,
    },
}
impl Modern {
    fn new(kind: &str, random: &mut JavaRandom) -> Self {
        let timer = Timer::default();
        match kind {
            "minecraft:bat" => Self::Bat {
                fly: timer,
                rest: timer,
            },
            "minecraft:copper_golem" => Self::CopperGolem {
                idle_start_tick: 0,
                idle: timer,
                interactions: [timer; 4],
            },
            "minecraft:rabbit" => Self::Rabbit {
                idle_timeout: random.next_int(40) as i32 + 180,
                jump_ticks: 0,
                jump_duration: 0,
                hop: timer,
                head_tilt: timer,
            },
            "minecraft:axolotl" => Self::Axolotl(Axolotl::default()),
            "minecraft:fox" => Self::Fox(Fox::default()),
            "minecraft:frog" => Self::Frog {
                jump: timer,
                croak: timer,
                tongue: timer,
                swim_idle: timer,
            },
            "minecraft:sniffer" => Self::Sniffer {
                happy: timer,
                scent: timer,
                sniff: timer,
                dig: timer,
                rise: timer,
            },
            "minecraft:armadillo" => Self::Armadillo {
                in_state_ticks: 0,
                peek_received: false,
                roll_up: timer,
                roll_out: timer,
                peek: timer,
            },
            "minecraft:camel" | "minecraft:camel_husk" => Self::Camel {
                idle_timeout: 0,
                sit: timer,
                sit_pose: timer,
                sit_up: timer,
                idle: timer,
                dash: timer,
            },
            "minecraft:warden" => Self::Warden {
                roar: timer,
                sniff: timer,
                emerge: timer,
                dig: timer,
                attack: timer,
                sonic: timer,
            },
            "minecraft:breeze" => Self::Breeze {
                idle: timer,
                slide: timer,
                slide_back: timer,
                long_jump: timer,
                shoot: timer,
                inhale: timer,
            },
            "minecraft:creaking" => Self::Creaking {
                attack: timer,
                invulnerable: timer,
                death: timer,
                attack_remaining: 0,
                invulnerable_remaining: 0,
                eyes_glowing: false,
                next_flicker: 0,
            },
            _ => Self::None,
        }
    }
}
/// Facts computed by the shared Source tick owner. No clock or world query is duplicated here.
pub(crate) struct TickInput {
    pub age: i32,
    pub position: [f64; 3],
    pub yaw: f32,
    pub alive: bool,
    pub passenger: bool,
    pub baby: bool,
    pub visually_swimming: bool,
    pub in_water: bool,
    pub uses_y_for_walk: bool,
    pub rotation_changed_before_base_tick: bool,
    pub is_mob: bool,
    pub first_passenger_is_mob: bool,
    pub max_head_yaw: f32,
    pub max_body_head_yaw: f32,
    pub attack_active: bool,
    pub camel_sitting: bool,
    pub camel_visually_sitting: bool,
    pub camel_sitting_down: bool,
    pub camel_pose_transition: bool,
    pub camel_pose_time: i64,
}
#[derive(Debug)]
pub(crate) struct State {
    position_old: [f64; 3],
    body_rule: BodyRule,
    walk_rule: WalkRule,
    clamps_received_head: bool,
    pub body_yaw: f32,
    pub body_yaw_old: f32,
    pub head_yaw: f32,
    pub head_yaw_old: f32,
    pub hurt_time: i32,
    pub death_time: i32,
    stable_head_yaw: f32,
    stable_head_ticks: i32,
    pub walk: Walk,
    swim: f32,
    swim_old: f32,
    pub modern: Modern,
    pub avatar: Avatar,
    random: Rc<RefCell<JavaRandom>>,
}
impl State {
    pub fn new(
        kind: &str,
        position: [f64; 3],
        body_yaw: f32,
        head_yaw: f32,
        random: Rc<RefCell<JavaRandom>>,
        facts: &MotionFacts,
    ) -> Result<Self> {
        let modern = Modern::new(kind, &mut random.borrow_mut());
        Ok(Self {
            body_rule: facts.body_rule.ok_or("living Source body rule missing")?,
            walk_rule: facts.walk_rule.ok_or("living Source walk rule missing")?,
            clamps_received_head: kind == "minecraft:goat",
            position_old: position,
            body_yaw,
            body_yaw_old: body_yaw,
            head_yaw,
            head_yaw_old: head_yaw,
            stable_head_yaw: 0.,
            stable_head_ticks: 0,
            hurt_time: 0,
            death_time: 0,
            walk: Walk::default(),
            swim: 0.,
            swim_old: 0.,
            modern,
            avatar: Avatar::default(),
            random,
        })
    }
    pub fn swim(&self, partial: f32) -> f32 {
        self.swim_old + partial * (self.swim - self.swim_old)
    }
    pub fn body(&self, partial: f32) -> f32 {
        self.body_yaw_old + partial * wrap(self.body_yaw - self.body_yaw_old)
    }
    pub fn head(&self, partial: f32) -> f32 {
        self.head_yaw_old + partial * wrap(self.head_yaw - self.head_yaw_old)
    }
    pub fn on_head_rotation(&mut self, yaw: f32) {
        self.head_yaw = if self.clamps_received_head {
            self.body_yaw + wrap(yaw - self.body_yaw).clamp(-15., 15.)
        } else {
            yaw
        };
    }
    pub fn on_metadata(
        &mut self,
        models: &Models,
        entity: &Entity,
        changed: &[bool; 256],
        age: i32,
    ) {
        let received = |name| {
            models
                .accessor(&entity.type_name, name)
                .is_some_and(|id| changed[usize::from(id)])
        };
        let pose = p::number(models, entity, "DATA_POSE").unwrap_or(0);
        match &mut self.modern {
            Modern::Frog {
                jump,
                croak,
                tongue,
                ..
            } if received("DATA_POSE") => {
                for (timer, expected) in [(jump, 6), (croak, 8), (tongue, 9)] {
                    if pose == expected {
                        timer.start(age);
                    } else {
                        timer.stop();
                    }
                }
            }
            Modern::Sniffer {
                happy,
                scent,
                sniff,
                dig,
                rise,
            } if received("DATA_STATE") => {
                let state = p::number(models, entity, "DATA_STATE").unwrap_or(0);
                for (timer, expected) in [(happy, 1), (scent, 2), (sniff, 3), (dig, 5), (rise, 6)] {
                    timer.stop();
                    if state == expected {
                        timer.start_if_stopped(age);
                    }
                }
            }
            Modern::Armadillo { in_state_ticks, .. } if received("ARMADILLO_STATE") => {
                *in_state_ticks = 0
            }
            Modern::Warden {
                roar,
                sniff,
                emerge,
                dig,
                ..
            } if received("DATA_POSE") => {
                // The Source handler starts the selected state; it does not stop prior timers here.
                match pose {
                    11 => roar.start(age),
                    12 => sniff.start(age),
                    13 => emerge.start(age),
                    14 => dig.start(age),
                    _ => {}
                }
            }
            Modern::Breeze {
                idle,
                slide,
                long_jump,
                shoot,
                inhale,
                ..
            } if received("DATA_POSE") => {
                shoot.stop();
                idle.stop();
                inhale.stop();
                long_jump.stop();
                match pose {
                    16 => shoot.start_if_stopped(age),
                    17 => inhale.start_if_stopped(age),
                    15 => slide.start_if_stopped(age),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    pub fn on_status(&mut self, status: u8, age: i32) {
        match &mut self.modern {
            Modern::Rabbit {
                jump_ticks,
                jump_duration,
                ..
            } if status == 1 => {
                *jump_ticks = 0;
                *jump_duration = 15;
            }
            Modern::Armadillo { peek_received, .. } if status == 64 => *peek_received = true,
            Modern::Warden {
                roar,
                attack,
                sonic,
                ..
            } => match status {
                4 => {
                    roar.stop();
                    attack.start(age);
                }
                62 => sonic.start(age),
                _ => {}
            },
            Modern::Creaking {
                attack_remaining,
                invulnerable_remaining,
                ..
            } => match status {
                4 => *attack_remaining = 15,
                66 => *invulnerable_remaining = 8,
                _ => {}
            },
            _ => {}
        }
    }
    pub fn on_leash_change(&mut self) {
        if let Modern::Rabbit { head_tilt, .. } = &mut self.modern {
            head_tilt.stop();
        }
    }
    pub fn on_damage(&mut self) {
        self.hurt_time = 10;
    }
    pub fn tick(&mut self, models: &Models, entity: &Entity, input: &TickInput) {
        let delta = std::array::from_fn::<_, 3, _>(|i| input.position[i] - self.position_old[i]);
        self.body_yaw_old = self.body_yaw;
        self.head_yaw_old = self.head_yaw;
        self.on_head_rotation(entity.presented_transform(0, 0.).head_yaw);
        if self.hurt_time > 0 {
            self.hurt_time -= 1;
        }
        if !input.alive {
            self.death_time = self.death_time.wrapping_add(1);
        }
        if let Modern::Breeze {
            idle,
            slide,
            slide_back,
            long_jump,
            ..
        } = &mut self.modern
        {
            let age_before_super = input.age.wrapping_sub(1);
            let pose = p::number(models, entity, "DATA_POSE").unwrap_or(0);
            if pose == 6 {
                long_jump.start_if_stopped(age_before_super);
            }
            idle.start_if_stopped(age_before_super);
            if pose != 15 && slide.started() {
                slide_back.start(age_before_super);
                slide.stop();
            }
        }
        self.swim_old = self.swim;
        self.swim = if input.visually_swimming {
            (self.swim + 0.09).min(1.)
        } else {
            (self.swim - 0.09).max(0.)
        };
        // Frog.tick samples last tick's walk state before LivingEntity updates it.
        if let Modern::Frog { swim_idle, .. } = &mut self.modern {
            swim_idle.animate_when(
                input.in_water && !self.walk.moving(),
                input.age.wrapping_sub(1),
            );
        }
        // These baseTick overrides sample the previous WalkAnimationState, before aiStep.
        match &mut self.modern {
            Modern::Axolotl(axolotl) => axolotl.tick(
                input.age,
                input.baby,
                p::boolean(models, entity, "DATA_PLAYING_DEAD"),
                input.in_water,
                entity.on_ground,
                self.walk.moving() || input.rotation_changed_before_base_tick,
            ),
            Modern::Rabbit {
                idle_timeout,
                jump_ticks,
                hop,
                head_tilt,
                ..
            } => {
                let no_ai = p::number(models, entity, "DATA_MOB_FLAGS_ID").unwrap_or(0) & 1 != 0;
                if *idle_timeout <= 0 && entity.leash_value().is_none() && !no_ai {
                    *idle_timeout = self.random.borrow_mut().next_int(40) as i32 + 180;
                    head_tilt.start(input.age);
                } else if *jump_ticks > 0 {
                    hop.start_if_stopped(input.age);
                    head_tilt.stop();
                } else {
                    *idle_timeout = idle_timeout.wrapping_sub(1);
                    hop.stop();
                }
            }
            _ => {}
        }
        let distance = (delta[0] * delta[0]
            + delta[2] * delta[2]
            + if input.uses_y_for_walk {
                delta[1] * delta[1]
            } else {
                0.
            })
        .sqrt() as f32;
        if input.passenger || !input.alive {
            self.walk.stop();
        } else {
            let (target, factor, scale) = match self.walk_rule {
                WalkRule::Frog => (
                    if matches!(&self.modern, Modern::Frog { jump, .. } if jump.started()) {
                        0.
                    } else {
                        (distance * 25.).min(1.)
                    },
                    0.4,
                    if input.baby { 3. } else { 1. },
                ),
                WalkRule::Camel => (
                    if p::number(models, entity, "DATA_POSE").unwrap_or(0) == 0
                        && matches!(&self.modern, Modern::Camel { dash, .. } if !dash.started())
                    {
                        (distance * 6.).min(1.)
                    } else {
                        0.
                    },
                    0.2,
                    if input.baby { 3. } else { 1. },
                ),
                WalkRule::Creaking => ((distance * 25.).min(3.), 0.4, 1.),
                WalkRule::Standard => (
                    (distance * 4.).min(1.),
                    0.4,
                    if input.baby { 3. } else { 1. },
                ),
            };
            self.walk.update(target, factor, scale);
        }
        let body_enabled = match self.body_rule {
            BodyRule::Shulker => false,
            BodyRule::Armadillo => p::number(models, entity, "ARMADILLO_STATE").unwrap_or(0) == 0,
            BodyRule::Camel => !input.camel_sitting && !input.camel_pose_transition,
            BodyRule::Creaking => p::value(models, entity, "CAN_MOVE")
                .and_then(Metadata::boolean)
                .unwrap_or(true),
            _ => true,
        };
        if matches!(self.body_rule, BodyRule::ArmorStand) {
            self.body_yaw = input.yaw;
        } else if matches!(self.body_rule, BodyRule::Phantom) {
            self.head_yaw = self.body_yaw;
            self.body_yaw = input.yaw;
        } else if input.is_mob && body_enabled {
            if matches!(self.body_rule, BodyRule::HappyGhast) && !entity.passenger_ids.is_empty() {
                self.body_yaw = input.yaw;
                self.head_yaw = input.yaw;
            }
            if delta[0] * delta[0] + delta[2] * delta[2] > 2.500000277905201e-7 {
                self.body_yaw = input.yaw;
                self.head_yaw =
                    rotate_if_necessary(self.head_yaw, self.body_yaw, input.max_head_yaw);
                self.stable_head_yaw = self.head_yaw;
                self.stable_head_ticks = 0;
            } else if !input.first_passenger_is_mob {
                if (self.head_yaw - self.stable_head_yaw).abs() > 15. {
                    self.stable_head_ticks = 0;
                    self.stable_head_yaw = self.head_yaw;
                    self.body_yaw =
                        rotate_if_necessary(self.body_yaw, self.head_yaw, input.max_head_yaw);
                } else {
                    self.stable_head_ticks = self.stable_head_ticks.wrapping_add(1);
                    if self.stable_head_ticks > 10 {
                        let fraction = ((self.stable_head_ticks - 10) as f32 / 10.).clamp(0., 1.);
                        self.body_yaw = rotate_if_necessary(
                            self.body_yaw,
                            self.head_yaw,
                            input.max_head_yaw * (1. - fraction),
                        );
                    }
                }
            }
        } else if !input.is_mob {
            let mut target = self.body_yaw;
            if (delta[0] * delta[0] + delta[2] * delta[2]) as f32 > 0.0025000002 {
                let direction = math::atan2(delta[2], delta[0]) as f32 * 57.295776 - 90.;
                let difference = (wrap(input.yaw) - direction).abs();
                target = if difference > 95. && difference < 265. {
                    direction - 180.
                } else {
                    direction
                };
            }
            if input.attack_active {
                target = input.yaw;
            }
            self.body_yaw += wrap(target - self.body_yaw) * 0.3;
            let difference = wrap(input.yaw - self.body_yaw);
            if difference.abs() > input.max_body_head_yaw {
                self.body_yaw += difference
                    - if difference > 0. {
                        input.max_body_head_yaw
                    } else {
                        -input.max_body_head_yaw
                    };
            }
        }
        match &mut self.modern {
            Modern::Bat { fly, rest } => {
                let resting = p::number(models, entity, "DATA_ID_FLAGS").unwrap_or(0) & 1 != 0;
                fly.animate_when(!resting, input.age);
                rest.animate_when(resting, input.age);
            }
            Modern::Fox(fox) => fox.tick(p::number(models, entity, "DATA_FLAGS_ID").unwrap_or(0)),
            Modern::CopperGolem {
                idle_start_tick,
                idle,
                interactions,
            } => {
                if p::number(models, entity, "DATA_MOB_FLAGS_ID").unwrap_or(0) & 1 == 0 {
                    match p::number(models, entity, "COPPER_GOLEM_STATE").unwrap_or(0) {
                        0 => {
                            for animation in interactions {
                                animation.stop();
                            }
                            if *idle_start_tick == input.age {
                                idle.start(input.age);
                            } else if *idle_start_tick == 0 {
                                *idle_start_tick = input.age.wrapping_add(
                                    self.random.borrow_mut().next_int(40) as i32 + 200,
                                );
                            }
                            if input.age as f32 == *idle_start_tick as f32 + 10. {
                                *idle_start_tick = 0;
                            }
                        }
                        state @ 1..=4 => {
                            idle.stop();
                            *idle_start_tick = 0;
                            for (index, animation) in interactions.iter_mut().enumerate() {
                                animation.animate_when(index == (state - 1) as usize, input.age);
                            }
                        }
                        _ => {}
                    }
                }
            }
            Modern::Rabbit {
                jump_ticks,
                jump_duration,
                ..
            } => {
                if jump_ticks != jump_duration {
                    *jump_ticks = jump_ticks.wrapping_add(1);
                } else if *jump_duration != 0 {
                    *jump_ticks = 0;
                    *jump_duration = 0;
                }
            }
            Modern::Armadillo {
                in_state_ticks,
                peek_received,
                roll_up,
                roll_out,
                peek,
            } => {
                match p::number(models, entity, "ARMADILLO_STATE").unwrap_or(0) {
                    0 => {
                        roll_up.stop();
                        roll_out.stop();
                        peek.stop();
                    }
                    1 => {
                        roll_out.stop();
                        roll_up.start_if_stopped(input.age);
                        peek.stop();
                    }
                    3 => {
                        roll_out.start_if_stopped(input.age);
                        roll_up.stop();
                        peek.stop();
                    }
                    2 => {
                        roll_out.stop();
                        roll_up.stop();
                        if *peek_received {
                            peek.stop();
                            *peek_received = false;
                        }
                        if *in_state_ticks == 0 {
                            peek.start(input.age);
                            peek.fast_forward(50, 1.);
                        } else {
                            peek.start_if_stopped(input.age);
                        }
                    }
                    _ => {}
                }
                *in_state_ticks = in_state_ticks.wrapping_add(1);
            }
            Modern::Camel {
                idle_timeout,
                sit,
                sit_pose,
                sit_up,
                idle,
                dash,
            } => {
                if *idle_timeout <= 0 {
                    *idle_timeout = self.random.borrow_mut().next_int(40) as i32 + 80;
                    idle.start(input.age);
                } else {
                    *idle_timeout -= 1;
                }
                if input.camel_visually_sitting {
                    sit_up.stop();
                    dash.stop();
                    if input.camel_sitting_down {
                        sit.start_if_stopped(input.age);
                        sit_pose.stop();
                    } else {
                        sit.stop();
                        sit_pose.start_if_stopped(input.age);
                    }
                } else {
                    sit.stop();
                    sit_pose.stop();
                    dash.animate_when(p::boolean(models, entity, "DASH"), input.age);
                    sit_up.animate_when(
                        input.camel_pose_transition && input.camel_pose_time >= 0,
                        input.age,
                    );
                }
            }
            Modern::Creaking {
                attack,
                invulnerable,
                death,
                attack_remaining,
                invulnerable_remaining,
                eyes_glowing,
                next_flicker,
            } => {
                if *attack_remaining > 0 {
                    *attack_remaining -= 1;
                }
                if *invulnerable_remaining > 0 {
                    *invulnerable_remaining -= 1;
                }
                attack.animate_when(*attack_remaining > 0, input.age);
                invulnerable.animate_when(*invulnerable_remaining > 0, input.age);
                death.animate_when(p::boolean(models, entity, "IS_TEARING_DOWN"), input.age);
                if self.death_time > *next_flicker {
                    let low = if *eyes_glowing {
                        2
                    } else {
                        self.death_time / 4
                    };
                    let high = if *eyes_glowing {
                        8
                    } else {
                        self.death_time / 2
                    };
                    *next_flicker = self.death_time.wrapping_add(
                        low + self.random.borrow_mut().next_int((high - low + 1) as u32) as i32,
                    );
                    *eyes_glowing = !*eyes_glowing;
                }
            }
            _ => {}
        }
        if matches!(self.body_rule, BodyRule::Armadillo) && !body_enabled {
            self.head_yaw = self.body_yaw;
        }
        if matches!(self.body_rule, BodyRule::Camel) && !body_enabled {
            self.head_yaw = rotate_if_necessary(self.head_yaw, self.body_yaw, input.max_head_yaw);
        }
        self.position_old = input.position;
    }
}
pub(crate) fn wrap(angle: f32) -> f32 {
    let angle = angle % 360.;
    if angle >= 180. {
        angle - 360.
    } else if angle < -180. {
        angle + 360.
    } else {
        angle
    }
}
fn rotate_if_necessary(current: f32, target: f32, limit: f32) -> f32 {
    target - wrap(target - current).clamp(-limit, limit)
}
#[derive(Debug, Default)]
pub(crate) struct Avatar {
    pub velocity_old: [f64; 3],
    cloak: [f64; 3],
    cloak_old: [f64; 3],
    walk: f32,
    walk_old: f32,
    bob: f32,
    bob_old: f32,
}
impl Avatar {
    pub fn tick(&mut self, position: [f64; 3], velocity: [f64; 3]) {
        self.walk_old = self.walk;
        self.velocity_old = velocity;
        self.cloak_old = self.cloak;
        for i in 0..3 {
            let delta = position[i] - self.cloak[i];
            if delta > 10. || delta < -10. {
                self.cloak[i] = position[i];
                self.cloak_old[i] = position[i];
            } else {
                self.cloak[i] += delta * 0.25;
            }
        }
    }
    pub fn add_walk_distance(&mut self, added: f32) {
        self.walk += added;
    }
    pub fn update_bob(&mut self, target: f32) {
        self.bob_old = self.bob;
        self.bob += (target - self.bob) * 0.4;
    }
    pub fn reset_bob(&mut self) {
        self.bob_old = self.bob;
        self.bob = 0.;
    }
    pub fn cloak(&self, partial: f32) -> [f64; 3] {
        std::array::from_fn(|i| {
            self.cloak_old[i] + f64::from(partial) * (self.cloak[i] - self.cloak_old[i])
        })
    }
    pub fn bob(&self, partial: f32) -> f32 {
        self.bob_old + partial * (self.bob - self.bob_old)
    }
    pub fn walk(&self, partial: f32) -> f32 {
        self.walk_old + partial * (self.walk - self.walk_old)
    }
    pub fn backwards_walk(&self, partial: f32) -> f32 {
        -(self.walk + (self.walk - self.walk_old) * partial)
    }
}
