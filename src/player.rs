//! Protocol-775 local player prediction and ordinary interaction owner.
//! Source pin: verified 26.1.2 client archive, ignored `.analysis/player-research/*.javap`,
//! C02 LocalPlayer/LivingEntity bytecode and entity-models PositionMoveRotation/Relative.
//! Prediction never writes received world blocks, inventory or remote entities.
use crate::protocol::packet_ids::play::serverbound as sb;
use crate::protocol::packet_ids::play::clientbound as cb;
mod adventure;
mod block_sounds;
pub(crate) mod collision;
mod interaction;
pub(crate) mod math;
mod movement;
mod use_on;
mod vehicle;
mod visual;
use crate::{
    entities::{Handle, Store as EntityStore},
    inventory::{InventoryStore, PacketCommand},
    protocol::{
        self,
        content::{Content, FluidKind},
        world::World,
        Reader, Result, Writer,
    },
};
pub use collision::Bounds;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, LazyLock},
};
pub(crate) use use_on::EntityUseState;
pub use use_on::{UiTrigger, UsePrediction};
pub(crate) use visual::camera_distance;

static EMPTY_FEATURES: LazyLock<Arc<BTreeSet<String>>> =
    LazyLock::new(|| Arc::new(BTreeSet::new()));

// ServerboundPlayerCommandPacket.Action, verified against the local 26.3 archive.
// Sneak state belongs to PLAYER_INPUT, not this enum.
#[derive(Clone, Copy)]
#[repr(i32)]
enum PlayerCommand {
    StartSprinting = 1,
    StopSprinting = 2,
    StartRidingJump = 3,
    OpenInventory = 5,
    StartFallFlying = 6,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pose {
    #[default]
    Standing,
    Crouching,
    Swimming,
    FallFlying,
    Sleeping,
    SpinAttack,
    Dying,
}
#[derive(Clone, Debug, Serialize)]
pub struct Abilities {
    pub invulnerable: bool,
    pub flying: bool,
    pub may_fly: bool,
    pub instabuild: bool,
    pub may_build: bool,
    pub flying_speed: f32,
    pub walking_speed: f32,
}
impl Default for Abilities {
    fn default() -> Self {
        Self {
            invulnerable: false,
            flying: false,
            may_fly: false,
            instabuild: false,
            may_build: true,
            flying_speed: 0.05,
            walking_speed: 0.1,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Effect {
    pub amplifier: u32,
    pub duration: i32,
    pub flags: u8,
    pub observed_tick: u64,
    pub revision: u64,
    pub generation: u64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Attribute {
    pub base: f64,
    pub value: f64,
    pub modifiers: BTreeMap<String, (f64, u8)>,
}
#[derive(Clone, Debug, Serialize)]
pub struct BlockHit {
    pub position: [i32; 3],
    pub face: i32,
    pub point: [f64; 3],
    pub state: u32,
    pub distance: f64,
    pub inside: bool,
}
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Hit {
    Block(BlockHit),
    Entity {
        handle: Handle,
        point: [f64; 3],
        distance: f64,
    },
}
#[derive(Clone, Copy, Debug, Default)]
pub struct HeldInput {
    pub flags: u8,
    pub attack: bool,
    pub use_item: bool,
    pub pick: bool,
    pub offhand: bool,
    pub drop: bool,
    pub swap_hands: bool,
}
impl HeldInput {
    pub fn from_keys(keys: &BTreeSet<String>, blocked: bool) -> Self {
        if blocked {
            return Self::default();
        }
        let mut flags = 0;
        for (i, name) in [
            "forward", "backward", "left", "right", "jump", "sneak", "sprint",
        ]
        .iter()
        .enumerate()
        {
            if keys.contains(*name) {
                flags |= 1 << i;
            }
        }
        Self {
            flags,
            attack: keys.contains("attack"),
            use_item: keys.contains("use"),
            pick: keys.contains("pick"),
            offhand: keys.contains("offhand"),
            drop: keys.contains("drop"),
            swap_hands: keys.contains("swap_hands"),
        }
    }
    pub fn has(self, bit: u8) -> bool {
        self.flags & (1 << bit) != 0
    }
    pub fn forward(self) -> f32 {
        u8::from(self.has(0)) as f32 - u8::from(self.has(1)) as f32
    }
    pub fn strafe(self) -> f32 {
        u8::from(self.has(2)) as f32 - u8::from(self.has(3)) as f32
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct InteractionContext {
    pub pose_revision: u64,
    pub inventory_revision: u64,
    pub target: Option<Hit>,
}
#[derive(Clone, Debug)]
pub enum InteractionIntent {
    Attack,
    Use { hand: u8 },
    Pick { include_data: bool },
    Drop { whole_stack: bool },
    SwapHands,
    StartGliding,
    ToggleFlight,
    OpenMountInventory,
    ReleaseAll,
}
/// Why an ordinary Attack cannot begin mining right now; `transient` means the
/// gate lifts by itself within a few client ticks (destroy delay, finalizing break).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AttackBlocker {
    pub reason: &'static str,
    pub transient: bool,
}

pub struct Player {
    pub position: [f64; 3],
    pub server_position: Option<[f64; 3]>,
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    visual: visual::FirstPerson,
    pub grounded: bool,
    pub horizontal_collision: bool,
    pub minor_horizontal_collision: bool,
    pub pose_known: bool,
    pub pose: Pose,
    pub entity_id: i32,
    sound_entity_generation: Option<u64>,
    pub game_mode: u8,
    pub health: Option<f32>,
    pub food: Option<i32>,
    pub saturation: Option<f32>,
    pub attributes: BTreeMap<String, Attribute>,
    pub abilities: Abilities,
    pub effects: BTreeMap<String, Effect>,
    pub corrections: u64,
    pub last_correction_distance: f64,
    pub movement_blocked: Option<String>,
    pub permission_level: Option<u8>,
    pub rain_at_position: Option<bool>,
    pub fishing_hook: Option<Handle>,
    pub uuid: Option<Arc<str>>,
    main_arm_right: bool,
    pub(crate) last_interaction_result: Option<&'static str>,
    enabled_features: Arc<BTreeSet<String>>,
    ordinary_ui: VecDeque<UiTrigger>,
    use_predictions: VecDeque<UsePrediction>,
    local_prediction_warnings: BTreeMap<Option<u32>, BTreeSet<String>>,
    interaction_random: u64,
    interaction_gaussian: Option<f64>,
    pub auto_spin_ticks: u32,
    pub pose_revision: u64,
    pub vehicle: Option<Handle>,
    vehicle_controlled: bool,
    pub sprinting: bool,
    pub swimming: bool,
    pub gliding: bool,
    pub fall_distance: f64,
    pub water_vision: f32,
    pub water_vision_ticks: u32,
    pub eye_in_water: bool,
    pub attack_ticks: u32,
    pub experience_level: i32,
    pub experience_progress: f32,
    pub experience_total: i32,
    pub hurt_tick: Option<u64>,
    pub hurt_direction: f32,
    pub death_message: Option<Value>,
    pub camera_entity: Option<i32>,
    sequence: i32,
    ack_sequence: i32,
    effect_revision: u64,
    pending_teleport: Option<(i32, [f64; 3], f32, f32)>,
    pending_vehicle_correction: Option<(Handle, [f64; 3], f32, f32)>,
    world_game_time: Option<i64>,
    fast_lava: bool,
    last_input: HeldInput,
    last_sent_input: u8,
    sent_sprinting: bool,
    stuck_multiplier: [f64; 3],
    jump_delay: u8,
    jump_trigger: u8,
    sprint_trigger: u8,
    use_delay: u8,
    miss_delay: u8,
    breaking: Option<interaction::Breaking>,
    break_delay: u8,
    step_distance: f32,
    next_step: f32,
    using: Option<interaction::UsingItem>,
    pending_use_release: Option<interaction::UsingItem>,
    cooldowns: BTreeMap<String, u64>,
    colliders: Vec<Bounds>,
    inside_movements: Vec<collision::Movement>,
    inside_visits: Vec<[i32; 3]>,
    inside_effects: Vec<collision::InsideVisit>,
    inside_block_state: collision::InsideBlockState,
    step_heights: Vec<f32>,
    last_sent_position: [f64; 3],
    last_sent_yaw: f32,
    last_sent_pitch: f32,
    last_sent_grounded: bool,
    last_sent_collision: bool,
    position_reminder: u8,
    vehicle_prediction: Option<vehicle::VehiclePrediction>,
    vehicle_seat_motion: Option<vehicle::SeatMotion>,
}
impl Default for Player {
    fn default() -> Self {
        Self {
            position: [0.; 3],
            server_position: None,
            velocity: [0.; 3],
            yaw: 0.,
            pitch: 0.,
            visual: visual::FirstPerson::default(),
            grounded: false,
            horizontal_collision: false,
            minor_horizontal_collision: false,
            pose_known: false,
            pose: Pose::Standing,
            entity_id: 0,
            sound_entity_generation: None,
            game_mode: 0,
            health: None,
            food: None,
            saturation: None,
            attributes: BTreeMap::new(),
            abilities: Abilities::default(),
            effects: BTreeMap::new(),
            corrections: 0,
            last_correction_distance: 0.,
            movement_blocked: None,
            permission_level: None,
            rain_at_position: None,
            fishing_hook: None,
            uuid: None,
            main_arm_right: true,
            last_interaction_result: None,
            enabled_features: Arc::clone(&EMPTY_FEATURES),
            ordinary_ui: VecDeque::with_capacity(16),
            use_predictions: VecDeque::with_capacity(256),
            local_prediction_warnings: BTreeMap::new(),
            interaction_random: 0,
            interaction_gaussian: None,
            auto_spin_ticks: 0,
            pose_revision: 0,
            vehicle: None,
            vehicle_controlled: false,
            sprinting: false,
            swimming: false,
            gliding: false,
            fall_distance: 0.,
            water_vision: 0.,
            water_vision_ticks: 0,
            eye_in_water: false,
            attack_ticks: 0,
            experience_level: 0,
            experience_progress: 0.,
            experience_total: 0,
            hurt_tick: None,
            hurt_direction: 0.,
            death_message: None,
            camera_entity: None,
            sequence: 0,
            ack_sequence: 0,
            effect_revision: 0,
            pending_teleport: None,
            pending_vehicle_correction: None,
            world_game_time: None,
            fast_lava: false,
            last_input: HeldInput::default(),
            last_sent_input: 0,
            sent_sprinting: false,
            stuck_multiplier: [0.; 3],
            jump_delay: 0,
            jump_trigger: 0,
            sprint_trigger: 0,
            use_delay: 0,
            miss_delay: 0,
            breaking: None,
            break_delay: 0,
            step_distance: 0.,
            next_step: 1.,
            using: None,
            pending_use_release: None,
            cooldowns: BTreeMap::new(),
            colliders: Vec::with_capacity(128),
            inside_movements: Vec::with_capacity(16),
            inside_visits: Vec::with_capacity(128),
            inside_effects: Vec::with_capacity(128),
            inside_block_state: collision::InsideBlockState::default(),
            step_heights: Vec::with_capacity(16),
            last_sent_position: [0.; 3],
            last_sent_yaw: 0.,
            last_sent_pitch: 0.,
            last_sent_grounded: false,
            last_sent_collision: false,
            position_reminder: 0,
            vehicle_prediction: None,
            vehicle_seat_motion: None,
        }
    }
}
impl Player {
    pub fn main_arm_right(&self) -> bool {
        self.main_arm_right
    }
    pub fn set_main_arm_right(&mut self, right: bool) {
        self.main_arm_right = right;
    }
    pub fn last_interaction_result(&self) -> Option<&'static str> {
        self.last_interaction_result
    }
    pub fn update_environment_context(&mut self, world: &World) -> Result<()> {
        self.fast_lava = world.dimension_fast_lava()?;
        Ok(())
    }
    pub fn new() -> Self {
        Self::default()
    }
    pub(crate) fn bind_sound_entity(&mut self, entity_id: i32, generation: u64) {
        self.entity_id = entity_id;
        self.sound_entity_generation = Some(generation);
    }
    pub fn sound_entity_generation(&self) -> Result<u64> {
        self.sound_entity_generation
            .ok_or_else(|| "local sound entity lifetime is unbound".into())
    }
    pub fn set_enabled_features(&mut self, features: Arc<BTreeSet<String>>) {
        self.enabled_features = features;
    }
    pub fn take_ui_trigger(&mut self) -> Option<UiTrigger> {
        self.ordinary_ui.pop_front()
    }
    pub fn take_use_prediction(&mut self) -> Option<UsePrediction> {
        self.use_predictions.pop_front()
    }
    pub fn set_interaction_seed(&mut self, seed: u64) {
        self.interaction_random = (seed ^ 0x5deece66d) & ((1 << 48) - 1);
        self.interaction_gaussian = None;
    }
    pub fn set_mode(&mut self, mode: u8) -> Result<()> {
        if mode > 3 {
            return Err("invalid_game_mode".into());
        }
        self.game_mode = mode;
        self.abilities.may_build = mode == 0 || mode == 1;
        self.abilities.instabuild = mode == 1;
        self.abilities.invulnerable = mode == 1 || mode == 3;
        self.abilities.may_fly = mode == 1 || mode == 3;
        if mode == 3 {
            self.abilities.flying = true;
        } else if !self.abilities.may_fly {
            self.abilities.flying = false;
        }
        self.sprinting = false;
        self.gliding = false;
        self.swimming = false;
        Ok(())
    }
    pub fn reset_dimension(&mut self, keep_attributes: bool) {
        self.visual.reset();
        self.pose_known = false;
        self.server_position = None;
        self.velocity = [0.; 3];
        self.minor_horizontal_collision = false;
        self.vehicle = None;
        self.vehicle_controlled = false;
        self.vehicle_prediction = None;
        self.vehicle_seat_motion = None;
        self.breaking = None;
        self.step_distance = 0.;
        self.next_step = 1.;
        self.using = None;
        self.pending_use_release = None;
        self.ordinary_ui.clear();
        self.use_predictions.clear();
        self.inside_movements.clear();
        self.inside_visits.clear();
        self.inside_effects.clear();
        self.inside_block_state = collision::InsideBlockState::default();
        self.rain_at_position = None;
        self.fishing_hook = None;
        self.auto_spin_ticks = 0;
        self.last_input = HeldInput::default();
        self.last_sent_input = 0;
        self.sent_sprinting = false;
        self.stuck_multiplier = [0.; 3];
        self.pending_teleport = None;
        self.pending_vehicle_correction = None;
        self.world_game_time = None;
        self.fast_lava = false;
        self.sequence = 0;
        self.ack_sequence = 0;
        self.jump_delay = 0;
        self.jump_trigger = 0;
        self.sprint_trigger = 0;
        self.sprinting = false;
        self.swimming = false;
        self.gliding = false;
        self.pose = Pose::Standing;
        self.fall_distance = 0.;
        self.water_vision_ticks = 0;
        self.water_vision = 0.;
        self.eye_in_water = false;
        self.hurt_tick = None;
        self.hurt_direction = 0.;
        self.death_message = None;
        self.cooldowns.clear();
        self.effects.clear();
        if !keep_attributes {
            self.attributes.clear();
        }
        self.pose_revision = self.pose_revision.wrapping_add(1);
    }
    /// Seed the local player from the activated DefaultAttributes supplier.
    /// Unchanged defaults need not arrive in an UpdateAttributes packet.
    pub(crate) fn init_vanilla_defaults<'a>(
        &mut self,
        defaults: impl IntoIterator<Item = (&'a str, f64)>,
    ) {
        for (name, base) in defaults {
            self.attributes.insert(
                name.to_owned(),
                Attribute {
                    base,
                    value: base,
                    modifiers: BTreeMap::new(),
                },
            );
        }
    }
    pub fn attribute(&self, name: &str) -> f64 {
        self.attributes
            .get(name)
            .map(|a| a.value)
            .unwrap_or_else(|| match name {
                "minecraft:movement_speed" => f64::from(0.1f32),
                "minecraft:gravity" => 0.08,
                "minecraft:jump_strength" => 0.42,
                "minecraft:step_height" => 0.6,
                "minecraft:scale" => 1.,
                "minecraft:sneaking_speed" => 0.3,
                "minecraft:block_interaction_range" => 4.5,
                "minecraft:entity_interaction_range" => 3.,
                "minecraft:submerged_mining_speed" => 0.2,
                "minecraft:block_break_speed" => 1.,
                "minecraft:attack_speed" => 4.,
                "minecraft:attack_damage" => 1.,
                "minecraft:movement_efficiency"
                | "minecraft:water_movement_efficiency"
                | "minecraft:mining_efficiency" => 0.,
                _ => 0.,
            })
    }
    pub fn effect_duration(&self, name: &str, tick: u64) -> Option<i32> {
        self.effects.get(name).and_then(|e| {
            if e.duration == -1 {
                Some(-1)
            } else {
                let n = i64::from(e.duration)
                    - tick.saturating_sub(e.observed_tick).min(i32::MAX as u64) as i64;
                Some(n.max(0) as i32)
            }
        })
    }
    fn amplifier(&self, name: &str, _tick: u64) -> Option<u32> {
        self.effects.get(name).map(|e| e.amplifier)
    }
    pub fn eye_position(&self) -> [f64; 3] {
        let height = match self.pose {
            Pose::Standing => 1.62f32,
            Pose::Crouching => 1.27f32,
            Pose::Swimming | Pose::FallFlying | Pose::SpinAttack => 0.4f32,
            Pose::Sleeping => 0.2f32,
            Pose::Dying => 1.62f32,
        };
        [
            self.position[0],
            self.position[1] + f64::from(height) * self.attribute("minecraft:scale"),
            self.position[2],
        ]
    }
    pub fn bounds(&self) -> Bounds {
        self.bounds_for(self.pose)
    }
    fn bounds_for(&self, pose: Pose) -> Bounds {
        let scale = self.attribute("minecraft:scale");
        let width = f64::from(0.6f32) * scale;
        let height = f64::from(match pose {
            Pose::Standing | Pose::Dying => 1.8f32,
            Pose::Crouching => 1.5,
            Pose::Swimming | Pose::FallFlying | Pose::SpinAttack => 0.6,
            Pose::Sleeping => 0.2,
        }) * scale;
        Bounds {
            min: [
                self.position[0] - width / 2.,
                self.position[1],
                self.position[2] - width / 2.,
            ],
            max: [
                self.position[0] + width / 2.,
                self.position[1] + height,
                self.position[2] + width / 2.,
            ],
        }
    }
    pub fn look(&mut self, yaw: f32, pitch: f32) -> Result<()> {
        if !yaw.is_finite() || !pitch.is_finite() {
            return Err("nonfinite_look".into());
        }
        self.yaw = yaw;
        self.pitch = pitch.clamp(-90., 90.);
        self.pose_revision = self.pose_revision.wrapping_add(1);
        Ok(())
    }
    pub fn look_direction(&self) -> [f64; 3] {
        math::view(self.pitch, self.yaw)
    }
    pub fn eye_medium(&self, world: &World, content: &Content) -> Result<Option<FluidKind>> {
        let eye = self.eye_position();
        let p = eye.map(|v| v.floor() as i32);
        let state = world.block(p).ok_or("unloaded_eye_environment")?;
        let Some(fluid) = content.info(state)?.fluid else {
            return Ok(None);
        };
        let above = world
            .block([p[0], p[1] + 1, p[2]])
            .map(|s| content.info(s))
            .transpose()?
            .and_then(|s| s.fluid);
        let height = if above.is_some_and(|f| f.kind == fluid.kind) {
            1.
        } else {
            f64::from(fluid.height())
        };
        Ok((eye[1] < f64::from(p[1]) + height).then_some(fluid.kind))
    }
    pub fn apply(
        &mut self,
        id: i32,
        r: &mut Reader<'_>,
        content: &Content,
        tick: u64,
    ) -> Result<bool> {
        let start = r.offset;
        match id {
            cb::BLOCK_CHANGED_ACK => {
                self.ack_sequence = r.var()?;
                if self.ack_sequence > self.sequence {
                    return Err("future_prediction_acknowledgement".into());
                }
            }
            cb::PLAYER_POSITION => {
                let teleport = r.var()?;
                let position = [r.f64()?, r.f64()?, r.f64()?];
                let velocity = [r.f64()?, r.f64()?, r.f64()?];
                let yaw = r.f32()?;
                let pitch = r.f32()?;
                let flags = r.i32()?;
                r.done()?;
                self.correct(position, velocity, yaw, pitch, flags)?;
                self.pending_teleport = Some((teleport, self.position, self.yaw, self.pitch));
            }
            cb::PLAYER_ROTATION => {
                self.yaw = r.f32()?;
                self.pitch = r.f32()?.clamp(-90., 90.);
                self.pose_revision = self.pose_revision.wrapping_add(1);
            }
            cb::MOVE_VEHICLE => {
                let position = [r.f64()?, r.f64()?, r.f64()?];
                let yaw = r.f32()?;
                let pitch = r.f32()?;
                self.correct_vehicle(position, yaw, pitch)?;
            }
            cb::PLAYER_ABILITIES => {
                let flags = r.u8()?;
                self.abilities.invulnerable = flags & 1 != 0;
                self.abilities.flying = flags & 2 != 0;
                self.abilities.may_fly = flags & 4 != 0;
                self.abilities.instabuild = flags & 8 != 0;
                self.abilities.flying_speed = r.f32()?;
                self.abilities.walking_speed = r.f32()?;
                if self.abilities.flying_speed < 0. || self.abilities.walking_speed < 0. {
                    return Err("negative_ability_speed".into());
                }
            }
            cb::SET_ENTITY_MOTION | cb::UPDATE_ATTRIBUTES | cb::UPDATE_MOB_EFFECT | cb::REMOVE_MOB_EFFECT => {
                if r.var()? != self.entity_id {
                    r.offset = start;
                    return Ok(false);
                }
                match id {
                    cb::SET_ENTITY_MOTION => self.velocity = read_velocity(r)?,
                    cb::UPDATE_ATTRIBUTES => {
                        let count = r.count(128)?;
                        for _ in 0..count {
                            let index = r.count(65536)? as i32;
                            let name = content
                                .registry_name("minecraft:attribute", index)?
                                .to_owned();
                            let base = r.f64()?;
                            let n = r.count(128)?;
                            let mut modifiers = BTreeMap::new();
                            for _ in 0..n {
                                let key = r.string(32767)?;
                                let amount = r.f64()?;
                                let operation = r.count(2)? as u8;
                                if modifiers.insert(key, (amount, operation)).is_some() {
                                    return Err("duplicate_attribute_modifier".into());
                                }
                            }
                            let value = attribute_value(
                                base,
                                &modifiers,
                                name == "minecraft:movement_speed",
                            );
                            let entry = content.registry_entry("minecraft:attribute", index)?;
                            let value = value.clamp(
                                entry.get("min").and_then(Value::as_f64).unwrap_or(f64::MIN),
                                entry.get("max").and_then(Value::as_f64).unwrap_or(f64::MAX),
                            );
                            self.attributes.insert(
                                name,
                                Attribute {
                                    base,
                                    value,
                                    modifiers,
                                },
                            );
                        }
                    }
                    cb::UPDATE_MOB_EFFECT => {
                        let index = r.count(65536)? as i32;
                        let name = content
                            .registry_name("minecraft:mob_effect", index)?
                            .to_owned();
                        let amplifier = r.count(255)? as u32;
                        let duration = r.var()?;
                        if duration < -1 {
                            return Err("invalid_effect_duration".into());
                        }
                        let flags = r.u8()?;
                        self.effect_revision = self
                            .effect_revision
                            .checked_add(1)
                            .ok_or("effect_revision_overflow")?;
                        let generation = self
                            .effects
                            .get(&name)
                            .map_or(self.effect_revision, |previous| previous.generation);
                        self.effects.insert(
                            name,
                            Effect {
                                amplifier,
                                duration,
                                flags,
                                observed_tick: tick,
                                revision: self.effect_revision,
                                generation,
                            },
                        );
                    }
                    cb::REMOVE_MOB_EFFECT => {
                        let index = r.count(65536)? as i32;
                        let name = content.registry_name("minecraft:mob_effect", index)?;
                        self.effects.remove(name);
                    }
                    _ => unreachable!(),
                }
            }
            cb::COOLDOWN => {
                let group = r.string(32767)?;
                let duration = r.count(i32::MAX as usize)? as u64;
                if duration == 0 {
                    self.cooldowns.remove(&group);
                } else {
                    if self.cooldowns.len() >= 4096 && !self.cooldowns.contains_key(&group) {
                        return Err("item_cooldown_bound".into());
                    }
                    self.cooldowns.insert(group, tick.saturating_add(duration));
                }
            }
            cb::SET_EXPERIENCE => {
                self.experience_progress = r.f32()?;
                self.experience_level = r.var()?;
                self.experience_total = r.var()?;
            }
            cb::SET_HEALTH => {
                self.health = Some(r.f32()?);
                self.food = Some(r.var()?);
                self.saturation = Some(r.f32()?);
            }
            cb::DAMAGE_EVENT => {
                if r.var()? != self.entity_id {
                    r.offset = start;
                    return Ok(false);
                }
                r.count(65535)?; // Damage type holder, followed by optional entity IDs.
                r.var()?.checked_sub(1).ok_or("damage cause id underflow")?;
                r.var()?
                    .checked_sub(1)
                    .ok_or("damage direct id underflow")?;
                if r.bool()? {
                    r.f64()?;
                    r.f64()?;
                    r.f64()?;
                }
                r.done()?;
                // LivingEntity.handleDamageEvent restarts hurtTime without changing hurtDir.
                self.hurt_tick = Some(tick);
            }
            cb::HURT_ANIMATION => {
                if r.var()? != self.entity_id {
                    r.offset = start;
                    return Ok(false);
                }
                let direction = r.f32()?;
                r.done()?;
                self.hurt_direction = direction;
                self.hurt_tick = Some(tick);
            }
            cb::PLAYER_COMBAT_KILL => {
                if r.var()? != self.entity_id {
                    r.offset = start;
                    return Ok(false);
                }
                self.death_message = Some(r.nbt()?);
                self.health = Some(0.);
            }
            cb::EXPLODE => {
                // ClientboundExplodePacket (26.1.2): Vec3 center, float radius, INT block
                // count, Optional<Vec3> player knockback, then particle/sound/weighted
                // block-particle presentation. ClientPacketListener.handleExplosion applies
                // only the knockback to gameplay (LocalPlayer.addDeltaMovement); removed
                // blocks arrive as ordinary block updates. The presentation tail is
                // unsupported (as other particle packets), so it is not decoded.
                for _ in 0..3 {
                    r.f64()?;
                }
                r.f32()?;
                r.i32()?;
                if r.bool()? {
                    for axis in 0..3 {
                        self.velocity[axis] += r.f64()?;
                    }
                }
                return Ok(true);
            }
            cb::SET_CAMERA => {
                self.camera_entity = Some(r.var()?);
            }
            _ => return Ok(false),
        }
        r.done()?;
        Ok(true)
    }
    pub fn correct(
        &mut self,
        position: [f64; 3],
        delta: [f64; 3],
        yaw: f32,
        pitch: f32,
        flags: i32,
    ) -> Result<()> {
        if flags & !511 != 0 {
            return Err("unknown_relative_teleport_flags".into());
        }
        let target = std::array::from_fn(|i| {
            position[i]
                + if flags & (1 << i) != 0 {
                    self.position[i]
                } else {
                    0.
                }
        });
        let y = if flags & 8 != 0 { self.yaw + yaw } else { yaw };
        let p = (if flags & 16 != 0 {
            self.pitch + pitch
        } else {
            pitch
        })
        .clamp(-90., 90.);
        let mut old_velocity = self.velocity;
        if flags & 256 != 0 {
            // PositionMoveRotation: rotate old delta around X, then Y, before relative addition.
            let xrot = f64::from((f64::from(self.pitch - p).to_radians()) as f32);
            let yrot = f64::from((f64::from(self.yaw - y).to_radians()) as f32);
            let (s, c) = (f64::from(math::sin(xrot)), f64::from(math::cos(xrot)));
            old_velocity = [
                old_velocity[0],
                old_velocity[1] * c + old_velocity[2] * s,
                old_velocity[2] * c - old_velocity[1] * s,
            ];
            let (s, c) = (f64::from(math::sin(yrot)), f64::from(math::cos(yrot)));
            old_velocity = [
                old_velocity[0] * c + old_velocity[2] * s,
                old_velocity[1],
                old_velocity[2] * c - old_velocity[0] * s,
            ];
        }
        if self.pose_known {
            self.corrections = self.corrections.saturating_add(1);
            self.last_correction_distance = (0..3)
                .map(|i| (target[i] - self.position[i]).powi(2))
                .sum::<f64>()
                .sqrt();
        }
        self.position = target;
        self.server_position = Some(target);
        self.velocity = std::array::from_fn(|i| {
            delta[i]
                + if flags & (1 << (i + 5)) != 0 {
                    old_velocity[i]
                } else {
                    0.
                }
        });
        self.yaw = y;
        self.pitch = p;
        self.grounded = false;
        self.pose_known = true;
        self.pose_revision = self.pose_revision.wrapping_add(1);
        self.position_reminder = 20;
        Ok(())
    }
    pub fn drain_commands(&mut self, out: &mut Vec<PacketCommand>) {
        if let Some((teleport, position, yaw, pitch)) = self.pending_teleport.take() {
            let mut w = Writer::default();
            w.var(teleport);
            for coordinate in position {
                w.f64(coordinate);
            }
            w.f32(yaw);
            w.f32(pitch);
            out.push(PacketCommand::new(sb::ACCEPT_TELEPORTATION, w));
            out.push(PacketCommand::new(sb::PLAYER_LOADED, Writer::default()));
            self.full_movement(out);
        }
        self.drain_vehicle_correction(out);
    }
    fn full_movement(&mut self, out: &mut Vec<PacketCommand>) {
        let mut w = Writer::default();
        for p in self.position {
            w.f64(p);
        }
        w.f32(self.yaw);
        w.f32(self.pitch);
        w.byte(u8::from(self.grounded) | (u8::from(self.horizontal_collision) << 1));
        out.push(PacketCommand::new(sb::MOVE_PLAYER_POS_ROT, w));
        self.last_sent_position = self.position;
        self.last_sent_yaw = self.yaw;
        self.last_sent_pitch = self.pitch;
        self.last_sent_grounded = self.grounded;
        self.last_sent_collision = self.horizontal_collision;
        self.position_reminder = 0;
    }
    /// Local 26.1.2 GameRenderer.bobHurt: remaining hurtTime / hurtDuration,
    /// fourth-power sine envelope, negative Z roll scaled by the accessibility option.
    /// LivingEntity.animateHurt and handleDamageEvent both reset the duration to ten ticks.
    pub fn hurt_tilt_deg(&self, tick: u64, partial_tick: f32, strength: f64) -> f32 {
        let Some(hurt_tick) = self.hurt_tick else {
            return 0.;
        };
        let elapsed = tick.saturating_sub(hurt_tick);
        if elapsed >= 10 {
            return 0.;
        }
        let remaining = (10 - elapsed) as f32 - partial_tick;
        if remaining <= 0. {
            return 0.;
        }
        let progress = remaining / 10.;
        let wave = (progress * progress * progress * progress * std::f32::consts::PI).sin();
        (-f64::from(wave) * 14. * strength) as f32
    }
    pub fn tick(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        keys: &BTreeSet<String>,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        self.drain_commands(out);
        if !self.pose_known {
            return Ok(());
        }
        let input = HeldInput::from_keys(
            keys,
            inventory.screen_open
                || self.health.is_some_and(|h| h <= 0.)
                || self.pose == Pose::Sleeping,
        );
        self.sync_input(input, out);
        let previous_position = self.position;
        self.movement_blocked = None;
        if let Err(e) = self.predict(world, content, inventory, entities, input, tick, out) {
            self.movement_blocked = Some(e);
            self.velocity = [0.; 3];
        }
        let item_id = self.using.as_ref().map(|using| using.item_id);
        let owner = item_id.and_then(|id| content.item_use_owner(id, false).ok())
            .unwrap_or("interaction tick");
        if self.local_prediction(item_id, owner, |player| {
            player.interaction_tick(world, content, inventory, entities, input, tick, out)
        }).is_none() {
            self.stop_using(out)?;
            self.pending_use_release = None;
        }
        self.visual_tick(previous_position, inventory);
        self.send_movement(out);
        self.last_input = input;
        self.attack_ticks = self.attack_ticks.saturating_add(1);
        self.pose_revision = self.pose_revision.wrapping_add(1);
        out.push(PacketCommand::new(sb::CLIENT_TICK_END, Writer::default()));
        Ok(())
    }
    fn sync_input(&mut self, input: HeldInput, out: &mut Vec<PacketCommand>) {
        if input.flags != self.last_sent_input {
            let mut w = Writer::default();
            w.byte(input.flags);
            out.push(PacketCommand::new(sb::PLAYER_INPUT, w));
            self.last_sent_input = input.flags;
        }
    }
    fn entity_command(&self, action: PlayerCommand, data: i32, out: &mut Vec<PacketCommand>) {
        let mut w = Writer::default();
        w.var(self.entity_id);
        w.var(action as i32);
        w.var(data);
        out.push(PacketCommand::new(sb::PLAYER_COMMAND, w));
    }
    fn send_movement(&mut self, out: &mut Vec<PacketCommand>) {
        self.position_reminder = self.position_reminder.saturating_add(1);
        let position = (0..3)
            .map(|i| (self.position[i] - self.last_sent_position[i]).powi(2))
            .sum::<f64>()
            > 4e-8
            || self.position_reminder >= 20;
        let rotation = self.yaw != self.last_sent_yaw || self.pitch != self.last_sent_pitch;
        if self.vehicle.is_some() {
            let mut w = Writer::default();
            w.f32(self.yaw);
            w.f32(self.pitch);
            w.byte(u8::from(self.grounded) | (u8::from(self.horizontal_collision) << 1));
            out.push(PacketCommand::new(sb::MOVE_PLAYER_ROT, w));
            return;
        }
        if position && rotation {
            self.full_movement(out);
            return;
        }
        let mut w = Writer::default();
        let id = if position {
            for p in self.position {
                w.f64(p);
            }
            sb::MOVE_PLAYER_POS
        } else if rotation {
            w.f32(self.yaw);
            w.f32(self.pitch);
            sb::MOVE_PLAYER_ROT
        } else if self.grounded != self.last_sent_grounded
            || self.horizontal_collision != self.last_sent_collision
        {
            sb::MOVE_PLAYER_STATUS_ONLY
        } else {
            return;
        };
        w.byte(u8::from(self.grounded) | (u8::from(self.horizontal_collision) << 1));
        out.push(PacketCommand::new(id, w));
        if position {
            self.last_sent_position = self.position;
            self.position_reminder = 0;
        }
        if rotation {
            self.last_sent_yaw = self.yaw;
            self.last_sent_pitch = self.pitch;
        }
        self.last_sent_grounded = self.grounded;
        self.last_sent_collision = self.horizontal_collision;
    }
}
fn attribute_value(
    base: f64,
    modifiers: &BTreeMap<String, (f64, u8)>,
    exclude_sprint: bool,
) -> f64 {
    let mut adjusted = base;
    for (name, (amount, operation)) in modifiers {
        if exclude_sprint && name == "minecraft:sprinting" {
            continue;
        }
        if *operation == 0 {
            adjusted += amount;
        }
    }
    let mut value = adjusted;
    for (name, (amount, operation)) in modifiers {
        if exclude_sprint && name == "minecraft:sprinting" {
            continue;
        }
        if *operation == 1 {
            value += adjusted * amount;
        }
    }
    for (name, (amount, operation)) in modifiers {
        if exclude_sprint && name == "minecraft:sprinting" {
            continue;
        }
        if *operation == 2 {
            value *= 1. + amount;
        }
    }
    value
}
fn read_velocity(r: &mut Reader<'_>) -> Result<[f64; 3]> {
    let first = r.u8()?;
    if first == 0 {
        return Ok([0.; 3]);
    }
    let second = r.u8()?;
    let hi = r.i32()? as u32;
    let packed = (u64::from(hi) << 16) | (u64::from(second) << 8) | u64::from(first);
    let mut scale = u64::from(first & 3);
    if first & 4 != 0 {
        scale |= u64::from(r.var()? as u32) << 2;
    }
    Ok(std::array::from_fn(|i| {
        ((((packed >> (3 + 15 * i)) & 32767).min(32766) as f64 * 2. / 32766.) - 1.) * scale as f64
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn correction_keeps_all_nine_flags_and_rotates_delta_before_relative_add() {
        let mut p = Player::new();
        p.position = [2., 3., 4.];
        p.velocity = [0., 0., 1.];
        p.yaw = 0.;
        p.pitch = 0.;
        p.pose_known = true;
        p.correct(
            [1., 2., 3.],
            [0.5, 0., 0.],
            90.,
            0.,
            1 | 4 | 32 | 64 | 128 | 256,
        )
        .unwrap();
        assert_eq!(p.position, [3., 2., 7.]);
        assert!((p.velocity[0] + 0.5).abs() < 1e-6);
        assert!(p.velocity[2].abs() < 1e-6);
        assert_eq!(p.yaw, 90.);
        assert_eq!(p.corrections, 1);
    }
    #[test]
    fn relative_pitch_is_clamped_and_absolute_velocity_replaces_rotated_old_component() {
        let mut p = Player::new();
        p.pitch = 80.;
        p.velocity = [4., 5., 6.];
        p.correct([0.; 3], [1., 2., 3.], 10., 30., 16 | 256)
            .unwrap();
        assert_eq!(p.pitch, 90.);
        assert_eq!(p.velocity, [1., 2., 3.]);
        assert!(p.correct([0.; 3], [0.; 3], 0., 0., 512).is_err());
    }
    #[test]
    fn mode_transition_cannot_leave_survival_flying() {
        let mut p = Player::new();
        p.set_mode(3).unwrap();
        assert!(p.abilities.flying);
        p.set_mode(0).unwrap();
        assert!(
            !p.abilities.flying
                && !p.abilities.may_fly
                && !p.abilities.instabuild
                && p.abilities.may_build
        );
        p.set_mode(2).unwrap();
        assert!(!p.abilities.may_build);
    }
    #[test]
    fn attribute_modifiers_apply_add_then_adjusted_base_then_total() {
        let m = BTreeMap::from([
            ("a".into(), (2., 0)),
            ("b".into(), (0.5, 1)),
            ("c".into(), (0.2, 2)),
        ]);
        assert_eq!(attribute_value(4., &m, false), 10.799999999999999);
    }
    #[test]
    fn vanilla_defaults_seed_max_health_before_any_attribute_packet() {
        // Fresh join: no UpdateAttributes seen yet. Vanilla LocalPlayer
        // already has DefaultAttributes (max_health 20), so the HUD must
        // too — no server packet, no HUD-side magic number.
        let mut player = Player::new();
        assert!(!player.attributes.contains_key("minecraft:max_health"));
        player.init_vanilla_defaults([("minecraft:max_health", 20.)]);
        let max = player
            .attributes
            .get("minecraft:max_health")
            .expect("defaults carry max_health");
        assert_eq!((max.base, max.value), (20., 20.));
        assert!(max.modifiers.is_empty());
        assert_eq!(player.attribute("minecraft:max_health"), 20.);
    }
    #[test]
    fn input_screen_cleanup_removes_every_held_action() {
        let k = BTreeSet::from([
            "forward".into(),
            "use".into(),
            "attack".into(),
            "sneak".into(),
        ]);
        let i = HeldInput::from_keys(&k, true);
        assert_eq!(i.flags, 0);
        assert!(!i.attack && !i.use_item);
    }

    #[test]
    fn p1_hurt_tilt_uses_remaining_fourth_power_envelope_and_strength() {
        let mut player = Player::new();
        assert_eq!(player.hurt_tilt_deg(100, 0., 1.), 0.);
        player.hurt_tick = Some(100);
        // Two ticks after hurt: (8 / 10)^4 * PI, not elapsed-time progress.
        assert!((player.hurt_tilt_deg(102, 0., 1.) + 13.439193).abs() < 0.00001);
        assert!((player.hurt_tilt_deg(105, 0., 1.) + 2.7312646).abs() < 0.00001);
        assert!((player.hurt_tilt_deg(102, 0., 0.5) + 6.7195964).abs() < 0.00001);
        assert_eq!(player.hurt_tilt_deg(102, 0., 0.), 0.);
        assert_eq!(player.hurt_tilt_deg(110, 0., 1.), 0.);
        assert_eq!(player.hurt_tilt_deg(u64::MAX, 0., 1.), 0.);
    }

    #[test]
    fn p1_hurt_tilt_supports_partial_ticks_and_resets_with_dimension() {
        let mut player = Player::new();
        player.hurt_tick = Some(20);
        player.hurt_direction = 75.;
        assert!((player.hurt_tilt_deg(24, 0.5, 1.) - player.hurt_tilt_deg(24, 0., 1.)).abs() > 0.1);
        assert_eq!(player.hurt_tilt_deg(29, 1., 1.), 0.);
        player.reset_dimension(true);
        assert_eq!(player.hurt_tilt_deg(24, 0., 1.), 0.);
        assert_eq!(player.hurt_direction, 0.);
    }

    #[test]
    fn p1_hurt_animation_only_restarts_local_player_after_complete_packet() {
        let content = Content::test_cube();
        let mut player = Player::new();
        player.entity_id = 7;
        player.yaw = 30.;
        player.pitch = 12.;
        let mut packet = Writer::default();
        packet.var(7);
        packet.f32(90.);
        assert!(player
            .apply(43, &mut Reader::new(&packet.0), &content, 100)
            .unwrap());
        assert_eq!(player.hurt_tick, Some(100));
        assert_eq!(player.hurt_direction, 90.);
        assert_eq!((player.yaw, player.pitch), (30., 12.));
        let mut other = Writer::default();
        other.var(8);
        other.f32(-45.);
        let mut reader = Reader::new(&other.0);
        assert!(!player.apply(43, &mut reader, &content, 101).unwrap());
        assert_eq!(reader.offset, 0);
        assert_eq!(player.hurt_tick, Some(100));
        packet.0.push(0);
        assert!(player
            .apply(43, &mut Reader::new(&packet.0), &content, 102)
            .is_err());
        assert_eq!(player.hurt_tick, Some(100));
        packet.0.pop();
        assert!(player
            .apply(43, &mut Reader::new(&packet.0), &content, 107)
            .unwrap());
        assert_eq!(player.hurt_tick, Some(107));
    }
    #[test]
    fn teleport_acknowledgement_echoes_corrected_pose_in_26_3_layout() {
        let mut player = Player::new();
        player.pending_teleport = Some((17, [1.5, 64., -2.25], 90., -30.));
        let mut commands = Vec::new();
        player.drain_commands(&mut commands);
        let acknowledgement = &commands[0];
        assert_eq!(acknowledgement.id, 0);
        let mut reader = Reader::new(&acknowledgement.payload.0);
        assert_eq!(reader.var().unwrap(), 17);
        assert_eq!([reader.f64().unwrap(), reader.f64().unwrap(), reader.f64().unwrap()], [1.5, 64., -2.25]);
        assert_eq!((reader.f32().unwrap(), reader.f32().unwrap()), (90., -30.));
        reader.done().unwrap();
    }
}
