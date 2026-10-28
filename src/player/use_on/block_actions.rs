use super::*;
use crate::audio::SoundCategory;

fn center(position: [i32; 3]) -> [f64; 3] {
    position.map(|value| f64::from(value) + 0.5)
}
fn set(content: &Content, state: u32, key: &str, value: &str) -> Result<u32> {
    content.with_state_property(state, key, value)
}

impl Player {
    pub(super) fn redstone_ore_particles(
        &mut self,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<()> {
        let mut samples = [UseParticleSample::default(); 16];
        let mut count = 0;
        for face in 0..6 {
            let neighbor = relative(position, face)?;
            if content.block_is_solid_render(self.use_block(world, neighbor)?)? {
                continue;
            }
            let delta: [i32; 3] = std::array::from_fn(|axis| neighbor[axis] - position[axis]);
            let normal_axis = face_axis(face);
            let mut particle = [0.; 3];
            for axis in 0..3 {
                particle[axis] = f64::from(position[axis])
                    + if axis == normal_axis {
                        0.5 + 0.5625 * f64::from(delta[axis])
                    } else {
                        f64::from(world.next_float()?)
                    };
            }
            samples[count] = UseParticleSample {
                position: particle,
                velocity: [0.; 3],
            };
            count += 1;
        }
        if count != 0 {
            self.predict_use(UsePrediction::Particles {
                kind: UseParticle::Dust {
                    color: 16711680,
                    scale: 1.,
                },
                samples,
                count: count as u8,
            })?;
        }
        Ok(())
    }

    fn eat_cake(
        &mut self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<()> {
        if self.food.unwrap_or(20) >= 20 && !self.abilities.invulnerable {
            return Ok(());
        }
        let food = (self.food.unwrap_or(20) + 2).min(20);
        self.food = Some(food);
        self.saturation = Some((self.saturation.unwrap_or(5.) + 0.4).min(food as f32));
        let bites = number(content, state, "bites")?;
        let next = if bites < 6 {
            set(content, state, "bites", &(bites + 1).to_string())?
        } else {
            content.fluid_legacy_state("minecraft:empty")?
        };
        self.predict_block(world, position, next, 3)?;
        Ok(())
    }

    pub(super) fn predict_block_interaction(
        &mut self,
        owner: &str,
        hit: &BlockHit,
        hand: u8,
        empty: bool,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<()> {
        let state = hit.state;
        let position = hit.position;
        let item = super::super::interaction::held(inventory, hand);
        match short_class(owner) {
            "LeverBlock" => {
                // Client use emits feedback; pull() runs only in ServerLevel.
                if !boolean(content, state, "powered")? {
                    let facing = facing_face(property(content, state, "facing")?)?;
                    let connected = match property(content, state, "face")? {
                        "floor" => 1,
                        "ceiling" => 0,
                        "wall" => facing,
                        _ => return Err("invalid lever attachment".into()),
                    };
                    let backwards = relative(position, facing ^ 1)?;
                    let support = relative(position, connected ^ 1)?;
                    let mut samples = [UseParticleSample::default(); 16];
                    samples[0].position = std::array::from_fn(|axis| {
                        f64::from(position[axis])
                            + 0.5
                            + 0.1 * f64::from(backwards[axis] - position[axis])
                            + 0.2 * f64::from(support[axis] - position[axis])
                    });
                    self.predict_use(UsePrediction::Particles {
                        kind: UseParticle::Dust {
                            color: 16711680,
                            scale: 1.,
                        },
                        samples,
                        count: 1,
                    })?;
                }
            }
            "ButtonBlock" => {
                if !boolean(content, state, "powered")? {
                    self.predict_block(
                        world,
                        position,
                        set(content, state, "powered", "true")?,
                        3,
                    )?;
                    self.play_use_level_sound(
                        world,
                        content.block_use_sound(state, "on")?,
                        SoundCategory::Blocks,
                        center(position),
                        0.3,
                        0.6,
                        true,
                    )?;
                }
            }
            "DoorBlock" | "TrapDoorBlock" | "FenceGateBlock" => {
                let open = !boolean(content, state, "open")?;
                let mut next = set(content, state, "open", if open { "true" } else { "false" })?;
                if short_class(owner) == "FenceGateBlock" && open {
                    let facing = facing_face(property(content, state, "facing")?)?;
                    let player_facing = [3, 4, 2, 5]
                        [((f64::from(self.yaw) / 90. + 0.5).floor() as i32 & 3) as usize];
                    if facing == (player_facing ^ 1) {
                        next = set(
                            content,
                            next,
                            "facing",
                            ["down", "up", "north", "south", "west", "east"]
                                [player_facing as usize],
                        )?;
                    }
                }
                let flags = if short_class(owner) == "TrapDoorBlock" {
                    2
                } else {
                    10
                };
                self.predict_block(world, position, next, flags)?;
                let pitch = world.next_float()? * 0.1 + 0.9;
                self.play_use_level_sound(
                    world,
                    content.block_use_sound(state, if open { "open" } else { "close" })?,
                    SoundCategory::Blocks,
                    center(position),
                    1.,
                    pitch,
                    true,
                )?;
            }
            "RepeaterBlock" => {
                self.predict_block(
                    world,
                    position,
                    content.cycle_state_property(state, "delay")?,
                    3,
                )?;
            }
            "ComparatorBlock" => {
                let next = content.cycle_state_property(state, "mode")?;
                let pitch = if property(content, next, "mode")? == "subtract" {
                    0.55
                } else {
                    0.5
                };
                self.play_use_level_sound(
                    world,
                    "minecraft:block.comparator.click",
                    SoundCategory::Blocks,
                    center(position),
                    0.3,
                    pitch,
                    true,
                )?;
                self.predict_block(world, position, next, 2)?;
                self.refresh_comparator(position, next, world, content, entities, tick)?;
            }
            "RedstoneWireBlock" => {
                self.predict_block(
                    world,
                    position,
                    self.toggled_wire_state(position, state, world, content)?,
                    3,
                )?;
            }
            "CakeBlock" if !empty => {
                let candle = content
                    .item_block(item.ok_or("missing cake candle")?.id)?
                    .ok_or("cake candle has no source block")?;
                let next = content
                    .block_candle_cake_state(candle)?
                    .ok_or("missing candle cake source mapping")?;
                self.consume_held(hand, inventory, 1)?;
                self.play_use_level_sound(
                    world,
                    "minecraft:block.cake.add_candle",
                    SoundCategory::Blocks,
                    center(position),
                    1.,
                    1.,
                    false,
                )?;
                self.predict_block(world, position, next, 3)?;
            }
            "CakeBlock" => self.eat_cake(state, position, world, content)?,
            "CandleCakeBlock" if empty => {
                self.eat_cake(
                    content.resolve_block_state("minecraft:cake", &BTreeMap::new())?,
                    position,
                    world,
                    content,
                )?;
            }
            "CandleBlock" | "CandleCakeBlock" => {
                self.candle_extinguish(state, position, true, world, content)?;
            }
            "FlowerPotBlock" if !empty => {
                let plant = content
                    .item_block(item.ok_or("missing potted plant")?.id)?
                    .ok_or("potted plant has no source block")?;
                let Some(next) = content.block_potted_state(plant)? else {
                    return Ok(());
                };
                // A filled pot consumes the interaction without replacing either plant.
                if content.name(state) == Some("minecraft:flower_pot") {
                    self.predict_block(world, position, next, 3)?;
                    self.consume_held(hand, inventory, 1)?;
                }
            }
            "FlowerPotBlock" => {
                if let Some(plant) = content.block_potted_content(state)? {
                    let plant_id = content.block_as_item(plant)?;
                    let plant_name = content
                        .item_names
                        .get(&plant_id)
                        .ok_or("unknown potted plant item")?;
                    let plant_item = crate::inventory::template_stack(
                        &serde_json::json!({"id":plant_name}),
                        content,
                    )?;
                    if item.is_none_or(Stack::empty) {
                        self.predict_held(hand, Some(plant_item), inventory, true)?;
                    } else {
                        self.predict_use(UsePrediction::ExtraItem {
                            stack: plant_item,
                            drop_remainder: true,
                        })?;
                    }
                    self.predict_block(
                        world,
                        position,
                        content.resolve_block_state("minecraft:flower_pot", &BTreeMap::new())?,
                        3,
                    )?;
                }
            }
            "ComposterBlock" if empty => {
                self.predict_block(world, position, set(content, state, "level", "0")?, 3)?;
                self.play_use_level_sound(
                    world,
                    "minecraft:block.composter.empty",
                    SoundCategory::Blocks,
                    center(position),
                    1.,
                    1.,
                    false,
                )?;
            }
            "RespawnAnchorBlock" if !empty => {
                let next = set(
                    content,
                    state,
                    "charges",
                    &(number(content, state, "charges")? + 1).to_string(),
                )?;
                self.consume_held(hand, inventory, 1)?;
                self.predict_block(world, position, next, 3)?;
                self.play_use_level_sound(
                    world,
                    "minecraft:block.respawn_anchor.charge",
                    SoundCategory::Blocks,
                    center(position),
                    1.,
                    1.,
                    false,
                )?;
            }
            "BeehiveBlock" => {
                let mut bottle = item.ok_or("missing hive bottle")?.clone();
                bottle.count -= 1;
                let honey = crate::inventory::template_stack(
                    &serde_json::json!({"id":"minecraft:honey_bottle"}),
                    content,
                )?;
                self.predict_held(
                    hand,
                    if bottle.empty() {
                        Some(honey.clone())
                    } else {
                        Some(bottle)
                    },
                    inventory,
                    item.is_some_and(|item| item.count == 1),
                )?;
                if item.is_some_and(|item| item.count > 1) {
                    self.predict_use(UsePrediction::ExtraItem {
                        stack: honey,
                        drop_remainder: true,
                    })?;
                }
                self.play_use_level_sound(
                    world,
                    "minecraft:item.bottle.fill",
                    SoundCategory::Blocks,
                    self.position,
                    1.,
                    1.,
                    true,
                )?;
                self.predict_block(world, position, set(content, state, "honey_level", "0")?, 3)?;
            }
            "CopperGolemStatueBlock" | "WeatheringCopperGolemStatueBlock" => {
                if item_tag(item, content, "minecraft:axes")? {
                    // removeStatue constructs a client-only temporary entity; ClientLevel
                    // refuses addFreshEntity, but removeBlock is still a local mutation.
                    self.play_use_level_sound(
                        world,
                        "minecraft:entity.copper_golem.spawn",
                        SoundCategory::Neutral,
                        center(position),
                        1.,
                        1.,
                        false,
                    )?;
                    self.predict_block(
                        world,
                        position,
                        content.fluid_legacy_state("minecraft:empty")?,
                        3,
                    )?;
                } else {
                    self.play_use_level_sound(
                        world,
                        "minecraft:entity.copper_golem.become_statue",
                        SoundCategory::Blocks,
                        center(position),
                        1.,
                        1.,
                        false,
                    )?;
                    self.predict_block(
                        world,
                        position,
                        content.cycle_state_property(state, "copper_golem_pose")?,
                        3,
                    )?;
                }
            }
            "DecoratedPotBlock" if empty => {
                // wobble() itself is ServerLevel-only, including the block event.
                self.play_use_level_sound(
                    world,
                    "minecraft:block.decorated_pot.insert_fail",
                    SoundCategory::Blocks,
                    center(position),
                    1.,
                    1.,
                    false,
                )?;
            }
            // These exact Source declaring methods dispatch their effects only on
            // ServerLevel. A client consumes their result, without inventing data.
            "BlockBehaviour"
            | "MovingPistonBlock"
            | "FenceBlock"
            | "DispenserBlock"
            | "NoteBlock"
            | "TntBlock"
            | "ChiseledBookShelfBlock"
            | "ShelfBlock"
            | "SignBlock"
            | "CeilingHangingSignBlock"
            | "WallHangingSignBlock"
            | "RedStoneOreBlock"
            | "JukeboxBlock"
            | "PumpkinBlock"
            | "AbstractCauldronBlock"
            | "LecternBlock"
            | "CampfireBlock"
            | "SweetBerryBushBlock"
            | "ComposterBlock"
            | "RespawnAnchorBlock"
            | "DecoratedPotBlock"
            | "VaultBlock"
            | "ChestBlock"
            | "CraftingTableBlock"
            | "AbstractFurnaceBlock"
            | "EnchantingTableBlock"
            | "BrewingStandBlock"
            | "DragonEggBlock"
            | "EnderChestBlock"
            | "BeaconBlock"
            | "AnvilBlock"
            | "HopperBlock"
            | "ShulkerBoxBlock"
            | "LoomBlock"
            | "BarrelBlock"
            | "CartographyTableBlock"
            | "GrindstoneBlock"
            | "SmithingTableBlock"
            | "StonecutterBlock"
            | "CrafterBlock"
            | "AbstractBedBlock"
            | "LightBlock"
            | "DaylightDetectorBlock"
            | "CaveVinesBlock"
            | "CaveVinesPlantBlock"
            | "CommandBlock"
            | "StructureBlock"
            | "JigsawBlock"
            | "TestBlock"
            | "TestInstanceBlock" => {}
            owner => return Err(format!("unknown verified block action owner {owner}")),
        }
        Ok(())
    }
}
