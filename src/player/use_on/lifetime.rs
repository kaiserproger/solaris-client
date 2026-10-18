use super::*;

pub(super) fn instrument<'a>(item: &'a Stack, content: &'a Content) -> Result<Option<&'a Value>> {
    let Some(value) = item.component("minecraft:instrument") else {
        return Ok(None);
    };
    if let Some(name) = value.as_str() {
        return content
            .registry_entries("minecraft:instrument")?
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| Some(value))
            .ok_or_else(|| format!("unknown instrument holder {name}"));
    }
    if value.is_object() {
        Ok(Some(value))
    } else {
        Err("invalid instrument holder".into())
    }
}

fn consume_ticks(consumable: &Value) -> i32 {
    (consumable
        .get("consume_seconds")
        .and_then(Value::as_f64)
        .unwrap_or(1.6) as f32
        * 20.) as i32
}

pub(super) fn sound_event(value: &Value) -> Result<&str> {
    value
        .as_str()
        .or_else(|| value.get("sound_id").and_then(Value::as_str))
        .ok_or_else(|| "invalid Source sound holder".into())
}

fn item_particle_rotation(vector: [f64; 3], pitch: f32, yaw: f32) -> [f64; 3] {
    let x = -pitch * (std::f32::consts::PI / 180.);
    let y = -yaw * (std::f32::consts::PI / 180.);
    let cx = f64::from(super::super::math::cos(f64::from(x)));
    let sx = f64::from(super::super::math::sin(f64::from(x)));
    let cy = f64::from(super::super::math::cos(f64::from(y)));
    let sy = f64::from(super::super::math::sin(f64::from(y)));
    let rotated = [
        vector[0],
        vector[1] * cx + vector[2] * sx,
        vector[2] * cx - vector[1] * sx,
    ];
    [
        rotated[0] * cy + rotated[2] * sy,
        rotated[1],
        rotated[2] * cy - rotated[0] * sy,
    ]
}

impl Player {
    pub(in crate::player) fn tick_using_item(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
        out: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        let Some(using) = self.using.as_ref() else {
            return Ok(());
        };
        let (hand, item_id, remaining) = (using.hand, using.item_id, using.remaining);
        let Some(item) = super::super::interaction::held(inventory, hand)
            .filter(|item| !item.empty() && item.id == item_id)
        else {
            // LivingEntity.updatingUsingItem stops a changed item, without invoking releaseUsing.
            self.using = None;
            return Ok(());
        };
        let identity = inventory.player_slot_identity(if hand == 1 {
            45
        } else {
            36 + u16::from(inventory.selected_slot)
        })?;
        self.using.as_mut().unwrap().slot_identity = identity;
        if let Some(consumable) = item.component("minecraft:consumable") {
            let duration = consume_ticks(consumable);
            if duration.wrapping_sub(remaining) > (duration as f32 * 0.21875) as i32
                && remaining % 4 == 0
            {
                self.emit_consumable_effects(item, consumable, 5, world)?;
            }
        }
        match short_class(content.item_class(item.id)?) {
            "BundleItem" if remaining == 200 || remaining < 190 && remaining % 2 == 0 => {
                if let Some((updated, _removed)) =
                    inventory.remove_one_bundle_for_use(item, content)?
                {
                    let pitch = 0.8 + world.next_float()? * 0.4;
                    self.play_use_local_sound(
                        world,
                        "minecraft:item.bundle.remove_one",
                        0.8,
                        pitch,
                    )?;
                    self.predict_held(hand, Some(updated), inventory, false)?;
                    // LivingEntity.drop on ClientLevel only swings; no item entity is spawned.
                    self.swing(0, out);
                    let pitch = 0.8 + world.next_float()? * 0.4;
                    self.play_use_level_sound(
                        world,
                        "minecraft:item.bundle.drop_contents",
                        crate::audio::SoundCategory::Players,
                        self.position.map(|coordinate| coordinate.floor() + 0.5),
                        0.8,
                        pitch,
                        false,
                    )?;
                }
            }
            "BrushItem" => {
                if remaining < 0 {
                    self.using = None;
                    return Ok(());
                }
                let Some(hit) = self.brush_hit(world, content, entities, tick)? else {
                    self.using = None;
                    return Ok(());
                };
                if 201i32.wrapping_sub(remaining) % 10 == 5 {
                    if content.block_should_spawn_terrain_particles(hit.state)?
                        && !content.block_render_invisible(hit.state)?
                    {
                        let view = self.look_direction();
                        let delta = match hit.face {
                            0 | 1 => [view[2], -view[0]],
                            2 => [1., -0.1],
                            3 => [-1., 0.1],
                            4 => [-0.1, -1.],
                            5 => [0.1, 1.],
                            _ => return Err("invalid brush hit face".into()),
                        };
                        let flip = if self.main_arm_right() != (hand == 1) {
                            1.
                        } else {
                            -1.
                        };
                        let count = 7 + world.next_int_bound(5)? as u8;
                        let mut samples = [UseParticleSample::default(); 16];
                        for sample in samples.iter_mut().take(usize::from(count)) {
                            sample.position = [
                                hit.point[0]
                                    - if hit.face == 4 {
                                        f64::from(1e-6f32)
                                    } else {
                                        0.
                                    },
                                hit.point[1],
                                hit.point[2]
                                    - if hit.face == 2 {
                                        f64::from(1e-6f32)
                                    } else {
                                        0.
                                    },
                            ];
                            sample.velocity = [
                                delta[0] * flip * 3. * world.next_double()?,
                                0.,
                                delta[1] * flip * 3. * world.next_double()?,
                            ];
                        }
                        self.predict_use(UsePrediction::Particles {
                            kind: UseParticle::BlockDust { state: hit.state },
                            samples,
                            count,
                        })?;
                    }
                    let sound = content
                        .block_brush_sound(hit.state)?
                        .unwrap_or("minecraft:item.brush.brushing.generic");
                    self.play_use_level_sound(
                        world,
                        sound,
                        crate::audio::SoundCategory::Blocks,
                        hit.position.map(|coordinate| f64::from(coordinate) + 0.5),
                        1.,
                        1.,
                        true,
                    )?;
                }
            }
            _ => {}
        }
        if let Some(using) = self.using.as_mut() {
            // ClientLevel never auto-finishes at zero; the server event finishes the same object.
            using.remaining = using.remaining.wrapping_sub(1);
        }
        Ok(())
    }

    pub(super) fn brush_hit(
        &self,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Option<BlockHit>> {
        let reach = self.attribute("minecraft:block_interaction_range");
        let block = self.raycast_blocks(world, content, reach, false, false)?;
        let limit = block.as_ref().map_or(reach, |hit| hit.distance);
        if entities
            .raycast_excluding(
                self.eye_position(),
                self.look_direction(),
                limit,
                self.entity_id,
                None,
                tick,
                0.,
            )?
            .is_some()
        {
            return Ok(None);
        }
        Ok(block)
    }

    fn emit_consumable_effects(
        &mut self,
        item: &Stack,
        consumable: &Value,
        count: u8,
        world: &World,
    ) -> Result<()> {
        let eat_volume = if self.random_bits(1) != 0 { 0.5 } else { 1. };
        let eat_pitch = 1. + 0.2 * (self.random_float() - self.random_float());
        let drink_pitch = 0.9 + self.random_float() * (1. - 0.9);
        let drinking = consumable.get("animation").and_then(Value::as_str) == Some("drink");
        if consumable
            .get("has_consume_particles")
            .and_then(Value::as_bool)
            .unwrap_or(true)
        {
            let eye = self.eye_position();
            let mut samples = [UseParticleSample::default(); 16];
            for sample in samples.iter_mut().take(usize::from(count)) {
                let velocity = [
                    (f64::from(self.random_float()) - 0.5) * 0.1,
                    f64::from(self.random_float()) * 0.1 + 0.1,
                    0.,
                ];
                let y = -f64::from(self.random_float()) * 0.6 - 0.3;
                let position = [(f64::from(self.random_float()) - 0.5) * 0.3, y, 0.6];
                sample.velocity = item_particle_rotation(velocity, self.pitch, self.yaw);
                sample.velocity[1] += 0.05;
                let position = item_particle_rotation(position, self.pitch, self.yaw);
                sample.position = std::array::from_fn(|axis| eye[axis] + position[axis]);
            }
            self.predict_use(UsePrediction::Particles {
                kind: UseParticle::Item {
                    stack: item.clone(),
                },
                samples,
                count,
            })?;
        }
        let sound = consumable
            .get("sound")
            .map(sound_event)
            .transpose()?
            .unwrap_or("minecraft:entity.generic.eat");
        self.play_use_local_sound(
            world,
            sound,
            if drinking { 0.5 } else { eat_volume },
            if drinking { drink_pitch } else { eat_pitch },
        )
    }

    pub(super) fn consume_item(&mut self, item: &Stack, world: &World) -> Result<Stack> {
        let consumable = item
            .component("minecraft:consumable")
            .ok_or("missing consumable component")?;
        self.emit_consumable_effects(item, consumable, 16, world)?;
        if let Some(food) = item.component("minecraft:food") {
            let sound = consumable
                .get("sound")
                .map(sound_event)
                .transpose()?
                .unwrap_or("minecraft:entity.generic.eat");
            let pitch = 1. + 0.4 * (self.random_float() - self.random_float());
            self.play_use_level_sound(
                world,
                sound,
                crate::audio::SoundCategory::Neutral,
                self.position,
                1.,
                pitch,
                false,
            )?;
            let nutrition = food
                .get("nutrition")
                .and_then(Value::as_i64)
                .ok_or("invalid food nutrition")? as i32;
            let saturation = food
                .get("saturation")
                .and_then(Value::as_f64)
                .ok_or("invalid food saturation")? as f32;
            if let Some(level) = self.food.as_mut() {
                *level = level.saturating_add(nutrition).min(20);
            }
            if let (Some(level), Some(value)) = (self.food, self.saturation.as_mut()) {
                *value = (*value + saturation).min(level as f32);
            }
            let pitch = 0.9 + self.random_float() * (1. - 0.9);
            self.play_use_level_sound(
                world,
                "minecraft:entity.player.burp",
                crate::audio::SoundCategory::Players,
                self.position,
                0.5,
                pitch,
                false,
            )?;
        }
        let mut result = item.clone();
        if !self.abilities.instabuild {
            result.count = (result.count - 1).max(0);
        }
        Ok(result)
    }

    pub(in crate::player) fn use_duration(&self, item: &Stack, content: &Content) -> Result<i32> {
        Ok(match short_class(content.item_class(item.id)?) {
            "BowItem" | "CrossbowItem" | "TridentItem" => 72000,
            "BrushItem" | "BundleItem" => 200,
            "SpyglassItem" => 1200,
            "EnderEyeItem" => 0,
            "InstrumentItem" => instrument(item, content)?
                .map(|value| {
                    value
                        .get("use_duration")
                        .and_then(Value::as_f64)
                        .map(|seconds| (seconds as f32 * 20.).floor() as i32)
                        .ok_or_else(|| "invalid instrument duration".to_owned())
                })
                .transpose()?
                .unwrap_or(0),
            _ => {
                if let Some(consumable) = item.component("minecraft:consumable") {
                    (consumable
                        .get("consume_seconds")
                        .and_then(Value::as_f64)
                        .unwrap_or(1.6) as f32
                        * 20.) as i32
                } else if item.component("minecraft:blocks_attacks").is_some()
                    || item.component("minecraft:kinetic_weapon").is_some()
                {
                    72000
                } else {
                    0
                }
            }
        })
    }

    /// Metadata changes hydrate/clear the local use stack, never synthesize a release packet.
    pub fn synchronize_using_flags(
        &mut self,
        flags: u8,
        inventory: &InventoryStore,
        content: &Content,
        tick: u64,
    ) -> Result<()> {
        if flags & 1 == 0 {
            self.using = None;
            return Ok(());
        }
        let hand = u8::from(flags & 2 != 0);
        if self.using.is_none() {
            self.start_using(
                super::super::interaction::held(inventory, hand),
                hand,
                content,
                inventory,
                tick,
            )?;
        }
        Ok(())
    }

    pub fn complete_using_item(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<()> {
        let Some(using) = self.using.take() else {
            return Ok(());
        };
        let Some(item) =
            super::super::interaction::held(inventory, using.hand).filter(|item| !item.empty())
        else {
            return Ok(());
        };
        let identity = inventory.player_slot_identity(if using.hand == 1 {
            45
        } else {
            36 + u16::from(inventory.selected_slot)
        })?;
        if identity != using.slot_identity {
            self.pending_use_release = Some(using);
            return self.apply_pending_use_release(world, content, inventory, entities, tick);
        }
        let result = if item.component("minecraft:consumable").is_some() {
            self.consume_item(item, world)?
        } else {
            if short_class(content.item_class(item.id)?) == "SpyglassItem" {
                self.play_use_local_sound(world, "minecraft:item.spyglass.stop_using", 1., 1.)?;
            }
            item.clone()
        };
        self.after_item_use(item, result, using.hand, inventory, content, tick)
    }

    pub(super) fn after_item_use(
        &mut self,
        before: &Stack,
        mut result: Stack,
        hand: u8,
        inventory: &InventoryStore,
        content: &Content,
        tick: u64,
    ) -> Result<()> {
        let mut replacement = false;
        if !self.abilities.instabuild && result.count < before.count {
            if let Some(template) = before.component("minecraft:use_remainder") {
                let retained = before
                    .patch
                    .added
                    .values()
                    .find(|component| component.name == "minecraft:use_remainder")
                    .and_then(|component| component.nested_stacks.first());
                let remainder = match retained {
                    Some(stack) => stack.clone(),
                    None => crate::inventory::template_stack(template, content)?,
                };
                if result.empty() {
                    result = remainder;
                    replacement = true;
                } else {
                    self.predict_use(UsePrediction::ExtraItem {
                        stack: remainder,
                        drop_remainder: true,
                    })?;
                }
            }
        }
        if let Some(cooldown) = before.component("minecraft:use_cooldown") {
            let seconds = cooldown
                .get("seconds")
                .and_then(Value::as_f64)
                .ok_or("invalid use cooldown duration")? as f32;
            let ticks = (seconds * 20.) as u64;
            let group = cooldown
                .get("cooldown_group")
                .and_then(Value::as_str)
                .unwrap_or(
                    content
                        .item_names
                        .get(&before.id)
                        .ok_or("unknown_held_item")?,
                );
            self.cooldowns
                .insert(group.to_owned(), tick.saturating_add(ticks));
        }
        if replacement || result != *before {
            self.predict_held(
                hand,
                (!result.empty()).then_some(result),
                inventory,
                replacement,
            )?;
        }
        Ok(())
    }

    pub(super) fn create_filled_result(
        &mut self,
        before: &Stack,
        filled: Stack,
        hand: u8,
        inventory: &InventoryStore,
        limit_creative_stack_size: bool,
    ) -> Result<()> {
        if limit_creative_stack_size && self.abilities.instabuild {
            if !inventory.source_contains_item(&filled)? {
                self.predict_use(UsePrediction::ExtraItem {
                    stack: filled,
                    drop_remainder: false,
                })?;
            }
            return Ok(());
        }
        let count = before.count - i32::from(!self.abilities.instabuild);
        if count <= 0 {
            self.predict_held(hand, Some(filled), inventory, true)?;
        } else {
            if count != before.count {
                let mut remaining = before.clone();
                remaining.count = count;
                self.predict_held(hand, Some(remaining), inventory, false)?;
            }
            self.predict_use(UsePrediction::ExtraItem {
                stack: filled,
                drop_remainder: true,
            })?;
        }
        Ok(())
    }
}
