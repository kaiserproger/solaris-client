use super::*;

impl Player {
    pub(super) fn item_raycast(
        &self,
        world: &World,
        content: &Content,
        fluids: bool,
        source_only: bool,
    ) -> Result<Option<BlockHit>> {
        self.raycast_blocks(
            world,
            content,
            self.attribute("minecraft:block_interaction_range"),
            fluids,
            source_only,
        )
    }

    pub(super) fn bucket_result(
        &mut self,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let item =
            super::super::interaction::held(inventory, hand).ok_or("missing bucket stack")?;
        let fluid = content
            .item_bucket_fluid(item.id)?
            .ok_or("missing Source bucket fluid")?;
        let empty = fluid == "minecraft:empty";
        let Some(hit) = self.item_raycast(world, content, empty, empty)? else {
            return Ok(Outcome::Pass);
        };
        let offset = relative(hit.position, hit.face)?;
        if !self.abilities.may_build
            && !super::super::adventure::matches(
                Some(item),
                "minecraft:can_place_on",
                &hit,
                world,
                entities,
                content,
            )?
        {
            return Ok(Outcome::Fail);
        }
        if empty {
            let Some(owner) = content.block_bucket_owners(hit.state)?.0 else {
                return Ok(Outcome::Fail);
            };
            let (filled_name, sound) = match short_class(owner) {
                "LiquidBlock" => {
                    if number(content, hit.state, "level")? != 0 {
                        return Ok(Outcome::Fail);
                    }
                    let kind = content
                        .info(hit.state)?
                        .fluid
                        .ok_or("missing pickup liquid")?
                        .kind;
                    self.predict_block(
                        world,
                        hit.position,
                        content.fluid_legacy_state("minecraft:empty")?,
                        11,
                    )?;
                    match kind {
                        FluidKind::Water => {
                            ("minecraft:water_bucket", "minecraft:item.bucket.fill")
                        }
                        FluidKind::Lava => {
                            ("minecraft:lava_bucket", "minecraft:item.bucket.fill_lava")
                        }
                    }
                }
                "BubbleColumnBlock" => {
                    self.predict_block(
                        world,
                        hit.position,
                        content.fluid_legacy_state("minecraft:empty")?,
                        11,
                    )?;
                    ("minecraft:water_bucket", "minecraft:item.bucket.fill")
                }
                "PowderSnowBlock" => {
                    self.predict_block(
                        world,
                        hit.position,
                        content.fluid_legacy_state("minecraft:empty")?,
                        11,
                    )?;
                    (
                        "minecraft:powder_snow_bucket",
                        "minecraft:item.bucket.fill_powder_snow",
                    )
                }
                "SimpleWaterloggedBlock" | "BarrierBlock" => {
                    if short_class(owner) == "BarrierBlock" && self.game_mode != 1
                        || !boolean(content, hit.state, "waterlogged")?
                    {
                        return Ok(Outcome::Fail);
                    }
                    let dry = content.with_state_property(hit.state, "waterlogged", "false")?;
                    self.predict_block(world, hit.position, dry, 3)?;
                    // Source tests the original state, against the level after removing its water.
                    if !self.can_survive(hit.state, hit.position, world, content, entities, tick)? {
                        self.predict_block(
                            world,
                            hit.position,
                            content.fluid_legacy_state("minecraft:empty")?,
                            3,
                        )?;
                    }
                    ("minecraft:water_bucket", "minecraft:item.bucket.fill")
                }
                owner => return Err(format!("unknown Source bucket pickup owner {owner}")),
            };
            self.play_use_local_sound(world, sound, 1., 1.)?;
            let filled =
                crate::inventory::template_stack(&serde_json::json!({"id":filled_name}), content)?;
            self.create_filled_result(item, filled, hand, inventory, true)?;
            return Ok(Outcome::CLIENT);
        }
        let kind = match fluid {
            "minecraft:water" => FluidKind::Water,
            "minecraft:lava" => FluidKind::Lava,
            _ => return Err(format!("unexpected Source bucket fluid {fluid}")),
        };
        let container = content.block_bucket_owners(hit.state)?.1.is_some();
        let position = if container && kind == FluidKind::Water {
            hit.position
        } else {
            offset
        };
        if !self.empty_bucket_contents(position, Some(&hit), kind, item.id, world, content)? {
            return Ok(Outcome::Fail);
        }
        let result = if self.abilities.instabuild {
            item.clone()
        } else {
            crate::inventory::template_stack(
                &serde_json::json!({"id":"minecraft:bucket"}),
                content,
            )?
        };
        self.create_filled_result(item, result, hand, inventory, true)?;
        Ok(Outcome::CLIENT)
    }

    fn empty_bucket_contents(
        &mut self,
        position: [i32; 3],
        hit: Option<&BlockHit>,
        fluid: FluidKind,
        item_id: u32,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let state = self.use_block(world, position)?;
        let (_, can_owner, place_owner) = content.block_bucket_owners(state)?;
        let may_replace = content.block_can_replace_fluid(state, fluid)?;
        let can_place = if may_replace {
            true
        } else {
            match can_owner.map(short_class) {
                None => false,
                Some("SimpleWaterloggedBlock") => fluid == FluidKind::Water,
                Some("BarrierBlock") => self.game_mode == 1 && fluid == FluidKind::Water,
                Some("SlabBlock") => {
                    property(content, state, "type")? != "double" && fluid == FluidKind::Water
                }
                Some("KelpBlock" | "KelpPlantBlock" | "SeagrassBlock" | "TallSeagrassBlock") => {
                    false
                }
                Some(owner) => return Err(format!("unknown Source canPlaceLiquid owner {owner}")),
            }
        };
        if !content.block_is_air(state)?
            && !(can_place && (self.last_sent_input & 32 == 0 || hit.is_none()))
        {
            return match hit {
                Some(hit) => self.empty_bucket_contents(
                    relative(hit.position, hit.face)?,
                    None,
                    fluid,
                    item_id,
                    world,
                    content,
                ),
                None => Ok(false),
            };
        }
        if fluid == FluidKind::Water && world.water_evaporates(position, content)? {
            let pitch = 2.6 + (world.next_float()? - world.next_float()?) * 0.8;
            self.play_use_level_sound(
                world,
                "minecraft:block.fire.extinguish",
                crate::audio::SoundCategory::Blocks,
                position.map(|coordinate| f64::from(coordinate) + 0.5),
                0.5,
                pitch,
                true,
            )?;
            let mut samples = [UseParticleSample::default(); 16];
            for sample in samples.iter_mut().take(8) {
                sample.position = [
                    f64::from(position[0]) + f64::from(world.next_float()?),
                    f64::from(position[1]) + f64::from(world.next_float()?),
                    f64::from(position[2]) + f64::from(world.next_float()?),
                ];
            }
            self.predict_use(UsePrediction::Particles {
                kind: UseParticle::LargeSmoke,
                samples,
                count: 8,
            })?;
            return Ok(true);
        }
        if fluid == FluidKind::Water && can_owner.is_some() {
            // The bucket ignores placeLiquid's boolean. SimpleWaterloggedBlock and
            // DriedGhastBlock perform their state writes only on ServerLevel.
            match place_owner
                .map(short_class)
                .ok_or("missing Source placeLiquid owner")?
            {
                "SimpleWaterloggedBlock"
                | "DriedGhastBlock"
                | "KelpBlock"
                | "KelpPlantBlock"
                | "SeagrassBlock"
                | "TallSeagrassBlock" => {}
                "SlabBlock" => {}
                "CampfireBlock" if !boolean(content, state, "waterlogged")? => {
                    if boolean(content, state, "lit")? {
                        self.campfire_dowse(state, position, world, content)?;
                    }
                    let state = content.with_state_property(
                        content.with_state_property(state, "waterlogged", "true")?,
                        "lit",
                        "false",
                    )?;
                    self.predict_block(world, position, state, 3)?;
                }
                "CandleBlock" if !boolean(content, state, "waterlogged")? => {
                    let wet = content.with_state_property(state, "waterlogged", "true")?;
                    if boolean(content, state, "lit")? {
                        self.candle_extinguish(wet, position, false, world, content)?;
                    } else {
                        self.predict_block(world, position, wet, 3)?;
                    }
                }
                "CampfireBlock" | "CandleBlock" => {}
                owner => return Err(format!("unknown Source placeLiquid owner {owner}")),
            }
        } else {
            let name = if fluid == FluidKind::Water {
                "minecraft:water"
            } else {
                "minecraft:lava"
            };
            if !self.predict_block(world, position, content.fluid_legacy_state(name)?, 11)?
                && !content
                    .info(state)?
                    .fluid
                    .is_some_and(|fluid| fluid.amount == 8 && !fluid.falling)
            {
                return Ok(false);
            }
        }
        let event = content
            .item_bucket_empty_sound(item_id)?
            .ok_or("missing Source bucket empty sound")?;
        self.play_use_level_sound(
            world,
            event,
            crate::audio::SoundCategory::Blocks,
            position.map(|coordinate| f64::from(coordinate) + 0.5),
            1.,
            1.,
            true,
        )?;
        Ok(true)
    }

    pub(super) fn bottle_result(
        &mut self,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        _entities: &EntityStore,
        _tick: u64,
    ) -> Result<Outcome> {
        // AreaEffectCloud.owner is not synchronized or set on the remote client.
        // Its Source constructor leaves it null, so dragon clouds fail getOwner().
        let Some(hit) = self.item_raycast(world, content, true, true)? else {
            return Ok(Outcome::Pass);
        };
        if !content
            .info(hit.state)?
            .fluid
            .is_some_and(|fluid| fluid.kind == FluidKind::Water)
        {
            return Ok(Outcome::Pass);
        }
        self.play_use_level_sound(
            world,
            "minecraft:item.bottle.fill",
            crate::audio::SoundCategory::Neutral,
            self.position,
            1.,
            1.,
            true,
        )?;
        let item =
            super::super::interaction::held(inventory, hand).ok_or("missing bottle stack")?;
        let filled = crate::inventory::template_stack(
            &serde_json::json!({"id":"minecraft:potion","components":{"minecraft:potion_contents":{"potion":"minecraft:water"}}}),
            content,
        )?;
        self.create_filled_result(item, filled, hand, inventory, true)?;
        Ok(Outcome::CLIENT)
    }

    pub(super) fn boat_result(
        &mut self,
        _hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let Some(hit) = self.item_raycast(world, content, true, false)? else {
            return Ok(Outcome::Pass);
        };
        if entities.boat_item_eye_obstructed(
            self.bounds(),
            self.look_direction(),
            self.eye_position(),
            self.entity_id,
            tick,
        )? {
            return Ok(Outcome::Pass);
        }
        let item = super::super::interaction::held(inventory, _hand).ok_or("missing boat stack")?;
        let entity_type = content
            .item_spawn_type(item.id)?
            .ok_or("missing Source boat entity type")?;
        let schema = entities
            .models_ref()
            .ok_or("entity models not activated")?
            .schemas
            .get(entity_type)
            .ok_or("missing Source boat schema")?;
        let width = f64::from(schema.dimensions.width) * 0.5;
        let bounds = super::super::collision::Bounds::new([
            hit.point[0] - width,
            hit.point[1],
            hit.point[2] - width,
            hit.point[0] + width,
            hit.point[1] + f64::from(schema.dimensions.height),
            hit.point[2] + width,
        ]);
        super::super::collision::gather_with_context(
            world,
            content,
            entities,
            tick,
            bounds,
            Some(super::super::collision::Context {
                bottom: hit.point[1],
                descending: false,
                fall_distance: 0.,
                walks_powder_snow: false,
            }),
            &mut self.colliders,
        )?;
        if !self.colliders.is_empty()
            || entities.boat_item_blocked(
                bounds,
                self.bounds(),
                self.entity_id,
                self.game_mode == 3,
                tick,
                world,
                content,
            )?
            || world.border_collides(hit.point, bounds)?
        {
            return Ok(Outcome::Fail);
        }
        // Boat creation, stack consumption and spawn game event are ServerLevel-only.
        Ok(Outcome::CLIENT)
    }

    pub(super) fn spawn_egg_air_result(
        &mut self,
        _hand: u8,
        world: &World,
        content: &Content,
        _inventory: &InventoryStore,
        _entities: &EntityStore,
        _tick: u64,
    ) -> Result<Outcome> {
        // SpawnEggItem.use's permissions, fluid-block and spawn checks belong to ServerLevel.
        Ok(
            if self.item_raycast(world, content, true, true)?.is_some() {
                Outcome::CLIENT
            } else {
                Outcome::Pass
            },
        )
    }

    pub(super) fn water_block_result(
        &mut self,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let reach = self.attribute("minecraft:block_interaction_range");
        let (position, face, point, distance, inside) =
            match self.item_raycast(world, content, true, true)? {
                Some(hit) => (hit.position, hit.face, hit.point, hit.distance, hit.inside),
                None => {
                    // BlockGetter.clip's MISS retains endpoint, its floor and the opposite ray normal.
                    let from = self.eye_position();
                    let direction = self.look_direction();
                    let point = std::array::from_fn(|i| from[i] + direction[i] * reach);
                    let delta: [f32; 3] = std::array::from_fn(|i| (from[i] - point[i]) as f32);
                    let mut face = 2;
                    let mut highest = f32::from_bits(1);
                    for (index, normal) in DIRECTIONS.iter().enumerate() {
                        let dot = delta[0] * normal[0] as f32
                            + delta[1] * normal[1] as f32
                            + delta[2] * normal[2] as f32;
                        if dot > highest {
                            highest = dot;
                            face = index as i32;
                        }
                    }
                    (
                        point.map(|value| value.floor() as i32),
                        face,
                        point,
                        reach,
                        false,
                    )
                }
            };
        let position = relative(position, 1)?;
        let hit = BlockHit {
            position,
            state: world
                .block(position)
                .ok_or("unloaded_water_block_placement")?,
            face,
            point,
            distance,
            inside,
        };
        self.place_item(&hit, hand, world, content, inventory, entities, tick)
    }
}
