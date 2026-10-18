use super::collision;
use super::*;
use crate::inventory::Stack;
use crate::protocol::content::FluidState;

#[derive(Default)]
struct Environment {
    water: f64,
    lava: f64,
    flow_water: [f64; 3],
    flow_lava: [f64; 3],
    water_cells: u32,
    lava_cells: u32,
    climbable: bool,
    powder: bool,
    inside_state: u32,
    below_state: u32,
}
impl Player {
    pub fn configure_dimension(&mut self, dimension_type: &Value) -> Result<()> {
        self.fast_lava = dimension_fast_lava(dimension_type)?;
        Ok(())
    }
    pub(super) fn start_auto_spin(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<()> {
        self.pose = Pose::SpinAttack;
        if self.grounded {
            let impulse = self.velocity;
            let rise = f64::from(1.1999999f32);
            let old_y = self.position[1];
            self.velocity = [0., rise, 0.];
            self.move_player(
                world,
                content,
                entities,
                HeldInput::default(),
                inventory,
                tick,
                [0.; 3],
            )?;
            let hit_ceiling = self.position[1] - old_y != rise;
            self.velocity = impulse;
            if hit_ceiling {
                self.velocity[1] = 0.;
            }
        }
        Ok(())
    }
    pub(super) fn predict(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        input: HeldInput,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        let tick_start_position = self.position;
        let spin_start = self.bounds();
        if self.jump_delay > 0 {
            self.jump_delay -= 1;
        }
        if self.jump_trigger > 0 {
            self.jump_trigger -= 1;
        }
        if self.sprint_trigger > 0 {
            self.sprint_trigger -= 1;
        }
        if let Some(vehicle) = entities.vehicle_of(self.entity_id) {
            self.vehicle = Some(vehicle);
            self.predict_vehicle(world, content, inventory, entities, input, tick, out)?;
            self.inside_blocks(world, content, inventory, input, tick_start_position)?;
            self.update_auto_spin(world, content, inventory, entities, input, tick, spin_start)?;
            self.update_eye_environment(world, content)?;
            return Ok(());
        }
        self.vehicle = None;
        self.vehicle_prediction = None;
        let env = self.environment(world, content)?;
        let underwater = self.update_eye_environment(world, content)?;
        if !self.abilities.flying {
            apply_flow(
                &mut self.velocity,
                env.flow_water,
                env.water_cells,
                0.014,
                false,
            );
            apply_flow(
                &mut self.velocity,
                env.flow_lava,
                env.lava_cells,
                if self.fast_lava {
                    0.007
                } else {
                    0.0023333333333333335
                },
                false,
            );
        }
        let sprint_allowed = (self.food.unwrap_or(20) > 6 || self.abilities.may_fly)
            && self.amplifier("minecraft:blindness", tick).is_none();
        let moving_slow = self.pose == Pose::Crouching
            || (self.pose == Pose::Swimming && env.water == 0.)
            || (input.has(5) && !self.abilities.flying && !self.swimming);
        let sprint_possible =
            sprint_allowed && (self.abilities.flying || env.water == 0. || underwater);
        let slow_use = self.using.as_ref().is_some_and(|using| !using.can_sprint);
        if self.last_input.has(5) || slow_use || input.has(1) {
            self.sprint_trigger = 0;
        }
        if !self.sprinting
            && input.forward() > 1e-5
            && sprint_possible
            && !slow_use
            && (!self.gliding || underwater)
            && (!moving_slow || underwater)
        {
            let rising = self.last_input.forward() <= 1e-5;
            if input.has(6) || (rising && self.sprint_trigger > 0) {
                self.sprinting = true;
            } else if rising {
                self.sprint_trigger = 7;
            }
        }
        if self.sprinting
            && if self.swimming {
                !sprint_allowed
                    || env.water == 0.
                    || (input.forward() <= 1e-5 && !self.grounded && !input.has(5))
            } else {
                !sprint_possible
                    || input.forward() <= 1e-5
                    || (self.horizontal_collision && !self.minor_horizontal_collision)
            }
        {
            self.sprinting = false;
        }
        if self.sprinting != self.sent_sprinting {
            // Hunger, collision and use effects can deny a held sprint key.
            self.entity_command(if self.sprinting { 3 } else { 4 }, 0, out);
            self.sent_sprinting = self.sprinting;
        }
        if input.has(5) != self.last_input.has(5) {
            self.entity_command(if input.has(5) { 0 } else { 1 }, 0, out);
        }
        self.swimming = if self.abilities.flying {
            false
        } else if self.swimming {
            self.sprinting && env.water > 0.
        } else {
            self.sprinting && underwater && env.water > 0.
        };
        let jump_edge = input.has(4) && !self.last_input.has(4);
        if self.abilities.may_fly && jump_edge && !self.swimming {
            if self.game_mode == 3 {
                self.abilities.flying = true;
            } else if self.jump_trigger == 0 {
                self.jump_trigger = 7;
            } else {
                self.abilities.flying = !self.abilities.flying;
                self.jump_trigger = 0;
                self.send_abilities(out);
            }
        } else if jump_edge && !env.climbable && self.can_glide(inventory, tick) && env.water == 0.
        {
            self.gliding = true;
            self.entity_command(8, 0, out);
        }
        if self.gliding && (self.grounded || env.water > 0. || !self.can_glide(inventory, tick)) {
            self.gliding = false;
        }
        self.choose_pose(world, content, inventory, entities, input, tick)?;
        if self.velocity[0] * self.velocity[0] + self.velocity[2] * self.velocity[2] < 9e-6 {
            self.velocity[0] = 0.;
            self.velocity[2] = 0.;
        }
        if self.velocity[1].abs() < 0.003 {
            self.velocity[1] = 0.;
        }
        let flight = self.abilities.flying;
        let old_y = self.velocity[1];
        if flight {
            self.velocity[1] += f64::from(
                (u8::from(input.has(4)) as f32 - u8::from(input.has(5)) as f32)
                    * self.abilities.flying_speed
                    * 3.,
            );
        } else {
            if env.water > 0. && input.has(5) {
                self.velocity[1] -= 0.04;
            }
            if input.has(4) {
                if env.water > 0.4
                    || env.lava > 0.4
                    || (!self.grounded && (env.water > 0. || env.lava > 0.))
                {
                    self.velocity[1] += 0.04;
                } else if (self.grounded || env.water > 0.) && self.jump_delay == 0 {
                    let factor =
                        self.block_factor(content, env.inside_state, env.below_state, true)?;
                    let boost = self
                        .amplifier("minecraft:jump_boost", tick)
                        .map_or(0., |a| (a + 1) as f32 * 0.1);
                    let power = self.attribute("minecraft:jump_strength") as f32 * factor + boost;
                    if power > 1e-5 {
                        self.velocity[1] = self.velocity[1].max(f64::from(power));
                        if self.sprinting {
                            let yaw = self.yaw * 0.017453292;
                            self.velocity[0] -= f64::from(math::sin(f64::from(yaw)) * 0.2);
                            self.velocity[2] += f64::from(math::cos(f64::from(yaw)) * 0.2);
                        }
                        self.jump_delay = 10;
                    }
                }
            } else {
                self.jump_delay = 0;
            }
        }
        let falling = self.velocity[1] <= 0.;
        let travel_start_y = self.position[1];
        let movement = modified_input(
            input,
            if moving_slow {
                self.attribute("minecraft:sneaking_speed") as f32
            } else {
                1.
            },
            self.using
                .as_ref()
                .map_or(1., |using| using.movement_factor),
        );
        let gravity =
            if self.velocity[1] <= 0. && self.amplifier("minecraft:slow_falling", tick).is_some() {
                self.attribute("minecraft:gravity").min(0.01)
            } else {
                self.attribute("minecraft:gravity")
            };
        let speed = self.attribute("minecraft:movement_speed") as f32
            * if self.sprinting { 1.3 } else { 1. };
        if self.game_mode == 3 {
            self.add_relative(
                movement,
                self.abilities.flying_speed * if self.sprinting { 2. } else { 1. },
            );
            for i in 0..3 {
                self.position[i] += self.velocity[i];
            }
            self.velocity[0] *= f64::from(0.91f32);
            self.velocity[1] *= 0.6;
            self.velocity[2] *= f64::from(0.91f32);
            self.grounded = false;
            self.horizontal_collision = false;
            return Ok(());
        }
        if self.gliding && env.climbable {
            self.gliding = false;
        }
        if env.water > 0. && !flight {
            if self.swimming {
                let look = self.look_direction()[1];
                if look <= 0.
                    || input.has(4)
                    || self.fluid_at(
                        world,
                        content,
                        [self.position[0], self.position[1] + 0.9, self.position[2]],
                        FluidKind::Water,
                    )?
                {
                    self.velocity[1] +=
                        (look - self.velocity[1]) * if look < -0.2 { 0.085 } else { 0.06 };
                }
            }
            let mut drag = if self.sprinting { 0.9f32 } else { 0.8 };
            let mut acceleration = 0.02f32;
            let efficiency = self.attribute("minecraft:water_movement_efficiency") as f32
                * if self.grounded { 1. } else { 0.5 };
            if efficiency > 0. {
                drag += (0.54600006 - drag) * efficiency;
                acceleration += (speed - acceleration) * efficiency;
            }
            if self.amplifier("minecraft:dolphins_grace", tick).is_some() {
                drag = 0.96;
            }
            self.add_relative(movement, acceleration);
            self.move_player(world, content, entities, input, inventory, tick, movement)?;
            if self.horizontal_collision && env.climbable {
                self.velocity[1] = 0.2;
            }
            self.velocity[0] *= f64::from(drag);
            self.velocity[1] *= f64::from(0.8f32);
            self.velocity[2] *= f64::from(drag);
            self.fluid_gravity(gravity, falling);
            self.jump_out_of_fluid(
                world,
                content,
                inventory,
                entities,
                input,
                tick,
                travel_start_y,
            )?;
        } else if env.lava > 0. && !flight {
            self.add_relative(movement, 0.02);
            self.move_player(world, content, entities, input, inventory, tick, movement)?;
            if env.lava <= 0.4 {
                self.velocity[0] *= 0.5;
                self.velocity[1] *= f64::from(0.8f32);
                self.velocity[2] *= 0.5;
                self.fluid_gravity(gravity, falling);
            } else {
                for v in &mut self.velocity {
                    *v *= 0.5;
                }
            }
            self.velocity[1] -= gravity / 4.;
            self.jump_out_of_fluid(
                world,
                content,
                inventory,
                entities,
                input,
                tick,
                travel_start_y,
            )?;
        } else if self.gliding {
            self.glide(gravity);
            self.move_player(world, content, entities, input, inventory, tick, movement)?;
        } else {
            let friction = if self.grounded {
                content.info(env.below_state)?.friction
            } else {
                1.
            };
            let acceleration = if self.grounded {
                speed * (0.21600002 / (friction * friction * friction))
            } else if flight {
                self.abilities.flying_speed * if self.sprinting { 2. } else { 1. }
            } else if self.sprinting {
                0.025999999
            } else {
                0.02
            };
            self.add_relative(movement, acceleration);
            if env.climbable && !flight {
                self.velocity[0] = self.velocity[0].clamp(-f64::from(0.15f32), f64::from(0.15f32));
                self.velocity[2] = self.velocity[2].clamp(-f64::from(0.15f32), f64::from(0.15f32));
                self.velocity[1] = self.velocity[1].max(-f64::from(0.15f32));
                if input.has(5) && content.name(env.inside_state) != Some("minecraft:scaffolding") {
                    self.velocity[1] = self.velocity[1].max(0.);
                }
                self.fall_distance = 0.;
            }
            self.move_player(world, content, entities, input, inventory, tick, movement)?;
            if (self.horizontal_collision || input.has(4))
                && (env.climbable || (env.powder && can_walk_powder(inventory, content)))
            {
                self.velocity[1] = 0.2;
            }
            if let Some(amplifier) = self.amplifier("minecraft:levitation", tick) {
                self.velocity[1] += (0.05 * f64::from(amplifier + 1) - self.velocity[1]) * 0.2;
                self.fall_distance = 0.;
            } else {
                self.velocity[1] -= gravity;
            }
            self.velocity[0] *= f64::from(friction * 0.91);
            self.velocity[1] *= f64::from(0.98f32);
            self.velocity[2] *= f64::from(friction * 0.91);
        }
        if flight {
            self.velocity[1] = (old_y
                + f64::from(
                    (u8::from(input.has(4)) as f32 - u8::from(input.has(5)) as f32)
                        * self.abilities.flying_speed
                        * 3.,
                ))
                * 0.6;
            self.fall_distance = 0.;
            if self.grounded && self.game_mode != 3 {
                self.abilities.flying = false;
                self.send_abilities(out);
            }
        }
        self.inside_blocks(world, content, inventory, input, tick_start_position)?;
        if !input.has(5) {
            self.accumulate_steps(world, content, tick_start_position)?;
        }
        self.update_auto_spin(world, content, inventory, entities, input, tick, spin_start)?;
        Ok(())
    }
    fn update_auto_spin(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        input: HeldInput,
        tick: u64,
        spin_start: Bounds,
    ) -> Result<()> {
        if self.auto_spin_ticks > 0 {
            self.auto_spin_ticks -= 1;
            let current = self.bounds();
            let swept = Bounds {
                min: std::array::from_fn(|i| spin_start.min[i].min(current.min[i])),
                max: std::array::from_fn(|i| spin_start.max[i].max(current.max[i])),
            };
            if entities.has_living_contact(swept, self.entity_id, tick)? {
                self.auto_spin_ticks = 0;
                for component in &mut self.velocity {
                    *component *= -0.2;
                }
            } else if self.horizontal_collision {
                self.auto_spin_ticks = 0;
            }
            if self.auto_spin_ticks == 0 {
                self.choose_pose(world, content, inventory, entities, input, tick)?;
            }
        }
        Ok(())
    }

    fn update_eye_environment(&mut self, world: &World, content: &Content) -> Result<bool> {
        let underwater = self.eye_medium(world, content)? == Some(FluidKind::Water);
        self.eye_in_water = underwater;
        self.water_vision_ticks = if underwater {
            (self.water_vision_ticks + if self.game_mode == 3 { 10 } else { 1 }).min(600)
        } else {
            self.water_vision_ticks.saturating_sub(10)
        };
        let t = self.water_vision_ticks as f32;
        self.water_vision = if underwater {
            (t / 100.).clamp(0., 1.) * 0.6 + ((t - 100.) / 500.).clamp(0., 1.) * 0.39999998
        } else {
            0.
        };
        Ok(underwater)
    }
    fn block_factor(&self, content: &Content, inside: u32, below: u32, jump: bool) -> Result<f32> {
        let current = content.info(inside)?;
        let f = if jump {
            current.jump_factor
        } else {
            current.speed_factor
        };
        if f != 1.
            || (!jump
                && (current.fluid.is_some()
                    || content.name(inside) == Some("minecraft:bubble_column")))
        {
            return Ok(f);
        }
        let below = content.info(below)?;
        Ok(if jump {
            below.jump_factor
        } else {
            below.speed_factor
        })
    }
    fn environment(&self, world: &World, content: &Content) -> Result<Environment> {
        let p = self.position.map(|v| v.floor() as i32);
        let inside_state = world.block(p).ok_or("unloaded_collision_neighborhood")?;
        let below = [p[0], (self.position[1] - 0.5000001).floor() as i32, p[2]];
        let below_state = world
            .block(below)
            .ok_or("unloaded_collision_neighborhood")?;
        let mut env = Environment {
            inside_state,
            below_state,
            powder: content.name(inside_state) == Some("minecraft:powder_snow"),
            ..Environment::default()
        };
        env.climbable = !self.abilities.flying
            && collision::climbable(world, content, self.position)?
            && !(self.gliding
                && self.has_block_tag(content, inside_state, "minecraft:can_glide_through")?);
        let fluid = fluid_contact(world, content, self.bounds(), !self.abilities.flying)?;
        env.water = fluid.depth[0];
        env.lava = fluid.depth[1];
        env.water_cells = fluid.cells[0];
        env.lava_cells = fluid.cells[1];
        env.flow_water = fluid.flow[0];
        env.flow_lava = fluid.flow[1];
        Ok(env)
    }
    pub(super) fn has_block_tag(&self, content: &Content, state: u32, tag: &str) -> Result<bool> {
        content.block_has_tag(state, tag)
    }
    pub(crate) fn fluid_height(
        world: &World,
        content: &Content,
        pos: [i32; 3],
        fluid: FluidState,
    ) -> Result<f64> {
        let above = world
            .block([pos[0], pos[1] + 1, pos[2]])
            .ok_or("unloaded_fluid_neighborhood")?;
        Ok(
            if content
                .info(above)?
                .fluid
                .is_some_and(|f| f.kind == fluid.kind)
            {
                1.
            } else {
                f64::from(fluid.height())
            },
        )
    }
    fn fluid_at(
        &self,
        world: &World,
        content: &Content,
        point: [f64; 3],
        kind: FluidKind,
    ) -> Result<bool> {
        let p = point.map(|v| v.floor() as i32);
        let state = world.block(p).ok_or("unloaded_fluid_neighborhood")?;
        let Some(f) = content.info(state)?.fluid else {
            return Ok(false);
        };
        Ok(
            f.kind == kind
                && point[1] < f64::from(p[1]) + Self::fluid_height(world, content, p, f)?,
        )
    }
    fn choose_pose(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        input: HeldInput,
        tick: u64,
    ) -> Result<()> {
        let desired = if self.health.is_some_and(|h| h <= 0.) {
            Pose::Dying
        } else if self.pose == Pose::Sleeping {
            Pose::Sleeping
        } else if self.swimming {
            Pose::Swimming
        } else if self.gliding {
            Pose::FallFlying
        } else if self.auto_spin_ticks > 0 {
            Pose::SpinAttack
        } else if input.has(5) && !self.abilities.flying {
            Pose::Crouching
        } else {
            Pose::Standing
        };
        if self.game_mode == 3 || self.vehicle.is_some() {
            self.pose = desired;
            return Ok(());
        }
        let context = self.collision_context(inventory, content, input);
        let swimming_bounds = self.bounds_for(Pose::Swimming).deflated(1e-7);
        collision::gather_with_context(
            world,
            content,
            entities,
            tick,
            swimming_bounds,
            Some(context),
            &mut self.colliders,
        )?;
        entities.collision_bounds(
            swimming_bounds,
            self.entity_id,
            self.vehicle,
            tick,
            self.position,
            world,
            content,
            &mut self.colliders,
        )?;
        if !self.colliders.is_empty() {
            return Ok(());
        }
        let desired_bounds = self.bounds_for(desired).deflated(1e-7);
        collision::gather_with_context(
            world,
            content,
            entities,
            tick,
            desired_bounds,
            Some(context),
            &mut self.colliders,
        )?;
        entities.collision_bounds(
            desired_bounds,
            self.entity_id,
            self.vehicle,
            tick,
            self.position,
            world,
            content,
            &mut self.colliders,
        )?;
        if self.colliders.is_empty() {
            self.pose = desired;
        } else {
            let crouching_bounds = self.bounds_for(Pose::Crouching).deflated(1e-7);
            collision::gather_with_context(
                world,
                content,
                entities,
                tick,
                crouching_bounds,
                Some(context),
                &mut self.colliders,
            )?;
            entities.collision_bounds(
                crouching_bounds,
                self.entity_id,
                self.vehicle,
                tick,
                self.position,
                world,
                content,
                &mut self.colliders,
            )?;
            self.pose = if self.colliders.is_empty() {
                Pose::Crouching
            } else {
                Pose::Swimming
            };
        }
        Ok(())
    }
    fn add_relative(&mut self, input: [f32; 3], speed: f32) {
        let length = input.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>();
        if length < 1e-7 {
            return;
        }
        let length = length.sqrt().max(1.);
        let x = f64::from(input[0]) / length * f64::from(speed);
        let z = f64::from(input[2]) / length * f64::from(speed);
        let yaw = self.yaw * 0.017453292;
        let (s, c) = (math::sin(f64::from(yaw)), math::cos(f64::from(yaw)));
        self.velocity[0] += x * f64::from(c) - z * f64::from(s);
        self.velocity[1] += f64::from(input[1]) / length * f64::from(speed);
        self.velocity[2] += z * f64::from(c) + x * f64::from(s);
    }
    fn move_player(
        &mut self,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        input: HeldInput,
        inventory: &InventoryStore,
        tick: u64,
        movement: [f32; 3],
    ) -> Result<()> {
        let bounds = self.bounds();
        let step = self.attribute("minecraft:step_height") as f32;
        let mut wanted = self.velocity;
        if self.stuck_multiplier != [0.; 3] {
            for i in 0..3 {
                wanted[i] *= self.stuck_multiplier[i];
            }
            self.velocity = [0.; 3];
            self.stuck_multiplier = [0.; 3];
        }
        let area = bounds
            .expanded([wanted[0], wanted[1].max(f64::from(step)), wanted[2]])
            .expanded([0., wanted[1].min(0.) - f64::from(step), 0.]);
        let context = self.collision_context(inventory, content, input);
        collision::gather_with_context(
            world,
            content,
            entities,
            tick,
            area,
            Some(context),
            &mut self.colliders,
        )?;
        entities.collision_bounds(
            area,
            self.entity_id,
            self.vehicle,
            tick,
            self.position,
            world,
            content,
            &mut self.colliders,
        )?;
        if input.has(5)
            && !self.abilities.flying
            && wanted[1] <= 0.
            && (self.grounded
                || (self.fall_distance < f64::from(step)
                    && !collision::can_fall_at_least(
                        bounds,
                        &self.colliders,
                        0.,
                        0.,
                        f64::from(step) - self.fall_distance,
                    )))
        {
            for axis in [0, 2] {
                while wanted[axis] != 0. {
                    let (x, z) = if axis == 0 {
                        (wanted[axis], 0.)
                    } else {
                        (0., wanted[axis])
                    };
                    if !collision::can_fall_at_least(bounds, &self.colliders, x, z, f64::from(step))
                    {
                        break;
                    }
                    wanted[axis] = towards_zero(wanted[axis], 0.05);
                }
            }
            while wanted[0] != 0.
                && wanted[2] != 0.
                && collision::can_fall_at_least(
                    bounds,
                    &self.colliders,
                    wanted[0],
                    wanted[2],
                    f64::from(step),
                )
            {
                wanted[0] = towards_zero(wanted[0], 0.05);
                wanted[2] = towards_zero(wanted[2], 0.05);
            }
        }
        let moved = collision::with_step(
            bounds,
            &self.colliders,
            wanted,
            self.grounded,
            step,
            &mut self.step_heights,
        );
        let vertical = wanted[1] != moved[1];
        self.horizontal_collision = wanted[0] != moved[0] || wanted[2] != moved[2];
        self.minor_horizontal_collision =
            self.horizontal_collision && minor_collision(self.yaw, movement, moved);
        self.grounded = vertical && wanted[1] < 0.;
        let from = self.position;
        for i in 0..3 {
            self.position[i] += moved[i];
            if i != 1 && wanted[i] != moved[i] {
                self.velocity[i] = 0.;
            }
        }
        self.inside_movements.push(collision::Movement {
            from,
            to: self.position,
            axis_order: Some(wanted),
        });
        if self.grounded {
            self.fall_distance = 0.;
        } else if moved[1] < 0. {
            self.fall_distance -= moved[1];
        }
        if vertical {
            let below = [
                self.position[0].floor() as i32,
                (self.position[1] - 0.2).floor() as i32,
                self.position[2].floor() as i32,
            ];
            let state = world
                .block(below)
                .ok_or("unloaded_collision_neighborhood")?;
            self.velocity[1] = if (content.name(state) == Some("minecraft:slime_block")
                || content.block_has_tag(state, "minecraft:beds")?)
                && !input.has(5)
                && self.velocity[1] < 0.
            {
                -self.velocity[1]
                    * if content.name(state) == Some("minecraft:slime_block") {
                        1.
                    } else {
                        f64::from(0.66f32)
                    }
            } else {
                0.
            };
            if content.name(state) == Some("minecraft:slime_block")
                && self.velocity[1].abs() < 0.1
                && !input.has(5)
            {
                let f = 0.4 + self.velocity[1].abs() * 0.2;
                self.velocity[0] *= f;
                self.velocity[2] *= f;
            }
        }
        // Entity.move applies the destination block's factor before travel applies drag/gravity.
        if !self.abilities.flying && !self.gliding {
            let p = self.position.map(|v| v.floor() as i32);
            let inside = world.block(p).ok_or("unloaded_collision_neighborhood")?;
            let below = world
                .block([p[0], (self.position[1] - 0.5000001).floor() as i32, p[2]])
                .ok_or("unloaded_collision_neighborhood")?;
            let factor = self.block_factor(content, inside, below, false)?;
            let factor =
                factor + (1. - factor) * self.attribute("minecraft:movement_efficiency") as f32;
            self.velocity[0] *= f64::from(factor);
            self.velocity[2] *= f64::from(factor);
        }
        Ok(())
    }
    fn collision_context(
        &self,
        inventory: &InventoryStore,
        content: &Content,
        input: HeldInput,
    ) -> collision::Context {
        collision::Context {
            bottom: self.position[1],
            descending: input.has(5),
            fall_distance: self.fall_distance,
            walks_powder_snow: can_walk_powder(inventory, content),
        }
    }
    fn fluid_gravity(&mut self, gravity: f64, falling: bool) {
        if gravity == 0. || self.sprinting {
            return;
        }
        let y = self.velocity[1];
        self.velocity[1] =
            if falling && (y - 0.005).abs() >= 0.003 && (y - gravity / 16.).abs() < 0.003 {
                -0.003
            } else {
                y - gravity / 16.
            };
    }
    fn jump_out_of_fluid(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        input: HeldInput,
        tick: u64,
        start_y: f64,
    ) -> Result<()> {
        if !self.horizontal_collision {
            return Ok(());
        }
        let b = self.bounds().moved([
            self.velocity[0],
            self.velocity[1] + f64::from(0.6f32) - self.position[1] + start_y,
            self.velocity[2],
        ]);
        let context = self.collision_context(inventory, content, input);
        collision::gather_with_context(
            world,
            content,
            entities,
            tick,
            b,
            Some(context),
            &mut self.colliders,
        )?;
        entities.collision_bounds(
            b,
            self.entity_id,
            self.vehicle,
            tick,
            self.position,
            world,
            content,
            &mut self.colliders,
        )?;
        if self.colliders.is_empty() && !contains_fluid(world, content, b)? {
            self.velocity[1] = f64::from(0.3f32);
        }
        Ok(())
    }
    fn glide(&mut self, gravity: f64) {
        let look = self.look_direction();
        let pitch = f64::from(self.pitch * 0.017453292);
        let horizontal = (look[0] * look[0] + look[2] * look[2]).sqrt();
        let speed =
            (self.velocity[0] * self.velocity[0] + self.velocity[2] * self.velocity[2]).sqrt();
        let lift = f64::from(math::cos(f64::from(pitch as f32))).powi(2);
        self.velocity[1] += gravity * (-1. + lift * 0.75);
        if self.velocity[1] < 0. && horizontal > 0. {
            let d = self.velocity[1] * -0.1 * lift;
            self.velocity[0] += look[0] * d / horizontal;
            self.velocity[1] += d;
            self.velocity[2] += look[2] * d / horizontal;
        }
        if pitch < 0. && horizontal > 0. {
            let d = speed * f64::from(-math::sin(f64::from(pitch as f32))) * 0.04;
            self.velocity[0] -= look[0] * d / horizontal;
            self.velocity[1] += d * 3.2;
            self.velocity[2] -= look[2] * d / horizontal;
        }
        if horizontal > 0. {
            self.velocity[0] += (look[0] / horizontal * speed - self.velocity[0]) * 0.1;
            self.velocity[2] += (look[2] / horizontal * speed - self.velocity[2]) * 0.1;
        }
        self.velocity[0] *= f64::from(0.99f32);
        self.velocity[1] *= f64::from(0.98f32);
        self.velocity[2] *= f64::from(0.99f32);
    }
    pub(super) fn can_glide(&self, inventory: &InventoryStore, tick: u64) -> bool {
        !self.grounded
            && self.vehicle.is_none()
            && !self.abilities.flying
            && self.amplifier("minecraft:levitation", tick).is_none()
            && [
                (5, "head"),
                (6, "chest"),
                (7, "legs"),
                (8, "feet"),
                (45, "offhand"),
                (36 + inventory.selected_slot as usize, "mainhand"),
            ]
            .iter()
            .any(|(index, slot)| {
                inventory
                    .player
                    .slots
                    .get(*index)
                    .and_then(Option::as_ref)
                    .is_some_and(|stack| {
                        stack.components.contains_key("minecraft:glider")
                            && stack
                                .components
                                .get("minecraft:equippable")
                                .and_then(|v| v.get("slot"))
                                .and_then(Value::as_str)
                                == Some(*slot)
                            && !next_damage_breaks(stack)
                    })
            })
    }
    fn inside_blocks(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        input: HeldInput,
        tick_start_position: [f64; 3],
    ) -> Result<()> {
        if self.game_mode == 3 {
            self.inside_movements.clear();
            return Ok(());
        }
        if self.inside_movements.is_empty() {
            self.inside_movements.push(collision::Movement {
                from: tick_start_position,
                to: self.position,
                axis_order: None,
            });
        }
        if let Some(last) = self.inside_movements.last() {
            if (0..3)
                .map(|i| (last.to[i] - self.position[i]).powi(2))
                .sum::<f64>()
                > f64::from(1e-5f32 * 1e-5f32)
            {
                self.inside_movements.push(collision::Movement {
                    from: last.to,
                    to: self.position,
                    axis_order: None,
                });
            }
        }
        collision::collect_inside(
            world,
            content,
            &self.inside_movements,
            self.bounds(),
            self.position,
            self.collision_context(inventory, content, input),
            &mut self.inside_visits,
            &mut self.inside_effects,
        )?;
        self.inside_movements.clear();
        let can_freeze = if self.inside_effects.iter().any(|visit| {
            visit.block_contact && content.name(visit.state) == Some("minecraft:powder_snow")
        }) {
            let immune =
                content.registry_tag("minecraft:item", "minecraft:freeze_immune_wearables")?;
            !inventory.player.slots[5..9]
                .iter()
                .flatten()
                .any(|stack| i32::try_from(stack.id).is_ok_and(|id| immune.contains(&id)))
        } else {
            true
        };
        for visit in &self.inside_effects {
            if !visit.block_contact {
                continue;
            }
            let [x, y, z] = visit.position;
            let state = visit.state;
            match content.name(state) {
                Some("minecraft:cobweb") => {
                    self.stuck_multiplier = if self.effects.contains_key("minecraft:weaving") {
                        [0.5, 0.25, 0.5]
                    } else {
                        [0.25, f64::from(0.05f32), 0.25]
                    };
                    self.fall_distance = 0.;
                }
                Some("minecraft:sweet_berry_bush") => {
                    self.stuck_multiplier = [f64::from(0.8f32), 0.75, f64::from(0.8f32)]
                }
                Some("minecraft:powder_snow") => {
                    if content.name(
                        world
                            .block(self.position.map(|v| v.floor() as i32))
                            .ok_or("unloaded_inside_block")?,
                    ) == Some("minecraft:powder_snow")
                    {
                        self.stuck_multiplier = [f64::from(0.9f32), 1.5, f64::from(0.9f32)];
                    }
                    if can_walk_powder(inventory, content)
                        && (self.horizontal_collision || input.has(4))
                    {
                        self.velocity[1] = 0.2;
                    }
                }
                Some("minecraft:bubble_column") if visit.affects_motion => {
                    let drag = content
                        .state_properties(state)
                        .and_then(|s| s.get("drag"))
                        .is_some_and(|v| v == "true");
                    let above = world.block([x, y + 1, z]).ok_or("unloaded_inside_block")?;
                    let surface = content.info(above)?.fluid.is_none()
                        && content
                            .collision_boxes(above)
                            .ok_or("unknown_collision_shape")?
                            .is_empty();
                    self.velocity[1] = if drag {
                        (self.velocity[1] - 0.03).max(if surface { -0.9 } else { -0.3 })
                    } else {
                        (self.velocity[1] + if surface { 0.1 } else { 0.06 }).min(if surface {
                            1.8
                        } else {
                            0.7
                        })
                    };
                    self.fall_distance = 0.;
                }
                Some("minecraft:honey_block") => {
                    let old_y = self.velocity[1] / f64::from(0.98f32) + 0.08;
                    let width = (self.bounds().max[0] - self.bounds().min[0]) / 2.;
                    if !self.grounded
                        && self.position[1] <= f64::from(y) + 0.9375 - 1e-7
                        && old_y < -0.08
                        && ((f64::from(x) + 0.5 - self.position[0]).abs() + 1e-7 > 0.4375 + width
                            || (f64::from(z) + 0.5 - self.position[2]).abs() + 1e-7
                                > 0.4375 + width)
                    {
                        if old_y < -0.13 {
                            let factor = -0.05 / old_y;
                            self.velocity[0] *= factor;
                            self.velocity[2] *= factor;
                        }
                        self.velocity[1] = (-0.05 - 0.08) * f64::from(0.98f32);
                        self.fall_distance = 0.;
                    }
                }
                _ => {}
            }
        }
        collision::apply_inside_effects(
            content,
            &self.inside_effects,
            &mut self.inside_block_state,
            can_freeze,
            false,
        )?;
        Ok(())
    }
    fn send_abilities(&self, out: &mut Vec<PacketCommand>) {
        let mut w = Writer::default();
        w.byte(if self.abilities.flying { 2 } else { 0 });
        out.push(PacketCommand::new(40, w));
    }
}
fn next_damage_breaks(stack: &Stack) -> bool {
    stack
        .components
        .get("minecraft:max_damage")
        .and_then(Value::as_i64)
        .is_some_and(|max| {
            stack
                .components
                .get("minecraft:damage")
                .and_then(Value::as_i64)
                .unwrap_or(0)
                >= max - 1
        })
}
fn can_walk_powder(inventory: &InventoryStore, content: &Content) -> bool {
    inventory
        .player
        .slots
        .get(8)
        .and_then(Option::as_ref)
        .is_some_and(|s| {
            content
                .item_names
                .get(&s.id)
                .is_some_and(|n| n == "minecraft:leather_boots")
        })
}
pub(super) fn modified_input(input: HeldInput, sneaking: f32, using: f32) -> [f32; 3] {
    let x = input.strafe();
    let z = input.forward();
    let length = (x * x + z * z).sqrt();
    if length < 1e-4 {
        return [0.; 3];
    }
    let reciprocal = 1. / length;
    let x = x * reciprocal * 0.98 * using * sneaking;
    let z = z * reciprocal * 0.98 * using * sneaking;
    let length = (x * x + z * z).sqrt();
    if length <= 0. {
        return [x, 0., z];
    }
    let reciprocal = 1. / length;
    let unit_x = x * reciprocal;
    let unit_z = z * reciprocal;
    let ratio = if unit_z.abs() > unit_x.abs() {
        unit_x.abs() / unit_z.abs()
    } else {
        unit_z.abs() / unit_x.abs()
    };
    let square_distance = (1. + ratio * ratio).sqrt();
    let speed = (length * square_distance).min(1.);
    [unit_x * speed, 0., unit_z * speed]
}

fn minor_collision(yaw: f32, movement: [f32; 3], moved: [f64; 3]) -> bool {
    let angle = f64::from(yaw * 0.017453292);
    let sin = f64::from(math::sin(angle));
    let cos = f64::from(math::cos(angle));
    let x = f64::from(movement[0]) * cos - f64::from(movement[2]) * sin;
    let z = f64::from(movement[2]) * cos + f64::from(movement[0]) * sin;
    let input_length = x * x + z * z;
    let moved_length = moved[0] * moved[0] + moved[2] * moved[2];
    if input_length < f64::from(1e-5f32) || moved_length < f64::from(1e-5f32) {
        return false;
    }
    ((x * moved[0] + z * moved[2]) / (input_length * moved_length).sqrt()).acos()
        < f64::from(0.13962634f32)
}

fn towards_zero(value: f64, amount: f64) -> f64 {
    if value.abs() <= amount {
        0.
    } else {
        value - value.signum() * amount
    }
}
pub(super) fn contains_fluid(world: &World, content: &Content, bounds: Bounds) -> Result<bool> {
    for y in bounds.min[1].floor() as i32..bounds.max[1].ceil() as i32 {
        for z in bounds.min[2].floor() as i32..bounds.max[2].ceil() as i32 {
            for x in bounds.min[0].floor() as i32..bounds.max[0].ceil() as i32 {
                let state = world
                    .block([x, y, z])
                    .ok_or("unloaded_fluid_neighborhood")?;
                if content.info(state)?.fluid.is_some() {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

pub(super) fn dimension_fast_lava(dimension: &Value) -> Result<bool> {
    let Some(value) = dimension
        .get("attributes")
        .and_then(|attributes| attributes.get("minecraft:gameplay/fast_lava"))
    else {
        return Ok(false);
    };
    let (operation, argument) = if let Some(operation) = value.get("modifier") {
        (
            operation.as_str().ok_or("invalid_fast_lava_modifier")?,
            value.get("argument").ok_or("missing_fast_lava_argument")?,
        )
    } else {
        ("override", value)
    };
    let argument = match argument {
        Value::Bool(value) => *value,
        Value::Number(value) if value.as_i64() == Some(0) => false,
        Value::Number(value) if value.as_i64() == Some(1) => true,
        _ => return Err("invalid_fast_lava_boolean".into()),
    };
    Ok(
        match operation.strip_prefix("minecraft:").unwrap_or(operation) {
            "override" | "or" | "xor" => argument,
            "and" => false,
            "nand" => true,
            "nor" | "xnor" => !argument,
            _ => return Err("unknown_fast_lava_modifier".into()),
        },
    )
}

#[derive(Default)]
pub(super) struct FluidContact {
    pub depth: [f64; 2],
    pub flow: [[f64; 3]; 2],
    pub cells: [u32; 2],
}

pub(super) fn fluid_contact(
    world: &World,
    content: &Content,
    bounds: Bounds,
    pushed: bool,
) -> Result<FluidContact> {
    fluid_contact_scan(world, content, bounds, bounds.deflated(0.001), pushed)
}

pub(super) fn fluid_contact_scan(
    world: &World,
    content: &Content,
    bounds: Bounds,
    scan: Bounds,
    pushed: bool,
) -> Result<FluidContact> {
    let mut contact = FluidContact::default();
    // EntityFluidInteraction scans X/Y/Z and attenuates by the running maximum depth.
    for x in scan.min[0].floor() as i32..scan.max[0].ceil() as i32 {
        for y in scan.min[1].floor() as i32..scan.max[1].ceil() as i32 {
            for z in scan.min[2].floor() as i32..scan.max[2].ceil() as i32 {
                let position = [x, y, z];
                let state = world.block(position).ok_or("unloaded_fluid_neighborhood")?;
                let Some(fluid) = content.info(state)?.fluid else {
                    continue;
                };
                let surface = f64::from(y) + Player::fluid_height(world, content, position, fluid)?;
                if surface < scan.min[1] {
                    continue;
                }
                let index = usize::from(fluid.kind == FluidKind::Lava);
                contact.depth[index] = contact.depth[index].max(surface - bounds.min[1]);
                if pushed {
                    let flow = fluid_flow(world, content, position, fluid)?;
                    let scale = if contact.depth[index] < 0.4 {
                        contact.depth[index]
                    } else {
                        1.
                    };
                    for axis in 0..3 {
                        contact.flow[index][axis] += flow[axis] * scale;
                    }
                    contact.cells[index] += 1;
                }
            }
        }
    }
    Ok(contact)
}

pub(super) fn apply_flow(
    velocity: &mut [f64; 3],
    mut flow: [f64; 3],
    cells: u32,
    speed: f64,
    normalize_flow: bool,
) {
    if cells == 0 || flow.iter().map(|v| v * v).sum::<f64>() < f64::from(1e-5f32) {
        return;
    }
    if normalize_flow {
        normalize(&mut flow);
    } else {
        for v in &mut flow {
            *v /= f64::from(cells);
        }
    }
    for v in &mut flow {
        *v *= speed;
    }
    let length = flow.iter().map(|v| v * v).sum::<f64>().sqrt();
    if velocity[0].abs() < 0.003 && velocity[2].abs() < 0.003 && length < 0.0045000000000000005 {
        for v in &mut flow {
            *v = *v / length * 0.0045000000000000005;
        }
    }
    for i in 0..3 {
        velocity[i] += flow[i];
    }
}
pub(super) fn fluid_flow(
    world: &World,
    content: &Content,
    pos: [i32; 3],
    fluid: FluidState,
) -> Result<[f64; 3]> {
    crate::protocol::models::fluids::flow(
        pos,
        fluid,
        |p| {
            let state = world.block(p).ok_or("unloaded_fluid_neighborhood")?;
            let info = content.info(state)?;
            Ok(crate::protocol::models::fluids::FluidSample {
                fluid: info.fluid,
                blocks_motion: info.solid,
            })
        },
        |p, direction| {
            let state = world.block(p).ok_or("unloaded_fluid_neighborhood")?;
            content.fluid_solid_face(state, fluid.kind, direction)
        },
    )
}
fn normalize(v: &mut [f64; 3]) {
    let length = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    if length < 1e-4 {
        *v = [0.; 3];
    } else {
        for x in v {
            *x /= length;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn square_input_preserves_slow_diagonal_motion_without_full_speed_normalization() {
        let diagonal = HeldInput {
            flags: 1 | 4,
            ..HeldInput::default()
        };
        let slow = modified_input(diagonal, 0.3, 0.2);
        assert!((slow[0] - 0.0588).abs() < 1e-7);
        assert!((slow[2] - 0.0588).abs() < 1e-7);
        let fast = modified_input(diagonal, 1., 1.);
        assert!((fast[0] * fast[0] + fast[2] * fast[2] - 1.).abs() < 1e-6);
        assert_eq!(
            modified_input(
                HeldInput {
                    flags: 1 | 2,
                    ..HeldInput::default()
                },
                1.,
                1.
            ),
            [0.; 3]
        );
    }
    #[test]
    fn shallow_wall_deflection_is_minor_but_head_on_collision_is_not() {
        assert!(minor_collision(0., [0., 0., 0.98], [0.01, 0., 0.1]));
        assert!(!minor_collision(0., [0., 0., 0.98], [0.02, 0., 0.1]));
        assert!(!minor_collision(0., [0., 0., 0.98], [0., 0., 0.]));
    }
    #[test]
    fn cancelled_and_tiny_fluid_currents_do_not_create_minimum_push() {
        let mut velocity = [0.; 3];
        apply_flow(&mut velocity, [0.001, 0., 0.], 2, 0.014, false);
        assert_eq!(velocity, [0.; 3]);
        apply_flow(&mut velocity, [0., 0., 0.], 4, 0.014, true);
        assert_eq!(velocity, [0.; 3]);
    }
    #[test]
    fn player_averages_current_while_mount_normalizes_accumulated_current() {
        let mut player = [0.01, 0., 0.];
        let mut mount = player;
        apply_flow(&mut player, [0.5, 0., 0.], 2, 0.014, false);
        apply_flow(&mut mount, [0.5, 0., 0.], 2, 0.014, true);
        assert_eq!(player, [0.0135, 0., 0.]);
        assert_eq!(mount, [0.024, 0., 0.]);
    }
    #[test]
    fn dimension_lava_rate_uses_received_environment_attribute_not_legacy_ultrawarm() {
        assert!(!dimension_fast_lava(&serde_json::json!({"ultrawarm":true})).unwrap());
        assert!(dimension_fast_lava(
            &serde_json::json!({"attributes":{"minecraft:gameplay/fast_lava":true}})
        )
        .unwrap());
        assert!(!dimension_fast_lava(&serde_json::json!({"attributes":{"minecraft:gameplay/fast_lava":{"modifier":"and","argument":true}}})).unwrap());
        assert!(dimension_fast_lava(&serde_json::json!({"attributes":{"minecraft:gameplay/fast_lava":{"modifier":"nand","argument":true}}})).unwrap());
        assert!(dimension_fast_lava(
            &serde_json::json!({"attributes":{"minecraft:gameplay/fast_lava":"true"}})
        )
        .is_err());
    }
}
