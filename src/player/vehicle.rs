//! Source-derived controlled mounts. Minecarts and non-controlling passengers use received
//! entity interpolation; they are never run through a guessed boat/horse simulator.
use crate::protocol::packet_ids::play::serverbound as sb;
use super::*;
use crate::entities::Metadata;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Boat,
    Horse,
    Camel,
    Pig,
    Strider,
    Nautilus,
    HappyGhast,
    Server,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BoatStatus {
    Air,
    Land,
    Water,
    UnderWater,
    FlowingWater,
}
pub(super) struct SeatMotion {
    handle: Handle,
    position: [f64; 3],
    tick: u64,
    walk_speed: f32,
    walk_position: f32,
    stand: f32,
    stand_old: f32,
}

pub(super) struct VehiclePrediction {
    pub handle: Handle,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
    fall_distance: f64,
    stuck_multiplier: [f64; 3],
    last_movement: Option<collision::Movement>,
    inside_block_state: collision::InsideBlockState,
    wire_position: [f64; 3],
    wire_velocity: [f64; 3],
    wire_yaw: f32,
    wire_pitch: f32,
    delta_rotation: f32,
    last_y_delta: f64,
    status: BoatStatus,
    riding_jump_ticks: i32,
    riding_jump_scale: f32,
    pending_jump: f32,
    dash_cooldown: u32,
    was_dashing: bool,
    allow_stand_sliding: bool,
    stand_animation: f32,
    stand_animation_old: f32,
    boost_total: u32,
    boost_ticks: u32,
    boosting: bool,
}
impl VehiclePrediction {
    fn received(e: &crate::entities::Entity) -> Self {
        Self {
            handle: e.handle,
            position: e.position,
            velocity: e.velocity,
            yaw: e.transform.yaw,
            pitch: e.transform.pitch,
            grounded: e.on_ground,
            fall_distance: 0.,
            stuck_multiplier: [0.; 3],
            last_movement: None,
            inside_block_state: collision::InsideBlockState::default(),
            wire_position: e.position,
            wire_velocity: e.velocity,
            wire_yaw: e.transform.yaw,
            wire_pitch: e.transform.pitch,
            delta_rotation: 0.,
            last_y_delta: 0.,
            status: BoatStatus::Air,
            riding_jump_ticks: 0,
            riding_jump_scale: 0.,
            pending_jump: 0.,
            dash_cooldown: 0,
            was_dashing: false,
            allow_stand_sliding: false,
            stand_animation: 0.,
            stand_animation_old: 0.,
            boost_total: 0,
            boost_ticks: 0,
            boosting: false,
        }
    }
    fn reconcile(&mut self, e: &crate::entities::Entity) {
        // Appearance/metadata changes are not movement corrections. Compare only received motion.
        if e.position != self.wire_position
            || e.velocity != self.wire_velocity
            || e.transform.yaw != self.wire_yaw
            || e.transform.pitch != self.wire_pitch
        {
            self.position = e.position;
            self.velocity = e.velocity;
            self.yaw = e.transform.yaw;
            self.pitch = e.transform.pitch;
            self.grounded = e.on_ground;
            self.wire_position = e.position;
            self.wire_velocity = e.velocity;
            self.wire_yaw = e.transform.yaw;
            self.wire_pitch = e.transform.pitch;
        }
    }
    fn charge_jump(&mut self, input: HeldInput, previous: HeldInput) -> Option<i32> {
        if self.riding_jump_ticks < 0 {
            self.riding_jump_ticks += 1;
            if self.riding_jump_ticks == 0 {
                self.riding_jump_scale = 0.;
            }
        }
        if previous.has(4) && !input.has(4) {
            self.riding_jump_ticks = -10;
            let strength = (self.riding_jump_scale * 100.).floor() as i32;
            self.pending_jump = if strength >= 90 {
                1.
            } else {
                0.4 + 0.4 * strength.max(0) as f32 / 90.
            };
            self.allow_stand_sliding = true;
            return Some(strength);
        }
        if !previous.has(4) && input.has(4) {
            self.riding_jump_ticks = 0;
            self.riding_jump_scale = 0.;
        } else if previous.has(4) {
            self.riding_jump_ticks += 1;
            self.riding_jump_scale = if self.riding_jump_ticks < 10 {
                self.riding_jump_ticks as f32 * 0.1
            } else {
                0.8 + 2. / (self.riding_jump_ticks - 9) as f32 * 0.1
            };
        }
        None
    }
    fn boost(&mut self, total: u32) -> f32 {
        if total != self.boost_total {
            self.boost_total = total;
            self.boost_ticks = 0;
            self.boosting = total > 0;
        }
        if self.boosting {
            let old = self.boost_ticks;
            self.boost_ticks = self.boost_ticks.saturating_add(1);
            if old > total {
                self.boosting = false;
            }
        }
        if self.boosting && total > 0 {
            1. + 1.15
                * math::sin(f64::from(
                    self.boost_ticks as f32 / total as f32 * std::f32::consts::PI,
                ))
        } else {
            1.
        }
    }
}
impl Player {
    pub(super) fn source_in_water(
        &mut self,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<bool> {
        let bounds = self.bounds();
        let mut scan = bounds.deflated(0.001);
        if let Some(handle) = entities.vehicle_of(self.entity_id) {
            let entity = entities.resolve(handle)?;
            let models = entities.models_ref().ok_or("entity_models_not_activated")?;
            let schema = models
                .schemas
                .get(&entity.type_name)
                .ok_or("missing_mount_schema")?;
            match schema
                .method_owners
                .get("modifyPassengerFluidInteractionBox")
                .map(String::as_str)
            {
                Some("net.minecraft.world.entity.vehicle.boat.AbstractBoat") => {
                    let received = entities.bounds(handle, tick, 1.)?;
                    let boat = if let Some(predicted) = self
                        .vehicle_prediction
                        .as_ref()
                        .filter(|v| v.handle == handle)
                    {
                        received.moved(std::array::from_fn(|i| {
                            predicted.position[i] - entity.position[i]
                        }))
                    } else {
                        received
                    };
                    let status = if let Some(predicted) = self
                        .vehicle_prediction
                        .as_ref()
                        .filter(|v| v.handle == handle)
                    {
                        predicted.status
                    } else {
                        self.boat_status(world, content, boat)?.0
                    };
                    let Some(clipped) = passenger_fluid_box(
                        scan,
                        boat,
                        matches!(status, BoatStatus::UnderWater | BoatStatus::FlowingWater),
                    ) else {
                        return Ok(false);
                    };
                    scan = clipped;
                }
                Some("net.minecraft.world.entity.Entity") => {}
                _ => return Err("unknown_source_passenger_fluid_box".into()),
            }
        }
        Ok(super::movement::fluid_contact_scan(world, content, bounds, scan, false)?.depth[0] > 0.)
    }

    pub fn update_world_time(&mut self, game_time: i64, _tick: u64) {
        self.world_game_time = Some(game_time);
    }
    pub fn predicted_vehicle_transform(&self) -> Option<(Handle, [f64; 3], f32, f32)> {
        self.vehicle_prediction
            .as_ref()
            .map(|v| (v.handle, v.position, v.yaw, v.pitch))
    }
    pub fn synchronize_vehicle(
        &mut self,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
    ) -> Result<()> {
        self.vehicle = entities.vehicle_of(self.entity_id);
        self.vehicle_controlled = false;
        if let Some(handle) = self.vehicle {
            let entity = entities.resolve(handle)?;
            let schema = entities
                .models_ref()
                .ok_or("entity_models_not_activated")?
                .schemas
                .get(&entity.type_name)
                .ok_or("missing_mount_schema")?;
            self.vehicle_controlled = self.game_mode != 3
                && !self.health.is_some_and(|h| h <= 0.)
                && entity.vehicle.is_none()
                && mount_controlled(
                    entities,
                    handle,
                    self.entity_id,
                    kind(&schema.class),
                    content,
                    inventory,
                )?;
            if self.vehicle_controlled {
                if self
                    .vehicle_prediction
                    .as_ref()
                    .is_none_or(|v| v.handle != handle)
                {
                    self.vehicle_prediction = Some(VehiclePrediction::received(entity));
                }
                return Ok(());
            }
        }
        self.vehicle_prediction = None;
        self.pending_vehicle_correction = None;
        Ok(())
    }
    pub(super) fn drain_vehicle_correction(&mut self, out: &mut Vec<PacketCommand>) {
        let Some((handle, _, _, _)) = self.pending_vehicle_correction.take() else {
            return;
        };
        let Some(vehicle) = self.vehicle_prediction.as_mut().filter(|v| {
            self.vehicle_controlled && v.handle == handle && self.vehicle == Some(handle)
        }) else {
            return;
        };
        write_vehicle(
            vehicle.position,
            vehicle.yaw,
            vehicle.pitch,
            vehicle.grounded,
            out,
        );
    }
    pub(super) fn correct_vehicle(
        &mut self,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
    ) -> Result<()> {
        if position.iter().any(|v| !v.is_finite()) || !yaw.is_finite() || !pitch.is_finite() {
            return Err("nonfinite_vehicle_correction".into());
        }
        if self.vehicle_controlled {
            if let Some(handle) = self.vehicle {
                if let Some(vehicle) = self
                    .vehicle_prediction
                    .as_mut()
                    .filter(|vehicle| vehicle.handle == handle)
                {
                    let distance = (0..3)
                        .map(|axis| (vehicle.position[axis] - position[axis]).powi(2))
                        .sum::<f64>()
                        .sqrt();
                    if distance > f64::from(1e-5f32) {
                        vehicle.position = position;
                        vehicle.yaw = yaw;
                        vehicle.pitch = pitch;
                        vehicle.delta_rotation = 0.;
                    }
                }
                self.pending_vehicle_correction = Some((handle, position, yaw, pitch));
            }
        }
        Ok(())
    }
    pub(super) fn predict_vehicle(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        input: HeldInput,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        let handle = self.vehicle.ok_or("mount_handle_missing")?;
        let e = entities.resolve(handle)?;
        let models = entities.models_ref().ok_or("entity_models_not_activated")?;
        let schema = models
            .schemas
            .get(&e.type_name)
            .ok_or("missing_mount_schema")?;
        let kind = kind(&schema.class);
        let controlled =
            mount_controlled(entities, handle, self.entity_id, kind, content, inventory)?;
        self.vehicle_controlled = controlled
            && e.vehicle.is_none()
            && self.game_mode != 3
            && !self.health.is_some_and(|h| h <= 0.);
        self.swimming = false;
        self.gliding = false;
        self.pose = Pose::Standing;
        let sprint_allowed = controlled
            && kind == Kind::Camel
            && !input.has(5)
            && self.using.as_ref().is_none_or(|u| u.can_sprint)
            && self.amplifier("minecraft:blindness", tick).is_none();
        if !sprint_allowed || input.forward() <= 0.8 {
            self.sprinting = false;
        } else if !self.sprinting {
            let rising = input.has(0) && !self.last_input.has(0);
            if input.has(6) || (rising && self.sprint_trigger > 0) {
                self.sprinting = true;
            } else if rising {
                self.sprint_trigger = 7;
            }
        }
        if self.sprinting != self.sent_sprinting {
            self.entity_command(
                if self.sprinting { PlayerCommand::StartSprinting } else { PlayerCommand::StopSprinting },
                0, out,
            );
            self.sent_sprinting = self.sprinting;
        }
        if !self.vehicle_controlled {
            self.vehicle_prediction = None;
            self.pending_vehicle_correction = None;
            let transform = e.presented_transform(tick, 1.);
            self.position = self.mount_seat_position(
                entities,
                handle,
                transform.position,
                transform.yaw,
                tick,
            )?;
            self.velocity = [0.; 3];
            self.grounded = e.on_ground;
            self.horizontal_collision = false;
            return Ok(());
        }
        let mut v = self
            .vehicle_prediction
            .take()
            .filter(|v| v.handle == handle)
            .unwrap_or_else(|| VehiclePrediction::received(e));
        v.reconcile(e);
        let result = (|| {
            let received_bounds = entities.bounds(handle, tick, 1.)?;
            let mut bounds =
                received_bounds.moved(std::array::from_fn(|i| v.position[i] - e.position[i]));
            let no_gravity = boolean(entities, handle, "DATA_NO_GRAVITY", false)?;
            let movement = super::movement::modified_input(input, 1., 1.);
            let efficiency = if kind == Kind::Boat {
                0.
            } else {
                entities.attribute(handle, "minecraft:movement_efficiency")? as f32
            };
            if kind == Kind::Boat {
                let (status, water_level, friction) = self.boat_status(world, content, bounds)?;
                let old_status = v.status;
                v.status = status;
                if old_status == BoatStatus::Air
                    && !matches!(status, BoatStatus::Air | BoatStatus::Land)
                {
                    let top = boat_water_above(world, content, bounds, v.last_y_delta)?;
                    let candidate = f64::from(top - (bounds.max[1] - bounds.min[1]) as f32) + 0.101;
                    let delta = [0., candidate - v.position[1], 0.];
                    collision::gather(
                        world,
                        content,
                        entities,
                        tick,
                        bounds.moved(delta),
                        &mut self.colliders,
                    )?;
                    if !self
                        .colliders
                        .iter()
                        .any(|c| bounds.moved(delta).intersects(*c))
                    {
                        v.position[1] = candidate;
                        bounds = bounds.moved(delta);
                        v.velocity[1] = 0.;
                        v.last_y_delta = 0.;
                    }
                    v.status = BoatStatus::Water;
                } else {
                    let mut gravity = if no_gravity { 0. } else { -0.04 };
                    let (buoyancy, drag) = match status {
                        BoatStatus::Water => (
                            (water_level - v.position[1]) / (bounds.max[1] - bounds.min[1]),
                            0.9f32,
                        ),
                        BoatStatus::FlowingWater => {
                            gravity = -0.0007;
                            (0., 0.9)
                        }
                        BoatStatus::UnderWater => (f64::from(0.01f32), 0.45),
                        BoatStatus::Air => (0., 0.9),
                        BoatStatus::Land => (0., friction),
                    };
                    v.velocity[0] *= f64::from(drag);
                    v.velocity[2] *= f64::from(drag);
                    v.velocity[1] += gravity;
                    v.delta_rotation *= drag;
                    if buoyancy > 0. {
                        v.velocity[1] = (v.velocity[1] + buoyancy * 0.04 / 0.65) * 0.75;
                    }
                }
                v.delta_rotation +=
                    (u8::from(input.has(3)) as f32) - (u8::from(input.has(2)) as f32);
                v.yaw += v.delta_rotation;
                let mut acceleration =
                    if input.has(2) != input.has(3) && !input.has(0) && !input.has(1) {
                        0.005f32
                    } else {
                        0.
                    };
                if input.has(0) {
                    acceleration += 0.04;
                }
                if input.has(1) {
                    acceleration -= 0.005;
                }
                let angle = f64::from(v.yaw * 0.017453292);
                v.velocity[0] += f64::from(math::sin(-angle) * acceleration);
                v.velocity[2] += f64::from(math::cos(angle) * acceleration);
                let mut w = Writer::default();
                w.byte(u8::from((input.has(3) && !input.has(2)) || input.has(0)));
                w.byte(u8::from((input.has(2) && !input.has(3)) || input.has(0)));
                out.push(PacketCommand::new(sb::PADDLE_BOAT, w));
                let wanted = prepared_move(&mut v);
                collision::gather(
                    world,
                    content,
                    entities,
                    tick,
                    bounds.expanded(wanted),
                    &mut self.colliders,
                )?;
                entities.collision_bounds(
                    bounds.expanded(wanted),
                    self.entity_id,
                    Some(handle),
                    tick,
                    v.position,
                    world,
                    content,
                    &mut self.colliders,
                )?;
                let moved = collision::collide(bounds, &self.colliders, wanted);
                finish_move(&mut v, wanted, moved, world, content, false, efficiency)?;
                self.yaw += v.delta_rotation;
                let difference = wrap_degrees(self.yaw - v.yaw);
                self.yaw += difference.clamp(-105., 105.) - difference;
                v.last_y_delta = moved[1];
            } else if kind == Kind::HappyGhast {
                let flying_speed = entities.attribute(handle, "minecraft:flying_speed")?;
                v.yaw += wrap_degrees(self.yaw - v.yaw) * 0.08;
                v.pitch = self.pitch * 0.5;
                let angle = f64::from(self.pitch * 0.017453292);
                let direction = if input.forward() < 0. { -0.5f32 } else { 1. };
                let travel = [
                    f64::from(movement[0]),
                    f64::from(
                        if input.forward() != 0. {
                            -math::sin(angle) * direction
                        } else {
                            0.
                        } + if input.has(4) { 0.5 } else { 0. },
                    ),
                    f64::from(if input.forward() != 0. {
                        math::cos(angle) * direction
                    } else {
                        0.
                    }),
                ]
                .map(|d| d * f64::from(3.9f32) * flying_speed);
                let fluid = mount_fluid(
                    world,
                    content,
                    bounds,
                    &mut v.velocity,
                    true,
                    self.fast_lava,
                )?;
                relative(&mut v, travel, flying_speed as f32 * 5. / 3.);
                let wanted = prepared_move(&mut v);
                let context = collision::Context {
                    bottom: v.position[1],
                    descending: false,
                    fall_distance: 0.,
                    walks_powder_snow: false,
                };
                collision::gather_with_context(
                    world,
                    content,
                    entities,
                    tick,
                    bounds.expanded(wanted),
                    Some(context),
                    &mut self.colliders,
                )?;
                entities.collision_bounds(
                    bounds.expanded(wanted),
                    self.entity_id,
                    Some(handle),
                    tick,
                    v.position,
                    world,
                    content,
                    &mut self.colliders,
                )?;
                let moved = collision::collide(bounds, &self.colliders, wanted);
                finish_move(&mut v, wanted, moved, world, content, true, efficiency)?;
                let drag = if fluid.0 > 0. {
                    f64::from(0.8f32)
                } else if fluid.1 > 0. {
                    0.5
                } else {
                    f64::from(0.91f32)
                };
                for component in &mut v.velocity {
                    *component *= drag;
                }
            } else {
                let movement_speed = entities.attribute(handle, "minecraft:movement_speed")? as f32;
                let gravity = if no_gravity {
                    0.
                } else {
                    entities.attribute(handle, "minecraft:gravity")?
                };
                let step = entities.attribute(handle, "minecraft:step_height")?;
                let jumping = matches!(kind, Kind::Camel | Kind::Nautilus)
                    && boolean(entities, handle, "DASH", false)?;
                if jumping && !v.was_dashing {
                    v.dash_cooldown = match kind {
                        Kind::Camel => 55,
                        Kind::Nautilus => 40,
                        _ => 0,
                    };
                }
                v.was_dashing = jumping;
                v.dash_cooldown = v.dash_cooldown.saturating_sub(1);
                let flags = if matches!(kind, Kind::Horse | Kind::Camel) {
                    integer(entities, handle, "DATA_ID_FLAGS", 0)? as u8
                } else {
                    0
                };
                v.stand_animation_old = v.stand_animation;
                if flags & 32 != 0 {
                    v.stand_animation =
                        (v.stand_animation + (1. - v.stand_animation) * 0.4 + 0.05).min(1.);
                } else {
                    let s = v.stand_animation;
                    v.stand_animation = (s + (0.8 * s * s * s - s) * 0.6 - 0.05).max(0.);
                    v.allow_stand_sliding = false;
                }
                let mut refuse = false;
                if kind == Kind::Camel {
                    let pose_time = match entities.metadata(handle, "LAST_POSE_CHANGE_TICK")? {
                        Some(Metadata::Long(s)) => {
                            s.parse::<i64>().map_err(|_| "invalid_camel_pose_time")?
                        }
                        None => 0,
                        _ => return Err("invalid_camel_pose_metadata".into()),
                    };
                    let elapsed = self
                        .world_game_time
                        .ok_or("world_time_not_received")?
                        .saturating_sub(pose_time.saturating_abs());
                    refuse = pose_time < 0 || elapsed < if pose_time < 0 { 40 } else { 52 };
                }
                if matches!(kind, Kind::Horse | Kind::Camel | Kind::Nautilus)
                    && v.dash_cooldown == 0
                    && !refuse
                {
                    if let Some(strength) = v.charge_jump(input, self.last_input) {
                        self.entity_command(PlayerCommand::StartRidingJump, strength, out);
                    }
                } else {
                    v.riding_jump_scale = 0.;
                }
                if kind == Kind::Nautilus {
                    v.yaw += wrap_degrees(self.yaw - v.yaw) * 0.5;
                } else if !refuse {
                    v.yaw = self.yaw;
                }
                v.pitch = self.pitch * 0.5;
                let fluid = mount_fluid(
                    world,
                    content,
                    bounds,
                    &mut v.velocity,
                    kind != Kind::Nautilus,
                    self.fast_lava,
                )?;
                let below = [
                    v.position[0].floor() as i32,
                    (bounds.min[1] - 0.5000001).floor() as i32,
                    v.position[2].floor() as i32,
                ];
                let below_state = world.block(below).ok_or("unloaded_mount_collision")?;
                let friction = content.info(below_state)?.friction;
                let block_speed = content.info(below_state)?.speed_factor;
                let mut speed = movement_speed;
                let mut travel = [
                    movement[0] * 0.5,
                    0.,
                    if input.forward() > 0. {
                        movement[2]
                    } else {
                        movement[2] * 0.25
                    },
                ];
                if refuse
                    || (v.grounded
                        && v.pending_jump == 0.
                        && flags & 32 != 0
                        && !v.allow_stand_sliding)
                {
                    travel = [0.; 3];
                }
                match kind {
                    Kind::Pig | Kind::Strider => {
                        travel = [0., 0., 1.];
                        let total = integer(entities, handle, "DATA_BOOST_TIME", 0)?.max(0) as u32;
                        let steering = if kind == Kind::Pig {
                            0.225
                        } else {
                            f64::from(if boolean(entities, handle, "DATA_SUFFOCATING", false)? {
                                0.35f32
                            } else {
                                0.55f32
                            })
                        };
                        speed = (f64::from(movement_speed) * steering * f64::from(v.boost(total)))
                            as f32;
                    }
                    Kind::Camel => {
                        if self.sprinting && v.dash_cooldown == 0 {
                            speed += 0.1;
                        }
                    }
                    Kind::Nautilus => {
                        let a = f64::from(self.pitch * 0.017453292);
                        let multiplier = if input.forward() < 0. { -0.5 } else { 1. };
                        travel = [
                            movement[0],
                            if input.forward() != 0. {
                                -math::sin(a) * multiplier
                            } else {
                                0.
                            },
                            if input.forward() != 0. {
                                math::cos(a) * multiplier
                            } else {
                                0.
                            },
                        ];
                        speed = movement_speed * if fluid.0 > 0. { 0.0325 } else { 0.02 };
                    }
                    _ => {}
                }
                if v.pending_jump > 0. && (v.grounded || kind == Kind::Nautilus) {
                    let power = v.pending_jump;
                    v.pending_jump = 0.;
                    let jump = entities.attribute(handle, "minecraft:jump_strength")? as f32
                        * content.info(below_state)?.jump_factor;
                    match kind {
                        Kind::Camel => {
                            let d = f64::from(22.2222f32 * power)
                                * f64::from(movement_speed)
                                * f64::from(block_speed);
                            let look = math::view(v.pitch, v.yaw);
                            let h = look[0].hypot(look[2]);
                            if h > 1e-7 {
                                v.velocity[0] += look[0] / h * d;
                                v.velocity[2] += look[2] / h * d;
                            }
                            v.velocity[1] += f64::from(1.4285f32 * power) * f64::from(jump);
                            v.dash_cooldown = 55;
                        }
                        Kind::Nautilus => {
                            let d = f64::from(if fluid.0 > 0. { 1.2f32 } else { 0.5 } * power)
                                * f64::from(movement_speed)
                                * f64::from(block_speed);
                            let look = self.look_direction();
                            for i in 0..3 {
                                v.velocity[i] += look[i] * d;
                            }
                            v.dash_cooldown = 40;
                        }
                        Kind::Horse => {
                            v.velocity[1] =
                                f64::from(jump * power) + mount_jump_boost(e, content, tick)?;
                            if travel[2] > 0. {
                                let a = f64::from(v.yaw * 0.017453292);
                                v.velocity[0] += f64::from(-0.4 * math::sin(a) * power);
                                v.velocity[2] += f64::from(0.4 * math::cos(a) * power);
                            }
                        }
                        _ => {}
                    }
                    v.grounded = false;
                }
                let water = fluid.0 > 0.;
                let start_y = v.position[1];
                let (dimensions, scale) = models.dimensions(e)?;
                let fluid_threshold = if f64::from(dimensions.eye_height * scale) < 0.4 {
                    0.
                } else {
                    0.4
                };
                let lava = fluid.1 > 0. && kind != Kind::Strider;
                let falling = v.velocity[1] <= 0.;
                let gravity =
                    if falling && mount_effect(e, content, "minecraft:slow_falling")?.is_some() {
                        gravity.min(0.01)
                    } else {
                        gravity
                    };
                let sprinting = integer(entities, handle, "DATA_SHARED_FLAGS_ID", 0)? & 8 != 0;
                let mut drag = if water {
                    if sprinting { 0.9f32 } else { 0.8 }
                } else if lava {
                    0.5
                } else if v.grounded {
                    friction * 0.91
                } else {
                    0.91
                };
                let mut acceleration = if kind == Kind::Nautilus && water {
                    speed
                } else if water || lava {
                    0.02
                } else if v.grounded {
                    speed * (0.21600002 / (friction * friction * friction))
                } else {
                    speed * 0.1
                };
                if water && kind != Kind::Nautilus {
                    let efficiency =
                        entities.attribute(handle, "minecraft:water_movement_efficiency")? as f32
                            * if v.grounded { 1. } else { 0.5 };
                    if efficiency > 0. {
                        drag += (0.54600006 - drag) * efficiency;
                        acceleration += (speed - acceleration) * efficiency;
                    }
                    if mount_effect(e, content, "minecraft:dolphins_grace")?.is_some() {
                        drag = 0.96;
                    }
                }
                relative(&mut v, travel.map(f64::from), acceleration);
                if !water && !lava && collision::climbable(world, content, v.position)? {
                    clamp_mount_climb(&mut v.velocity, &mut v.fall_distance);
                }
                let wanted = prepared_move(&mut v);
                let mut area = bounds.expanded(wanted);
                area.max[1] += step;
                let walks_powder_snow = content
                    .registry_tag(
                        "minecraft:entity_type",
                        "minecraft:powder_snow_walkable_mobs",
                    )?
                    .contains(&(e.kind as i32))
                    || e.equipment[2].as_ref().is_some_and(|stack| {
                        content
                            .item_names
                            .get(&stack.id)
                            .is_some_and(|name| name == "minecraft:leather_boots")
                    });
                collision::gather_with_context(
                    world,
                    content,
                    entities,
                    tick,
                    area,
                    Some(collision::Context {
                        bottom: v.position[1],
                        descending: false,
                        fall_distance: v.fall_distance,
                        walks_powder_snow,
                    }),
                    &mut self.colliders,
                )?;
                if kind == Kind::Strider {
                    append_lava_colliders(world, content, bounds, area, &mut self.colliders)?;
                }
                entities.collision_bounds(
                    area,
                    self.entity_id,
                    Some(handle),
                    tick,
                    v.position,
                    world,
                    content,
                    &mut self.colliders,
                )?;
                let moved = collision::with_step(
                    bounds,
                    &self.colliders,
                    wanted,
                    v.grounded,
                    step as f32,
                    &mut self.step_heights,
                );
                finish_move(&mut v, wanted, moved, world, content, true, efficiency)?;
                let horizontal_collision = wanted[0] != moved[0] || wanted[2] != moved[2];
                if horizontal_collision
                    && !lava
                    && (!water || kind != Kind::Nautilus)
                    && (collision::climbable(world, content, v.position)?
                        || !water && v.inside_block_state.powder_snow && walks_powder_snow)
                {
                    v.velocity[1] = 0.2;
                }
                if kind == Kind::Nautilus && water {
                    for d in &mut v.velocity {
                        *d *= 0.9;
                    }
                } else if water {
                    v.velocity[0] *= f64::from(drag);
                    v.velocity[2] *= f64::from(drag);
                    v.velocity[1] *= f64::from(0.8f32);
                    fluid_fall(&mut v.velocity, gravity, falling, sprinting);
                } else if lava {
                    v.velocity[0] *= 0.5;
                    v.velocity[2] *= 0.5;
                    let threshold = fluid_threshold;
                    if fluid.1 <= threshold {
                        v.velocity[1] *= f64::from(0.8f32);
                        fluid_fall(&mut v.velocity, gravity, falling, sprinting);
                    } else {
                        v.velocity[1] *= 0.5;
                    }
                    v.velocity[1] -= gravity / 4.;
                } else {
                    v.velocity[0] *= f64::from(drag);
                    v.velocity[2] *= f64::from(drag);
                    if let Some(amplifier) = mount_effect(e, content, "minecraft:levitation")? {
                        v.velocity[1] += (0.05 * f64::from(amplifier + 1) - v.velocity[1]) * 0.2;
                    } else {
                        v.velocity[1] -= gravity;
                    }
                    v.velocity[1] *= f64::from(0.98f32);
                }
                if (water && kind != Kind::Nautilus || lava) && horizontal_collision {
                    let current = bounds.moved(std::array::from_fn(|axis| {
                        v.position[axis]
                            - v.last_movement
                                .as_ref()
                                .map_or(v.position[axis], |m| m.from[axis])
                    }));
                    let probe =
                        current.moved(fluid_escape_offset(v.velocity, v.position[1], start_y));
                    collision::gather_with_context(
                        world,
                        content,
                        entities,
                        tick,
                        probe,
                        Some(collision::Context {
                            bottom: v.position[1],
                            descending: false,
                            fall_distance: v.fall_distance,
                            walks_powder_snow,
                        }),
                        &mut self.colliders,
                    )?;
                    entities.collision_bounds(
                        probe,
                        self.entity_id,
                        Some(handle),
                        tick,
                        v.position,
                        world,
                        content,
                        &mut self.colliders,
                    )?;
                    if self.colliders.is_empty()
                        && !super::movement::contains_fluid(world, content, probe)?
                    {
                        v.velocity[1] = f64::from(0.3f32);
                    }
                }
                if water {
                    let floats = content
                        .registry_tag("minecraft:entity_type", "minecraft:can_float_while_ridden")?
                        .contains(&(e.kind as i32));
                    ridden_buoyancy(&mut v.velocity, fluid.0, fluid_threshold, floats);
                }
                if kind == Kind::Strider && fluid.1 > 0. {
                    let p = v.position.map(|d| d.floor() as i32);
                    let above = world
                        .block([p[0], p[1] + 1, p[2]])
                        .ok_or("unloaded_mount_fluid")?;
                    let above_lava = content
                        .info(above)?
                        .fluid
                        .is_some_and(|f| f.kind == FluidKind::Lava);
                    if v.position[1] > f64::from(p[1]) + 0.5 - 1e-5 && !above_lava {
                        v.grounded = true;
                    } else {
                        for d in &mut v.velocity {
                            *d *= 0.5;
                        }
                        v.velocity[1] += 0.05;
                    }
                }
            }
            let final_bounds = bounds.moved(std::array::from_fn(|axis| {
                v.position[axis]
                    - bounds.min[axis]
                    - if axis == 1 {
                        0.
                    } else {
                        (bounds.max[axis] - bounds.min[axis]) / 2.
                    }
            }));
            let movement = v.last_movement.unwrap_or(collision::Movement {
                from: v.position,
                to: v.position,
                axis_order: None,
            });
            collision::collect_inside(
                world,
                content,
                std::slice::from_ref(&movement),
                final_bounds,
                v.position,
                collision::Context {
                    bottom: v.position[1],
                    descending: false,
                    fall_distance: v.fall_distance,
                    walks_powder_snow: false,
                },
                &mut self.inside_visits,
                &mut self.inside_effects,
            )?;
            inside_mount(
                world,
                content,
                &mut v,
                final_bounds,
                &self.inside_effects,
                e,
                kind,
                schema.fire_immune,
            )?;
            let seat = self.mount_seat_position(entities, handle, v.position, v.yaw, tick)?;
            write_vehicle(v.position, v.yaw, v.pitch, v.grounded, out);
            self.position = seat;
            self.velocity = [0.; 3];
            self.grounded = v.grounded;
            self.horizontal_collision = false;
            Ok(())
        })();
        self.vehicle_prediction = Some(v);
        result
    }
    fn mount_seat_position(
        &mut self,
        entities: &EntityStore,
        handle: Handle,
        position: [f64; 3],
        yaw: f32,
        tick: u64,
    ) -> Result<[f64; 3]> {
        let entity = entities.resolve(handle)?;
        let models = entities.models_ref().ok_or("entity_models_not_activated")?;
        let schema = models
            .schemas
            .get(&entity.type_name)
            .ok_or("missing_mount_schema")?;
        let index = entity
            .passenger_ids
            .iter()
            .position(|id| *id == self.entity_id)
            .ok_or("mount_passenger_not_received")?;
        let (dimensions, scale) = models.dimensions(entity)?;
        let game_time = self.world_game_time.ok_or("world_time_not_received")?;
        let motion = self.vehicle_seat_motion.get_or_insert_with(|| SeatMotion {
            handle,
            position,
            tick,
            walk_speed: 0.,
            walk_position: 0.,
            stand: 0.,
            stand_old: 0.,
        });
        if motion.handle != handle {
            *motion = SeatMotion {
                handle,
                position,
                tick,
                walk_speed: 0.,
                walk_position: 0.,
                stand: 0.,
                stand_old: 0.,
            };
        } else if motion.tick != tick {
            let distance = ((position[0] - motion.position[0]).powi(2)
                + (position[2] - motion.position[2]).powi(2))
            .sqrt() as f32;
            if entity.vehicle.is_some() || entity.death_tick.is_some() {
                motion.walk_speed = 0.;
                motion.walk_position = 0.;
            } else {
                motion.walk_speed += ((distance * 4.).min(1.) - motion.walk_speed) * 0.4;
                motion.walk_position += motion.walk_speed;
            }
            motion.stand_old = motion.stand;
            if integer(entities, handle, "DATA_ID_FLAGS", 0)? & 32 != 0 {
                motion.stand = (motion.stand + (1. - motion.stand) * 0.4 + 0.05).min(1.);
            } else {
                let s = motion.stand;
                motion.stand = (s + (0.8 * s * s * s - s) * 0.6 - 0.05).max(0.);
            }
            motion.position = position;
            motion.tick = tick;
        }
        let mut point = dimensions
            .attachments
            .get("PASSENGER")
            .and_then(|points| points.get(index.min(points.len().saturating_sub(1))))
            .ok_or("source_mount_passenger_attachment_missing")?
            .map(|value| value * f64::from(scale));
        let height = dimensions.height * scale;
        match schema
            .method_owners
            .get("getPassengerAttachmentPoint")
            .map(String::as_str)
        {
            Some("net.minecraft.world.entity.vehicle.boat.AbstractBoat") => {
                let chest = schema.hierarchy.iter().any(|name| {
                    name == "net.minecraft.world.entity.vehicle.boat.AbstractChestBoat"
                });
                let raft = matches!(
                    schema.class.as_str(),
                    "net.minecraft.world.entity.vehicle.boat.Raft"
                        | "net.minecraft.world.entity.vehicle.boat.ChestRaft"
                );
                point = [
                    0.,
                    f64::from(if raft {
                        height * 0.8888889
                    } else {
                        height / 3.
                    }),
                    f64::from(if entity.passenger_ids.len() > 1 {
                        if index == 0 { 0.2f32 } else { -0.6 }
                    } else if chest {
                        0.15
                    } else {
                        0.
                    }),
                ];
            }
            Some("net.minecraft.world.entity.animal.equine.AbstractHorse") => {
                point[1] += 0.15 * f64::from(motion.stand_old);
                point[2] += -0.7 * f64::from(motion.stand_old);
            }
            Some("net.minecraft.world.entity.animal.camel.Camel") => {
                let pose_time = match entities.metadata(handle, "LAST_POSE_CHANGE_TICK")? {
                    Some(Metadata::Long(value)) => value
                        .parse::<i64>()
                        .map_err(|_| "invalid_camel_pose_time")?,
                    None => 0,
                    _ => return Err("invalid_camel_pose_metadata".into()),
                };
                point = [
                    0.,
                    f64::from(camel_seat_height(
                        height,
                        index == 0,
                        pose_time < 0,
                        game_time.saturating_sub(pose_time.saturating_abs()),
                    )),
                    f64::from(if entity.passenger_ids.len() > 1 && index != 0 {
                        -0.7f32
                    } else {
                        0.5
                    }),
                ];
            }
            Some("net.minecraft.world.entity.monster.Strider") => {
                let phase = motion.walk_position
                    * if boolean(entities, handle, "DATA_BABY_ID", false)? {
                        3.
                    } else {
                        1.
                    };
                point[1] += f64::from(
                    0.12 * math::cos(f64::from(phase * 1.5)) * 2. * motion.walk_speed.min(0.25),
                );
            }
            Some("net.minecraft.world.entity.monster.cubemob.AbstractCubeMob") => {
                let size = integer(entities, handle, "ID_SIZE", 1)?.max(1);
                point = [0., f64::from(height) - 0.015625 * f64::from(size), 0.];
            }
            Some("net.minecraft.world.entity.Entity")
            | Some("net.minecraft.world.entity.animal.equine.Llama")
            | Some("net.minecraft.world.entity.vehicle.minecart.AbstractMinecart") => {}
            _ => return Err(format!("unknown_source_mount_seat:{}", schema.class)),
        }
        let point = rotate_seat(point, yaw);
        // Avatar's current dimensions carry the received scale; riding pose is standing.
        let player = models
            .schemas
            .get("minecraft:player")
            .ok_or("source_player_schema_missing")?;
        let attachment = player
            .dimensions
            .attachments
            .get("VEHICLE")
            .and_then(|points| points.first())
            .ok_or("source_player_vehicle_attachment_missing")?;
        let attachment = rotate_seat(
            attachment.map(|value| value * self.attribute("minecraft:scale")),
            self.yaw,
        );
        Ok(std::array::from_fn(|axis| {
            position[axis] + point[axis] - attachment[axis]
        }))
    }

    fn boat_status(
        &mut self,
        world: &World,
        content: &Content,
        b: Bounds,
    ) -> Result<(BoatStatus, f64, f32)> {
        let top = b.max[1] + 0.001;
        let mut under_source = false;
        for x in b.min[0].floor() as i32..b.max[0].ceil() as i32 {
            for z in b.min[2].floor() as i32..b.max[2].ceil() as i32 {
                for y in b.max[1].floor() as i32..top.ceil() as i32 {
                    let p = [x, y, z];
                    let s = world.block(p).ok_or("unloaded_boat_water")?;
                    if let Some(f) = content
                        .info(s)?
                        .fluid
                        .filter(|f| f.kind == FluidKind::Water)
                    {
                        if top < f64::from(y) + Self::fluid_height(world, content, p, f)? {
                            if f.amount != 8 || f.falling {
                                return Ok((BoatStatus::FlowingWater, b.max[1], 0.));
                            }
                            under_source = true;
                        }
                    }
                }
            }
        }
        if under_source {
            return Ok((BoatStatus::UnderWater, b.max[1], 0.));
        }
        let mut level = f64::MIN;
        let mut in_water = false;
        for x in b.min[0].floor() as i32..b.max[0].ceil() as i32 {
            for z in b.min[2].floor() as i32..b.max[2].ceil() as i32 {
                for y in b.min[1].floor() as i32..(b.min[1] + 0.001).ceil() as i32 {
                    let p = [x, y, z];
                    let s = world.block(p).ok_or("unloaded_boat_water")?;
                    if let Some(f) = content
                        .info(s)?
                        .fluid
                        .filter(|f| f.kind == FluidKind::Water)
                    {
                        let h = f64::from(y) + Self::fluid_height(world, content, p, f)?;
                        level = level.max(h);
                        in_water |= b.min[1] < h;
                    }
                }
            }
        }
        if in_water {
            return Ok((BoatStatus::Water, level, 0.));
        }
        let ground = Bounds {
            min: [b.min[0], b.min[1] - 0.001, b.min[2]],
            max: [b.max[0], b.min[1], b.max[2]],
        };
        let mut total = 0.0f32;
        let mut count = 0u32;
        let min = ground.min.map(|n| n.floor() as i32 - 1);
        let max = ground.max.map(|n| n.ceil() as i32 + 1);
        for x in min[0]..max[0] {
            for z in min[2]..max[2] {
                let edges = u8::from(x == min[0] || x == max[0] - 1)
                    + u8::from(z == min[2] || z == max[2] - 1);
                if edges == 2 {
                    continue;
                }
                for y in min[1]..max[1] {
                    if edges > 0 && (y == min[1] || y == max[1] - 1) {
                        continue;
                    }
                    let p = [x, y, z];
                    let s = world.block(p).ok_or("unloaded_boat_ground")?;
                    if content.name(s) == Some("minecraft:lily_pad") {
                        continue;
                    }
                    let shapes = content
                        .collision_boxes(s)
                        .ok_or("unknown_boat_ground_shape")?;
                    let offset = content.block_offset(s, p);
                    if shapes.iter().any(|raw| {
                        ground.intersects(
                            Bounds::new(*raw)
                                .moved(std::array::from_fn(|i| f64::from(p[i]) + offset[i])),
                        )
                    }) {
                        total += content.info(s)?.friction;
                        count += 1;
                    }
                }
            }
        }
        if count > 0 {
            Ok((BoatStatus::Land, level, total / count as f32))
        } else {
            Ok((BoatStatus::Air, level, 0.))
        }
    }
}
fn mount_controlled(
    entities: &EntityStore,
    handle: Handle,
    player_id: i32,
    kind: Kind,
    content: &Content,
    inventory: &InventoryStore,
) -> Result<bool> {
    let entity = entities.resolve(handle)?;
    let saddle = entity.equipment[7].as_ref().is_some_and(|s| !s.empty());
    let steer_item = match kind {
        Kind::Pig => Some("minecraft:carrot_on_a_stick"),
        Kind::Strider => Some("minecraft:warped_fungus_on_a_stick"),
        _ => None,
    };
    let holds_steer = steer_item.is_none_or(|name| {
        [36 + inventory.selected_slot as usize, 45]
            .into_iter()
            .any(|slot| {
                inventory
                    .player
                    .slots
                    .get(slot)
                    .and_then(Option::as_ref)
                    .and_then(|stack| content.item_names.get(&stack.id))
                    .is_some_and(|item| item == name)
            })
    });
    Ok(entities.is_first_passenger(handle, player_id)
        && kind != Kind::Server
        && (kind == Kind::Boat
            || (kind == Kind::HappyGhast
                && entity.equipment[6].as_ref().is_some_and(|s| !s.empty())
                && !boolean(entities, handle, "STAYS_STILL", false)?)
            || (kind != Kind::HappyGhast && saddle))
        && holds_steer)
}
fn rotate_seat(point: [f64; 3], yaw: f32) -> [f64; 3] {
    let angle = f64::from(-yaw * 0.017453292);
    let sin = f64::from(math::sin(angle));
    let cos = f64::from(math::cos(angle));
    [
        point[0] * cos + point[2] * sin,
        point[1],
        point[2] * cos - point[0] * sin,
    ]
}

fn camel_seat_height(height: f32, first: bool, sitting: bool, elapsed: i64) -> f32 {
    let base = height - 0.375;
    let rise = 1.43f32;
    let span = rise - 0.2;
    let seated = rise - span;
    let duration = if sitting { 40 } else { 52 };
    if elapsed >= duration {
        return base + if sitting { seated } else { 0. };
    }
    let split = if sitting {
        28
    } else if first {
        24
    } else {
        32
    };
    let fraction = if sitting {
        if first { 0.5 } else { 0.1 }
    } else if first {
        0.6
    } else {
        0.35
    };
    let time = (elapsed as f32).clamp(0., duration as f32);
    let early = time < split as f32;
    let progress = if early {
        time / split as f32
    } else {
        (time - split as f32) / (duration - split) as f32
    };
    let middle = rise - fraction * span;
    let (from, to) = if sitting {
        if early {
            (rise, middle)
        } else {
            (middle, seated)
        }
    } else if early {
        (seated - rise, seated - middle)
    } else {
        (seated - middle, 0.)
    };
    base + (from + progress * (to - from))
}

fn kind(class: &str) -> Kind {
    match class {
        "net.minecraft.world.entity.vehicle.boat.Boat"
        | "net.minecraft.world.entity.vehicle.boat.ChestBoat"
        | "net.minecraft.world.entity.vehicle.boat.Raft"
        | "net.minecraft.world.entity.vehicle.boat.ChestRaft" => Kind::Boat,
        "net.minecraft.world.entity.animal.equine.Horse"
        | "net.minecraft.world.entity.animal.equine.Donkey"
        | "net.minecraft.world.entity.animal.equine.Mule"
        | "net.minecraft.world.entity.animal.equine.SkeletonHorse"
        | "net.minecraft.world.entity.animal.equine.ZombieHorse" => Kind::Horse,
        "net.minecraft.world.entity.animal.equine.Llama"
        | "net.minecraft.world.entity.animal.equine.TraderLlama" => Kind::Horse,
        "net.minecraft.world.entity.animal.camel.Camel"
        | "net.minecraft.world.entity.animal.camel.CamelHusk" => Kind::Camel,
        "net.minecraft.world.entity.animal.pig.Pig" => Kind::Pig,
        "net.minecraft.world.entity.monster.Strider" => Kind::Strider,
        "net.minecraft.world.entity.animal.nautilus.Nautilus"
        | "net.minecraft.world.entity.animal.nautilus.ZombieNautilus" => Kind::Nautilus,
        "net.minecraft.world.entity.animal.happyghast.HappyGhast" => Kind::HappyGhast,
        _ => Kind::Server,
    }
}
fn integer(store: &EntityStore, h: Handle, key: &str, default: i32) -> Result<i32> {
    match store.metadata(h, key)? {
        None => Ok(default),
        Some(m) => m
            .int()
            .ok_or_else(|| format!("invalid_mount_integer:{key}")),
    }
}
fn boolean(store: &EntityStore, h: Handle, key: &str, default: bool) -> Result<bool> {
    match store.metadata(h, key)? {
        None => Ok(default),
        Some(m) => m
            .boolean()
            .ok_or_else(|| format!("invalid_mount_boolean:{key}")),
    }
}
fn wrap_degrees(v: f32) -> f32 {
    (v + 180.).rem_euclid(360.) - 180.
}
fn relative(v: &mut VehiclePrediction, input: [f64; 3], speed: f32) {
    let length = input.iter().map(|n| n * n).sum::<f64>();
    if length < 1e-7 {
        return;
    }
    let divisor = length.sqrt().max(1.);
    let x = input[0] / divisor * f64::from(speed);
    let z = input[2] / divisor * f64::from(speed);
    let a = f64::from(v.yaw * 0.017453292);
    let s = f64::from(math::sin(a));
    let c = f64::from(math::cos(a));
    v.velocity[0] += x * c - z * s;
    v.velocity[1] += input[1] / divisor * f64::from(speed);
    v.velocity[2] += z * c + x * s;
}
fn inside_mount(
    world: &World,
    content: &Content,
    v: &mut VehiclePrediction,
    bounds: Bounds,
    visits: &[collision::InsideVisit],
    entity: &crate::entities::Entity,
    kind: Kind,
    fire_immune: bool,
) -> Result<()> {
    let can_freeze = if visits.iter().any(|visit| {
        visit.block_contact && content.name(visit.state) == Some("minecraft:powder_snow")
    }) {
        let immune_items =
            content.registry_tag("minecraft:item", "minecraft:freeze_immune_wearables")?;
        let immune_types = content.registry_tag(
            "minecraft:entity_type",
            "minecraft:freeze_immune_entity_types",
        )?;
        !immune_types.contains(&(entity.kind as i32))
            && !entity.equipment[2..7]
                .iter()
                .flatten()
                .any(|stack| i32::try_from(stack.id).is_ok_and(|id| immune_items.contains(&id)))
    } else {
        true
    };
    for visit in visits {
        if !visit.block_contact {
            continue;
        }
        let [x, y, z] = visit.position;
        let state = visit.state;
        match content.name(state) {
            Some("minecraft:cobweb") => {
                v.stuck_multiplier =
                    if mount_effect(entity, content, "minecraft:weaving")?.is_some() {
                        [0.5, 0.25, 0.5]
                    } else {
                        [0.25, f64::from(0.05f32), 0.25]
                    };
                v.fall_distance = 0.;
            }
            Some("minecraft:sweet_berry_bush") if kind != Kind::Boat => {
                v.stuck_multiplier = [f64::from(0.8f32), 0.75, f64::from(0.8f32)];
                v.fall_distance = 0.;
            }
            Some("minecraft:powder_snow") => {
                if kind == Kind::Boat
                    || content.name(
                        world
                            .block(v.position.map(|value| value.floor() as i32))
                            .ok_or("unloaded_mount_inside_block")?,
                    ) == Some("minecraft:powder_snow")
                {
                    v.stuck_multiplier = [f64::from(0.9f32), 1.5, f64::from(0.9f32)];
                    v.fall_distance = 0.;
                }
            }
            Some("minecraft:bubble_column") if visit.affects_motion => {
                let above = world
                    .block([x, y + 1, z])
                    .ok_or("unloaded_mount_inside_block")?;
                let surface = content.info(above)?.fluid.is_none()
                    && content
                        .collision_boxes(above)
                        .ok_or("unknown_collision_shape")?
                        .is_empty();
                if !(surface && kind == Kind::Boat) {
                    let drag = content
                        .state_properties(state)
                        .and_then(|props| props.get("drag"))
                        .is_some_and(|value| value == "true");
                    v.velocity[1] = if drag {
                        (v.velocity[1] - 0.03).max(if surface { -0.9 } else { -0.3 })
                    } else {
                        (v.velocity[1] + if surface { 0.1 } else { 0.06 }).min(if surface {
                            1.8
                        } else {
                            0.7
                        })
                    };
                    v.fall_distance = 0.;
                }
            }
            Some("minecraft:honey_block") => {
                let old_y = v.velocity[1] / f64::from(0.98f32) + 0.08;
                let width = (bounds.max[0] - bounds.min[0]) / 2.;
                if !v.grounded
                    && v.position[1] <= f64::from(y) + 0.9375 - 1e-7
                    && old_y < -0.08
                    && ((f64::from(x) + 0.5 - v.position[0]).abs() + 1e-7 > 0.4375 + width
                        || (f64::from(z) + 0.5 - v.position[2]).abs() + 1e-7 > 0.4375 + width)
                {
                    if old_y < -0.13 {
                        v.velocity[0] *= -0.05 / old_y;
                        v.velocity[2] *= -0.05 / old_y;
                    }
                    v.velocity[1] = (-0.05 - 0.08) * f64::from(0.98f32);
                    v.fall_distance = 0.;
                }
            }
            _ => {}
        }
    }
    collision::apply_inside_effects(
        content,
        visits,
        &mut v.inside_block_state,
        can_freeze,
        fire_immune,
    )?;
    Ok(())
}

fn prepared_move(v: &mut VehiclePrediction) -> [f64; 3] {
    let mut wanted = v.velocity;
    if v.stuck_multiplier != [0.; 3] {
        for axis in 0..3 {
            wanted[axis] *= v.stuck_multiplier[axis];
        }
        v.velocity = [0.; 3];
        v.stuck_multiplier = [0.; 3];
    }
    wanted
}

fn finish_move(
    v: &mut VehiclePrediction,
    wanted: [f64; 3],
    moved: [f64; 3],
    world: &World,
    content: &Content,
    living: bool,
    efficiency: f32,
) -> Result<()> {
    v.grounded = wanted[1] != moved[1] && wanted[1] < 0.;
    let from = v.position;
    for axis in 0..3 {
        v.position[axis] += moved[axis];
        if axis != 1 && wanted[axis] != moved[axis] {
            v.velocity[axis] = 0.;
        }
    }
    v.last_movement = Some(collision::Movement {
        from,
        to: v.position,
        axis_order: Some(wanted),
    });
    if v.grounded {
        v.fall_distance = 0.;
    } else if moved[1] < 0. {
        v.fall_distance -= moved[1];
    }
    let p = v.position.map(|value| value.floor() as i32);
    let underfoot = world
        .block([p[0], (v.position[1] - 0.2).floor() as i32, p[2]])
        .ok_or("unloaded_mount_collision")?;
    let name = content.name(underfoot).ok_or("unknown_mount_block")?;
    if wanted[1] != moved[1] {
        v.velocity[1] = if v.velocity[1] < 0.
            && (name == "minecraft:slime_block"
                || content.block_has_tag(underfoot, "minecraft:beds")?)
        {
            let bounce = if name == "minecraft:slime_block" {
                1.
            } else {
                f64::from(0.66f32)
            };
            -v.velocity[1] * bounce * if living { 1. } else { 0.8 }
        } else {
            0.
        };
    }
    if v.grounded && name == "minecraft:slime_block" && v.velocity[1].abs() < 0.1 {
        let multiplier = 0.4 + v.velocity[1].abs() * 0.2;
        v.velocity[0] *= multiplier;
        v.velocity[2] *= multiplier;
    }
    let state = world.block(p).ok_or("unloaded_mount_collision")?;
    let current = content.info(state)?;
    let factor = if current.speed_factor != 1.
        || current
            .fluid
            .is_some_and(|fluid| fluid.kind == FluidKind::Water)
        || content.name(state) == Some("minecraft:bubble_column")
    {
        current.speed_factor
    } else {
        content
            .info(
                world
                    .block([p[0], (v.position[1] - 0.5000001).floor() as i32, p[2]])
                    .ok_or("unloaded_mount_collision")?,
            )?
            .speed_factor
    };
    let factor = factor + (1. - factor) * efficiency;
    v.velocity[0] *= f64::from(factor);
    v.velocity[2] *= f64::from(factor);
    Ok(())
}
fn write_vehicle(p: [f64; 3], yaw: f32, pitch: f32, grounded: bool, out: &mut Vec<PacketCommand>) {
    let mut w = Writer::default();
    for d in p {
        w.f64(d);
    }
    w.f32(yaw);
    w.f32(pitch);
    w.byte(u8::from(grounded));
    out.push(PacketCommand::new(sb::MOVE_VEHICLE, w));
}
fn boat_water_above(world: &World, content: &Content, b: Bounds, last_y: f64) -> Result<f32> {
    let end = (b.max[1] - last_y).ceil() as i32;
    for y in b.max[1].floor() as i32..end {
        let mut maximum = 0.0f32;
        for x in b.min[0].floor() as i32..b.max[0].ceil() as i32 {
            for z in b.min[2].floor() as i32..b.max[2].ceil() as i32 {
                let p = [x, y, z];
                let s = world.block(p).ok_or("unloaded_boat_water")?;
                if let Some(f) = content
                    .info(s)?
                    .fluid
                    .filter(|f| f.kind == FluidKind::Water)
                {
                    maximum = maximum.max(Player::fluid_height(world, content, p, f)? as f32);
                }
            }
        }
        if maximum < 1. {
            return Ok(y as f32 + maximum);
        }
    }
    Ok((end + 1) as f32)
}
fn mount_fluid(
    world: &World,
    content: &Content,
    b: Bounds,
    velocity: &mut [f64; 3],
    pushed: bool,
    fast_lava: bool,
) -> Result<(f64, f64)> {
    let contact = super::movement::fluid_contact(world, content, b, pushed)?;
    if pushed {
        super::movement::apply_flow(velocity, contact.flow[0], contact.cells[0], 0.014, true);
        super::movement::apply_flow(
            velocity,
            contact.flow[1],
            contact.cells[1],
            if fast_lava {
                0.007
            } else {
                0.0023333333333333335
            },
            true,
        );
    }
    for component in velocity {
        if component.abs() < 0.003 {
            *component = 0.;
        }
    }
    Ok((contact.depth[0], contact.depth[1]))
}
fn append_lava_colliders(
    world: &World,
    content: &Content,
    b: Bounds,
    area: Bounds,
    out: &mut Vec<Bounds>,
) -> Result<()> {
    for y in area.min[1].floor() as i32..area.max[1].ceil() as i32 {
        for x in area.min[0].floor() as i32..area.max[0].ceil() as i32 {
            for z in area.min[2].floor() as i32..area.max[2].ceil() as i32 {
                let p = [x, y, z];
                let s = world.block(p).ok_or("unloaded_mount_fluid")?;
                if content
                    .info(s)?
                    .fluid
                    .is_some_and(|fluid| fluid.kind == FluidKind::Lava)
                {
                    let above = world.block([x, y + 1, z]).ok_or("unloaded_mount_fluid")?;
                    let top = f64::from(y) + 0.5;
                    if b.min[1] > top - 1e-5
                        && !content
                            .info(above)?
                            .fluid
                            .is_some_and(|fluid| fluid.kind == FluidKind::Lava)
                    {
                        out.push(Bounds {
                            min: [f64::from(x), f64::from(y), f64::from(z)],
                            max: [f64::from(x + 1), top, f64::from(z + 1)],
                        });
                    }
                }
            }
        }
    }
    Ok(())
}
fn mount_effect(e: &crate::entities::Entity, content: &Content, name: &str) -> Result<Option<u32>> {
    for (id, effect) in &e.effects {
        if content.registry_name("minecraft:mob_effect", *id as i32)? == name {
            return Ok(Some(effect.amplifier));
        }
    }
    Ok(None)
}

fn passenger_fluid_box(mut passenger: Bounds, boat: Bounds, underwater: bool) -> Option<Bounds> {
    if underwater {
        return Some(passenger);
    }
    if boat.max[1] >= passenger.max[1] {
        return None;
    }
    passenger.min[1] = passenger.min[1].max(boat.max[1]);
    Some(passenger)
}

fn clamp_mount_climb(velocity: &mut [f64; 3], fall_distance: &mut f64) {
    let limit = f64::from(0.15f32);
    velocity[0] = velocity[0].clamp(-limit, limit);
    velocity[2] = velocity[2].clamp(-limit, limit);
    velocity[1] = velocity[1].max(-limit);
    *fall_distance = 0.;
}

fn fluid_escape_offset(velocity: [f64; 3], y: f64, start_y: f64) -> [f64; 3] {
    [
        velocity[0],
        velocity[1] + f64::from(0.6f32) - y + start_y,
        velocity[2],
    ]
}

fn ridden_buoyancy(velocity: &mut [f64; 3], water_depth: f64, threshold: f64, can_float: bool) {
    if can_float && water_depth > threshold {
        velocity[1] += f64::from(0.04f32);
    }
}

fn fluid_fall(velocity: &mut [f64; 3], gravity: f64, falling: bool, sprinting: bool) {
    if gravity != 0. && !sprinting {
        let y = velocity[1];
        velocity[1] = if falling && (y - 0.005).abs() >= 0.003 && (y - gravity / 16.).abs() < 0.003
        {
            -0.003
        } else {
            y - gravity / 16.
        };
    }
}

fn mount_jump_boost(e: &crate::entities::Entity, content: &Content, _tick: u64) -> Result<f64> {
    for (id, effect) in &e.effects {
        if content.registry_name("minecraft:mob_effect", *id as i32)? == "minecraft:jump_boost" {
            return Ok(f64::from(0.1f32 * (effect.amplifier + 1) as f32));
        }
    }
    Ok(0.)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mounted_climb_limits_fall_but_never_inherits_player_sneak_suppression() {
        let mut velocity = [0.4, -0.8, -0.3];
        let mut fall_distance = 8.;
        clamp_mount_climb(&mut velocity, &mut fall_distance);
        assert_eq!(
            velocity,
            [f64::from(0.15f32), -f64::from(0.15f32), -f64::from(0.15f32)]
        );
        assert_eq!(fall_distance, 0.);
        velocity[1] = 0.5;
        clamp_mount_climb(&mut velocity, &mut fall_distance);
        assert_eq!(velocity[1], 0.5);
    }

    #[test]
    fn ridden_float_threshold_is_strict_and_escape_probe_preserves_tick_start_height() {
        let mut velocity = [0.1, -0.02, 0.2];
        ridden_buoyancy(&mut velocity, 0.4, 0.4, true);
        assert_eq!(velocity[1], -0.02);
        ridden_buoyancy(&mut velocity, 0.4000001, 0.4, false);
        assert_eq!(velocity[1], -0.02);
        ridden_buoyancy(&mut velocity, 0.4000001, 0.4, true);
        assert_eq!(velocity[1], -0.02 + f64::from(0.04f32));
        let escape = fluid_escape_offset(velocity, 2.2, 2.);
        assert_eq!(
            escape,
            [0.1, velocity[1] + f64::from(0.6f32) - 2.2 + 2., 0.2]
        );
    }
    #[test]
    fn boat_fluid_box_keeps_exposed_passenger_and_restores_submerged_body() {
        let passenger = Bounds::new([-0.3, 0.2, -0.3, 0.3, 1.8, 0.3]).deflated(0.001);
        let mut boat = Bounds::new([-0.7, 0., -0.7, 0.7, 0.6, 0.7]);
        let exposed = passenger_fluid_box(passenger, boat, false).unwrap();
        assert_eq!(exposed.min, [passenger.min[0], 0.6, passenger.min[2]]);
        assert_eq!(exposed.max, passenger.max);
        assert_eq!(passenger_fluid_box(passenger, boat, true), Some(passenger));
        boat.max[1] = passenger.max[1];
        assert_eq!(passenger_fluid_box(passenger, boat, false), None);
        boat.max[1] = passenger.min[1] - 0.1;
        assert_eq!(passenger_fluid_box(passenger, boat, false), Some(passenger));
    }
    #[test]
    fn camel_seat_pose_transitions_are_continuous_at_both_stages() {
        let seated_height = 2.375f32 - 1.43;
        let seated = camel_seat_height(seated_height, true, true, 40);
        let standing = camel_seat_height(2.375, true, false, 52);
        assert!((camel_seat_height(seated_height, true, true, 0) - standing).abs() < 1e-6);
        assert!((camel_seat_height(2.375, true, false, 0) - seated).abs() < 1e-6);
        for first in [false, true] {
            let split = if first { 24 } else { 32 };
            let before = camel_seat_height(2.375, first, false, split - 1);
            let at = camel_seat_height(2.375, first, false, split);
            let after = camel_seat_height(2.375, first, false, split + 1);
            assert!(before < at && at < after);
        }
    }
    fn prediction() -> VehiclePrediction {
        VehiclePrediction {
            handle: Handle {
                network_id: 4,
                generation: 2,
            },
            position: [0.; 3],
            velocity: [0.; 3],
            yaw: 0.,
            pitch: 0.,
            grounded: true,
            fall_distance: 0.,
            stuck_multiplier: [0.; 3],
            last_movement: None,
            inside_block_state: collision::InsideBlockState::default(),
            wire_position: [0.; 3],
            wire_velocity: [0.; 3],
            wire_yaw: 0.,
            wire_pitch: 0.,
            delta_rotation: 0.,
            last_y_delta: 0.,
            status: BoatStatus::Air,
            riding_jump_ticks: 0,
            riding_jump_scale: 0.,
            pending_jump: 0.,
            dash_cooldown: 0,
            was_dashing: false,
            allow_stand_sliding: false,
            stand_animation: 0.,
            stand_animation_old: 0.,
            boost_total: 0,
            boost_ticks: 0,
            boosting: false,
        }
    }
    #[test]
    fn mount_jump_release_is_once_and_quantized() {
        let mut v = prediction();
        let jump = HeldInput {
            flags: 16,
            ..HeldInput::default()
        };
        let empty = HeldInput::default();
        assert_eq!(v.charge_jump(jump, empty), None);
        for _ in 0..9 {
            assert_eq!(v.charge_jump(jump, jump), None);
        }
        assert_eq!(v.charge_jump(empty, jump), Some(90));
        assert_eq!(v.pending_jump, 1.);
        assert_eq!(v.charge_jump(empty, empty), None);
    }
    #[test]
    fn controlled_pig_boost_expires_and_does_not_restart_without_server_change() {
        let mut v = prediction();
        assert!(v.boost(140) > 1.);
        for _ in 0..142 {
            v.boost(140);
        }
        assert_eq!(v.boost(140), 1.);
        assert_eq!(v.boost(0), 1.);
        assert!(v.boost(141) > 1.);
    }
    #[test]
    fn minecart_is_server_interpolated_not_boat_predicted() {
        assert_eq!(
            kind("net.minecraft.world.entity.vehicle.minecart.Minecart"),
            Kind::Server
        );
        assert_eq!(
            kind("net.minecraft.world.entity.vehicle.boat.Raft"),
            Kind::Boat
        );
    }
}
