use super::*;

fn shifted(position: [i32; 3], direction: u8) -> [i32; 3] {
    let delta = match direction {
        0 => [0, -1, 0],
        1 => [0, 1, 0],
        2 => [0, 0, -1],
        3 => [0, 0, 1],
        4 => [-1, 0, 0],
        5 => [1, 0, 0],
        _ => unreachable!("source direction"),
    };
    std::array::from_fn(|axis| position[axis] + delta[axis])
}
fn opposite(direction: u8) -> u8 {
    direction ^ 1
}
fn direction(value: &str) -> Result<u8> {
    match value {
        "down" => Ok(0),
        "up" => Ok(1),
        "north" => Ok(2),
        "south" => Ok(3),
        "west" => Ok(4),
        "east" => Ok(5),
        _ => Err(format!("invalid source direction {value}")),
    }
}
fn direction_name(value: u8) -> &'static str {
    ["down", "up", "north", "south", "west", "east"][usize::from(value)]
}

impl Player {
    pub(super) fn neighbor_signal(
        &self,
        position: [i32; 3],
        world: &World,
        content: &Content,
        entities: &EntityStore,
    ) -> Result<bool> {
        for face in 0..6 {
            let neighbor = shifted(position, face);
            let state = self.use_block(world, neighbor)?;
            let mut signal =
                self.block_signal(neighbor, state, face, false, world, content, entities)?;
            if content.block_redstone_conductor(state)? {
                for direct_face in 0..6 {
                    let direct_neighbor = shifted(neighbor, direct_face);
                    let direct_state = self.use_block(world, direct_neighbor)?;
                    signal = signal.max(self.block_signal(
                        direct_neighbor,
                        direct_state,
                        direct_face,
                        true,
                        world,
                        content,
                        entities,
                    )?);
                    if signal >= 15 {
                        break;
                    }
                }
            }
            if signal > 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(super) fn block_signal(
        &self,
        position: [i32; 3],
        state: u32,
        face: u8,
        direct: bool,
        world: &World,
        content: &Content,
        entities: &EntityStore,
    ) -> Result<i32> {
        let owners = content.block_signal_owners(state)?;
        let owner = short_class(if direct { owners.1 } else { owners.0 });
        let facing = || direction(property(content, state, "facing")?);
        let powered = || boolean(content, state, "powered");
        let connected_direction = || match property(content, state, "face")? {
            "floor" => Ok(1),
            "ceiling" => Ok(0),
            "wall" => facing(),
            _ => Err("invalid source attached face".into()),
        };
        Ok(match owner {
            "BlockBehaviour" => 0,
            "PoweredBlock" => 15,
            "BasePressurePlateBlock" => {
                if !direct || face == 1 {
                    if content
                        .state_properties(state)
                        .ok_or("missing pressure plate state")?
                        .contains_key("power")
                    {
                        number(content, state, "power")?
                    } else if powered()? {
                        15
                    } else {
                        0
                    }
                } else {
                    0
                }
            }
            "ButtonBlock" | "LeverBlock" => {
                if powered()? && (!direct || face == connected_direction()?) {
                    15
                } else {
                    0
                }
            }
            "DaylightDetectorBlock" | "TargetBlock" => number(content, state, "power")?,
            "CalibratedSculkSensorBlock" => {
                if face != facing()? {
                    number(content, state, "power")?
                } else {
                    0
                }
            }
            "SculkSensorBlock" => {
                if !direct || face == 1 {
                    if short_class(owners.0) == "CalibratedSculkSensorBlock" && face == facing()? {
                        0
                    } else {
                        number(content, state, "power")?
                    }
                } else {
                    0
                }
            }
            "DetectorRailBlock" | "LecternBlock" => {
                if powered()? && (!direct || face == 1) {
                    15
                } else {
                    0
                }
            }
            "LightningRodBlock" => {
                if powered()? && (!direct || face == facing()?) {
                    15
                } else {
                    0
                }
            }
            "ObserverBlock" => {
                if powered()? && face == facing()? {
                    15
                } else {
                    0
                }
            }
            "DiodeBlock" => {
                if powered()? && face == facing()? {
                    if content.name(state) == Some("minecraft:comparator") {
                        entities.comparator_output(position).unwrap_or(0)
                    } else {
                        15
                    }
                } else {
                    0
                }
            }
            "RedstoneTorchBlock" => {
                let weak_face = if direct { 0 } else { face };
                let enabled = boolean(content, state, "lit")?
                    && if short_class(owners.0) == "RedstoneWallTorchBlock" {
                        weak_face != facing()?
                    } else {
                        weak_face != 1
                    };
                if enabled && (!direct || face == 0) {
                    15
                } else {
                    0
                }
            }
            "RedstoneWallTorchBlock" => {
                if boolean(content, state, "lit")? && face != facing()? {
                    15
                } else {
                    0
                }
            }
            "TripWireHookBlock" => {
                if powered()? && (!direct || face == facing()?) {
                    15
                } else {
                    0
                }
            }
            "JukeboxBlock" => {
                if entities.jukebox_playing(position).unwrap_or(false) {
                    15
                } else {
                    0
                }
            }
            "TrappedChestBlock" => {
                if !direct || face == 1 {
                    entities
                        .block_open_count(position)
                        .unwrap_or(0)
                        .clamp(0, 15)
                } else {
                    0
                }
            }
            "TestBlock" => {
                if property(content, state, "mode")? == "start"
                    && entities.test_block_powered(position).unwrap_or(false)
                {
                    15
                } else {
                    0
                }
            }
            "RedstoneWireBlock" => {
                let power = number(content, state, "power")?;
                if face == 0 || power == 0 {
                    0
                } else if face == 1
                    || self.wire_connections(position, state, world, content)?
                        [usize::from(opposite(face) - 2)]
                {
                    power
                } else {
                    0
                }
            }
            owner => return Err(format!("unimplemented verified signal owner {owner}")),
        })
    }

    fn wire_connections(
        &self,
        position: [i32; 3],
        state: u32,
        world: &World,
        content: &Content,
    ) -> Result<[bool; 4]> {
        let connected = self.wire_connection_state(position, state, world, content)?;
        let mut result = [false; 4];
        for face in 2..6 {
            result[usize::from(face - 2)] =
                property(content, connected, direction_name(face))? != "none";
        }
        Ok(result)
    }

    pub(super) fn wire_connection_state(
        &self,
        position: [i32; 3],
        state: u32,
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let was_dot = (2..6).try_fold(true, |dot, face| -> Result<bool> {
            Ok(dot && property(content, state, direction_name(face))? == "none")
        })?;
        let can_connect_up =
            !content.block_redstone_conductor(self.use_block(world, shifted(position, 1))?)?;
        let mut connections = ["none"; 4];
        for face in 2..6 {
            let neighbor = shifted(position, face);
            let relative_state = self.use_block(world, neighbor)?;
            let placeable_above = short_class(content.block_use_owners(relative_state)?.1)
                == "TrapDoorBlock"
                || content.info(relative_state)?.sturdy_faces[1]
                || content.name(relative_state) == Some("minecraft:hopper");
            let up_connected = can_connect_up
                && placeable_above
                && self.wire_connects_to(
                    self.use_block(world, shifted(neighbor, 1))?,
                    None,
                    content,
                )?;
            connections[usize::from(face - 2)] = if up_connected {
                if content.info(relative_state)?.sturdy_faces[usize::from(opposite(face))] {
                    "up"
                } else {
                    "side"
                }
            } else if self.wire_connects_to(relative_state, Some(face), content)?
                || !content.block_redstone_conductor(relative_state)?
                    && self.wire_connects_to(
                        self.use_block(world, shifted(neighbor, 0))?,
                        None,
                        content,
                    )?
            {
                "side"
            } else {
                "none"
            };
        }
        if !was_dot || connections.iter().any(|connection| *connection != "none") {
            let north_south_empty = connections[0] == "none" && connections[1] == "none";
            let east_west_empty = connections[2] == "none" && connections[3] == "none";
            for (index, connection) in connections.iter_mut().enumerate() {
                if *connection == "none"
                    && if index < 2 {
                        east_west_empty
                    } else {
                        north_south_empty
                    }
                {
                    *connection = "side";
                }
            }
        }
        let mut result = content.block_default_state(state)?;
        result =
            content.with_state_property(result, "power", property(content, state, "power")?)?;
        for face in 2..6 {
            result = content.with_state_property(
                result,
                direction_name(face),
                connections[usize::from(face - 2)],
            )?;
        }
        Ok(result)
    }

    pub(super) fn toggled_wire_state(
        &self,
        position: [i32; 3],
        state: u32,
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let dot = (2..6).try_fold(true, |dot, face| -> Result<bool> {
            Ok(dot && property(content, state, direction_name(face))? == "none")
        })?;
        let mut seed = content.block_default_state(state)?;
        seed = content.with_state_property(seed, "power", property(content, state, "power")?)?;
        for face in 2..6 {
            seed = content.with_state_property(
                seed,
                direction_name(face),
                if dot { "side" } else { "none" },
            )?;
        }
        self.wire_connection_state(position, seed, world, content)
    }

    fn source_signal(
        &self,
        position: [i32; 3],
        face: u8,
        world: &World,
        content: &Content,
        entities: &EntityStore,
    ) -> Result<i32> {
        let state = self.use_block(world, position)?;
        let mut signal =
            self.block_signal(position, state, face, false, world, content, entities)?;
        if content.block_redstone_conductor(state)? {
            for direct_face in 0..6 {
                let neighbor = shifted(position, direct_face);
                signal = signal.max(self.block_signal(
                    neighbor,
                    self.use_block(world, neighbor)?,
                    direct_face,
                    true,
                    world,
                    content,
                    entities,
                )?);
                if signal >= 15 {
                    break;
                }
            }
        }
        Ok(signal)
    }

    fn control_signal(
        &self,
        position: [i32; 3],
        face: u8,
        world: &World,
        content: &Content,
        entities: &EntityStore,
    ) -> Result<i32> {
        let state = self.use_block(world, position)?;
        Ok(
            match content.name(state).ok_or("unknown control input state")? {
                "minecraft:redstone_block" => 15,
                "minecraft:redstone_wire" => number(content, state, "power")?,
                _ if content.block_is_signal_source(state)? => {
                    self.block_signal(position, state, face, true, world, content, entities)?
                }
                _ => 0,
            },
        )
    }

    fn analog_signal(
        &self,
        position: [i32; 3],
        state: u32,
        face: u8,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Option<i32>> {
        let Some(owner) = content.block_analog_output_owner(state)? else {
            return Ok(None);
        };
        let signal = match short_class(owner) {
            "BlockBehaviour" | "ChiseledBookShelfBlock" | "ShelfBlock" => 0,
            "CakeBlock" => (7 - number(content, state, "bites")?) * 2,
            "CandleCakeBlock" => 14,
            "ComposterBlock" => number(content, state, "level")?,
            "BeehiveBlock" => number(content, state, "honey_level")?,
            "CopperGolemStatueBlock" => {
                content.state_property_index(state, "copper_golem_pose")? as i32 + 1
            }
            "EndPortalFrameBlock" => {
                if boolean(content, state, "eye")? {
                    15
                } else {
                    0
                }
            }
            "RespawnAnchorBlock" => {
                (number(content, state, "charges")? as f32 / 4. * 15.).floor() as i32
            }
            _ => return entities.block_analog_output(position, state, content, face, world, tick),
        };
        Ok(Some(signal))
    }

    pub(super) fn refresh_comparator(
        &mut self,
        position: [i32; 3],
        state: u32,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<()> {
        let face = direction(property(content, state, "facing")?)?;
        let target = shifted(position, face);
        let target_state = self.use_block(world, target)?;
        let mut input = self.source_signal(target, face, world, content, entities)?;
        if input < 15 && content.name(target_state) == Some("minecraft:redstone_wire") {
            input = input.max(number(content, target_state, "power")?);
        }
        if let Some(analog) = self.analog_signal(
            target,
            target_state,
            opposite(face),
            world,
            content,
            entities,
            tick,
        )? {
            input = analog;
        } else if input < 15 && content.block_redstone_conductor(target_state)? {
            let farther = shifted(target, face);
            let block = self.analog_signal(
                farther,
                self.use_block(world, farther)?,
                opposite(face),
                world,
                content,
                entities,
                tick,
            )?;
            let frame = entities.item_frame_analog_output(farther, face, tick)?;
            if let Some(analog) = block.into_iter().chain(frame).max() {
                input = analog;
            }
        }
        let clockwise = match face {
            2 => 5,
            3 => 4,
            4 => 2,
            5 => 3,
            _ => unreachable!(),
        };
        let alternate = self
            .control_signal(
                shifted(position, clockwise),
                clockwise,
                world,
                content,
                entities,
            )?
            .max(self.control_signal(
                shifted(position, opposite(clockwise)),
                opposite(clockwise),
                world,
                content,
                entities,
            )?);
        let compare = property(content, state, "mode")? == "compare";
        let output = if input == 0 || alternate > input {
            0
        } else if compare {
            input
        } else {
            input - alternate
        };
        let old_output = entities.comparator_output(position).unwrap_or(0);
        if be_name(position, world, entities, content) == Some("minecraft:comparator") {
            self.predict_use(UsePrediction::ComparatorOutput {
                position,
                signal: output,
            })?;
        }
        if old_output != output || compare {
            let powered = input > 0 && (input > alternate || input == alternate && compare);
            let updated = content.with_state_property(
                state,
                "powered",
                if powered { "true" } else { "false" },
            )?;
            self.predict_block(world, position, updated, 2)?;
        }
        Ok(())
    }

    pub(super) fn wire_connects_to(
        &self,
        state: u32,
        face: Option<u8>,
        content: &Content,
    ) -> Result<bool> {
        Ok(
            match content.name(state).ok_or("unknown wire neighbor state")? {
                "minecraft:redstone_wire" => true,
                "minecraft:repeater" => {
                    let facing = direction(property(content, state, "facing")?)?;
                    face == Some(facing) || face == Some(opposite(facing))
                }
                "minecraft:observer" => {
                    face == Some(direction(property(content, state, "facing")?)?)
                }
                _ => face.is_some() && content.block_is_signal_source(state)?,
            },
        )
    }
}
