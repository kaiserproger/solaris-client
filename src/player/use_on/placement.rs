use super::survival::{
    clockwise, face, face_name, fluid_tag, full_water, metadata_state, metadata_string, same_block,
    water, water_source,
};
use super::*;
use crate::player::collision::Bounds;

#[cfg(test)]
#[path = "armor_stand_tests.rs"]
mod armor_stand_tests;

struct PlacementContext {
    hit: BlockHit,
    position: [i32; 3],
    replacing: bool,
    directions: [u8; 6],
    horizontal: u8,
    nearest: u8,
}

fn transformer_action(component: Option<&Value>) -> Option<&'static str> {
    match component.and_then(Value::as_str) {
        Some("minecraft:axe") => Some("BlockTransformerAxe"),
        Some("minecraft:hoe") => Some("BlockTransformerHoe"),
        Some("minecraft:shovel") => Some("BlockTransformerShovel"),
        _ => None,
    }
}

fn source_tool_action(
    item: &crate::inventory::Stack,
    content: &Content,
) -> Result<Option<&'static str>> {
    let defaults = content.item_components(item.id)?;
    let transformer = item
        .component("minecraft:block_transformer")
        .or_else(|| defaults.get("minecraft:block_transformer"));
    if transformer.is_some() {
        return Ok(transformer_action(transformer));
    }
    for (tag, action) in [
        ("minecraft:axes", "BlockTransformerAxe"),
        ("minecraft:hoes", "BlockTransformerHoe"),
        ("minecraft:shovels", "BlockTransformerShovel"),
    ] {
        if content.registry_tag_contains("minecraft:item", tag, item.id as i32)? {
            return Ok(Some(action));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod transformer_tests {
    use super::*;

    #[test]
    fn block_transformers_select_tool_predictions() {
        assert_eq!(
            transformer_action(Some(&serde_json::json!("minecraft:axe"))),
            Some("BlockTransformerAxe")
        );
        assert_eq!(
            transformer_action(Some(&serde_json::json!("minecraft:hoe"))),
            Some("BlockTransformerHoe")
        );
        assert_eq!(
            transformer_action(Some(&serde_json::json!("minecraft:shovel"))),
            Some("BlockTransformerShovel")
        );
    }
}

fn set(content: &Content, state: u32, key: &str, value: impl AsRef<str>) -> Result<u32> {
    content.with_state_property(state, key, value.as_ref())
}
fn set_bool(content: &Content, state: u32, key: &str, value: bool) -> Result<u32> {
    set(content, state, key, if value { "true" } else { "false" })
}
fn axis(face: u8) -> &'static str {
    ["x", "y", "z"][face_axis(i32::from(face))]
}
fn horizontal(yaw: f32) -> u8 {
    [3, 4, 2, 5][((f64::from(yaw) / 90. + 0.5).floor() as i32 & 3) as usize]
}
fn rotation(yaw: f32) -> String {
    (((yaw * (16. / 360.) + 0.5).floor() as i32) & 15).to_string()
}
fn ordered_directions(pitch: f32, yaw: f32) -> [u8; 6] {
    let pitch = f64::from(pitch * 0.017453292);
    let yaw = f64::from(-yaw * 0.017453292);
    let sy = super::super::math::sin(pitch);
    let cy = super::super::math::cos(pitch);
    let sx = super::super::math::sin(yaw);
    let cz = super::super::math::cos(yaw);
    let dx = if sx > 0. { 5 } else { 4 };
    let dy = if sy < 0. { 1 } else { 0 };
    let dz = if cz > 0. { 3 } else { 2 };
    let x = sx.abs();
    let y = sy.abs();
    let z = cz.abs();
    let [first, second, third] = if x > z {
        if y > x * cy {
            [dy, dx, dz]
        } else if z * cy > y {
            [dx, dz, dy]
        } else {
            [dx, dy, dz]
        }
    } else if y > z * cy {
        [dy, dz, dx]
    } else if x * cy > y {
        [dz, dx, dy]
    } else {
        [dz, dy, dx]
    };
    [first, second, third, third ^ 1, second ^ 1, first ^ 1]
}

impl Player {
    fn placement_target(
        &self,
        hit: &BlockHit,
        stack: &Stack,
        world: &World,
        content: &Content,
    ) -> Result<([i32; 3], bool)> {
        let clicked = self.use_block(world, hit.position)?;
        let replacing = self.block_can_replace(clicked, hit, Some(stack), true, content)?;
        let position = if replacing {
            hit.position
        } else {
            relative(hit.position, hit.face)?
        };
        Ok((position, replacing))
    }
    fn placement_context(
        &self,
        hit: &BlockHit,
        stack: &Stack,
        world: &World,
        content: &Content,
    ) -> Result<Option<PlacementContext>> {
        let (position, replacing) = self.placement_target(hit, stack, world, content)?;
        if !replacing
            && !self.block_can_replace(
                self.use_block(world, position)?,
                hit,
                Some(stack),
                false,
                content,
            )?
        {
            return Ok(None);
        }
        let mut directions = ordered_directions(self.pitch, self.yaw);
        if !replacing {
            let preferred = u8::try_from(hit.face).map_err(|_| "invalid_hit_face")? ^ 1;
            let index = directions
                .iter()
                .position(|direction| *direction == preferred)
                .ok_or("invalid_hit_face")?;
            directions[..=index].rotate_right(1);
        }
        Ok(Some(PlacementContext {
            hit: hit.clone(),
            position,
            replacing,
            directions,
            horizontal: horizontal(self.yaw),
            nearest: ordered_directions(self.pitch, self.yaw)[0],
        }))
    }
    fn placement_unobstructed(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
        placement: bool,
    ) -> Result<bool> {
        let boxes = if placement {
            content.block_placement_collision_boxes(state)?
        } else {
            content
                .collision_boxes(state)
                .ok_or("unknown_placement_collision")?
        };
        let offset = content.block_offset(state, position);
        let delta = std::array::from_fn(|i| f64::from(position[i]) + offset[i]);
        for bounds in boxes {
            let bounds = Bounds::new(*bounds).moved(delta);
            if self.game_mode != 3 && bounds.intersects(self.bounds()) {
                return Ok(false);
            }
            if !entities.placement_unobstructed(bounds, self.entity_id, tick)? {
                return Ok(false);
            }
        }
        let _ = world;
        Ok(true)
    }
    fn apply_placement_components(
        &self,
        mut state: u32,
        stack: &Stack,
        content: &Content,
    ) -> Result<u32> {
        if let Some(properties) = stack.component("minecraft:block_state") {
            let properties = properties
                .as_object()
                .ok_or("invalid_block_state_component")?;
            for (key, value) in properties {
                let value = value
                    .as_str()
                    .ok_or("invalid_block_state_component_value")?;
                if content
                    .compare_property(state, key, value, value)?
                    .is_some()
                {
                    state = set(content, state, key, value)?;
                }
            }
        }
        Ok(state)
    }
    pub(super) fn place_item(
        &mut self,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let Some(stack) =
            super::super::interaction::held(inventory, hand).filter(|stack| !stack.empty())
        else {
            return Ok(Outcome::Fail);
        };
        let Some(base) = content.item_block(stack.id)? else {
            return Ok(Outcome::Pass);
        };
        if !content
            .block_required_features(base)?
            .iter()
            .all(|feature| self.enabled_features.contains(feature))
        {
            return Ok(Outcome::Fail);
        }
        let Some(mut context) = self.placement_context(hit, stack, world, content)? else {
            return Ok(Outcome::Fail);
        };
        let item_metadata = content.item_placement_metadata(stack.id)?;
        match short_class(metadata_string(item_metadata, "update_context_owner")?) {
            "BlockItem" => {}
            "ScaffoldingBlockItem" => {
                if same_block(content, self.use_block(world, context.position)?, base)? {
                    let secondary = self.last_sent_input & 32 != 0;
                    let direction = if secondary {
                        (hit.face as u8) ^ u8::from(hit.inside)
                    } else if hit.face == 1 {
                        context.horizontal
                    } else {
                        1
                    };
                    let mut position = relative(context.position, i32::from(direction))?;
                    let mut horizontal_distance = 0;
                    loop {
                        if horizontal_distance >= 7 {
                            return Ok(Outcome::Fail);
                        }
                        let state = self.use_block(world, position)?;
                        if !same_block(content, state, base)? {
                            if !self.block_can_replace(
                                state,
                                &context.hit,
                                Some(stack),
                                context.replacing,
                                content,
                            )? {
                                return Ok(Outcome::Fail);
                            }
                            let adjusted = BlockHit {
                                position,
                                face: i32::from(direction),
                                point: std::array::from_fn(|i| {
                                    f64::from(position[i])
                                        + 0.5
                                        + f64::from(DIRECTIONS[usize::from(direction)][i]) * 0.5
                                }),
                                state,
                                distance: hit.distance,
                                inside: false,
                            };
                            let Some(updated) =
                                self.placement_context(&adjusted, stack, world, content)?
                            else {
                                return Ok(Outcome::Fail);
                            };
                            context = updated;
                            break;
                        }
                        position = relative(position, i32::from(direction))?;
                        if direction >= 2 {
                            horizontal_distance += 1;
                        }
                    }
                } else if self.placement_scaffolding_distance(context.position, world, content)?
                    == 7
                {
                    return Ok(Outcome::Fail);
                }
            }
            owner => return Err(format!("unknown Source placement context owner {owner}")),
        }
        let get_state_owner = short_class(metadata_string(item_metadata, "placement_state_owner")?);
        let state = match get_state_owner {
            "BlockItem" => {
                self.source_placement_state(base, &context, stack, world, content, entities, tick)?
            }
            "StandingAndWallBlockItem" => {
                let wall = metadata_state(item_metadata, "wall_state")?;
                let attachment = face(metadata_string(item_metadata, "attachment_direction")?)?;
                let wall_state = self.source_placement_state(
                    wall, &context, stack, world, content, entities, tick,
                )?;
                let mut selected = None;
                for direction in context.directions {
                    if direction == attachment ^ 1 {
                        continue;
                    }
                    let candidate = if direction == attachment {
                        self.source_placement_state(
                            base, &context, stack, world, content, entities, tick,
                        )?
                    } else {
                        wall_state
                    };
                    if let Some(candidate) = candidate {
                        if !self.can_survive(
                            candidate,
                            context.position,
                            world,
                            content,
                            entities,
                            tick,
                        )? {
                            continue;
                        }
                        if short_class(metadata_string(item_metadata, "can_place_owner")?)
                            == "HangingSignItem"
                            && short_class(content.block_placement_owners(candidate)?.0)
                                == "WallHangingSignBlock"
                            && !self.placement_wall_hanging_support(
                                candidate,
                                context.position,
                                world,
                                content,
                            )?
                        {
                            continue;
                        }
                        selected = Some(candidate);
                        break;
                    }
                }
                selected
            }
            owner => return Err(format!("unknown Source item placement owner {owner}")),
        };
        let Some(state) = state else {
            return Ok(Outcome::Fail);
        };
        let must_survive = item_metadata
            .get("must_survive")
            .and_then(Value::as_bool)
            .ok_or("missing_source_must_survive")?;
        if must_survive
            && !self.can_survive(state, context.position, world, content, entities, tick)?
            || !self.placement_unobstructed(
                state,
                context.position,
                world,
                content,
                entities,
                tick,
                get_state_owner == "BlockItem",
            )?
        {
            return Ok(Outcome::Fail);
        }
        match short_class(metadata_string(item_metadata, "place_block_owner")?) {
            "DoubleHighBlockItem" => {
                let above = relative(context.position, 1)?;
                let displaced = if water(content, self.use_block(world, above)?)? {
                    content.resolve_block_state("minecraft:water", &BTreeMap::new())?
                } else {
                    content.resolve_block_state("minecraft:air", &BTreeMap::new())?
                };
                self.predict_block(world, above, displaced, 27)?;
                if !self.predict_block(world, context.position, state, 11)? {
                    return Ok(Outcome::Fail);
                }
            }
            "BedItem" => {
                if !self.predict_block(world, context.position, state, 26)? {
                    return Ok(Outcome::Fail);
                }
            }
            "BlockItem" => {
                if !self.predict_block(world, context.position, state, 11)? {
                    return Ok(Outcome::Fail);
                }
            }
            owner => return Err(format!("unknown Source placeBlock owner {owner}")),
        }
        let mut placed = self.use_block(world, context.position)?;
        if same_block(content, placed, state)? {
            let modified = self.apply_placement_components(placed, stack, content)?;
            if modified != placed {
                self.predict_block(world, context.position, modified, 2)?;
                placed = modified;
            }
            if content
                .placement_metadata(placed)?
                .get("block_entity_type")
                .is_some()
            {
                self.predict_use(UsePrediction::BlockEntityComponents {
                    position: context.position,
                    components: stack.components.clone(),
                    patch: stack.patch.clone(),
                })?;
            }
            placed = self.source_set_placed_by(
                placed,
                context.position,
                stack,
                world,
                content,
                entities,
                tick,
            )?;
        }
        let sound = content.block_sound(placed)?;
        let event = match short_class(metadata_string(item_metadata, "place_sound_owner")?) {
            "BlockItem" => content.block_use_sound(placed, "place")?,
            "SolidBucketItem" => content
                .item_on_metadata(stack.id)?
                .and_then(|metadata| metadata.get("place_sound"))
                .and_then(Value::as_str)
                .ok_or("missing Source solid bucket place sound")?,
            owner => return Err(format!("unknown Source place sound owner {owner}")),
        };
        self.play_use_level_sound(
            world,
            event,
            crate::audio::SoundCategory::Blocks,
            context.position.map(|value| f64::from(value) + 0.5),
            (sound.volume + 1.) / 2.,
            sound.pitch * 0.8,
            true,
        )?;
        self.consume_held(hand, inventory, 1)?;
        Ok(Outcome::CLIENT)
    }
    fn source_placement_state(
        &self,
        base: u32,
        context: &PlacementContext,
        stack: &Stack,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Option<u32>> {
        let mut state = content.block_default_state(base)?;
        let owner = short_class(content.block_placement_owners(state)?.0);
        let position = context.position;
        let clicked = context.hit.face as u8;
        let old = self.use_block(world, position)?;
        match owner {
            "Block" | "PitcherCropBlock" => {}
            "AbstractFurnaceBlock"
            | "BeehiveBlock"
            | "CarvedPumpkinBlock"
            | "ChiseledBookShelfBlock"
            | "DiodeBlock"
            | "GlazedTerracottaBlock"
            | "LoomBlock"
            | "StonecutterBlock"
            | "VaultBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal ^ 1))?
            }
            "AnvilBlock" => {
                state = set(
                    content,
                    state,
                    "facing",
                    face_name(clockwise(context.horizontal)),
                )?
            }
            "BarrelBlock" | "CommandBlock" | "DispenserBlock" => {
                state = set(content, state, "facing", face_name(context.nearest ^ 1))?
            }
            "ObserverBlock" => state = set(content, state, "facing", face_name(context.nearest))?,
            "PistonBaseBlock" => {
                state = set(content, state, "facing", face_name(context.nearest ^ 1))?;
                state = set_bool(content, state, "extended", false)?;
            }
            "RotatedPillarBlock"
            | "InfestedRotatedPillarBlock"
            | "ChainBlock"
            | "CreakingHeartBlock" => {
                state = set(content, state, "axis", axis(clicked))?;
                if owner == "ChainBlock" {
                    state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                }
                if owner == "CreakingHeartBlock" {
                    state = self.placement_creaking_heart(state, position, world, content)?;
                }
            }
            "BarrierBlock"
            | "HeavyCoreBlock"
            | "MangroveRootsBlock"
            | "HangingRootsBlock"
            | "SculkSensorBlock"
            | "SculkShriekerBlock"
            | "WaterloggedTransparentBlock" => {
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?
            }
            "ConduitBlock" | "BaseCoralPlantTypeBlock" => {
                state = set_bool(content, state, "waterlogged", full_water(content, old)?)?
            }
            "AmethystClusterBlock" | "LightningRodBlock" => {
                state = set(content, state, "facing", face_name(clicked))?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "ShulkerBoxBlock" => state = set(content, state, "facing", face_name(clicked))?,
            "EndRodBlock" => {
                let behind = self.use_block(world, relative(position, i32::from(clicked ^ 1))?)?;
                state = set(
                    content,
                    state,
                    "facing",
                    face_name(
                        if same_block(content, behind, state)?
                            && face(property(content, behind, "facing")?)? == clicked
                        {
                            clicked ^ 1
                        } else {
                            clicked
                        },
                    ),
                )?;
            }
            "EndPortalFrameBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal ^ 1))?;
                state = set_bool(content, state, "eye", false)?;
            }
            "HopperBlock" => {
                state = set(
                    content,
                    state,
                    "facing",
                    face_name(if clicked < 2 { 0 } else { clicked ^ 1 }),
                )?;
                state = set_bool(content, state, "enabled", true)?;
            }
            "BannerBlock" | "StandingSignBlock" | "SkullBlock" => {
                state = set(
                    content,
                    state,
                    "rotation",
                    rotation(self.yaw + if owner == "SkullBlock" { 0. } else { 180. }),
                )?;
                if owner == "StandingSignBlock" {
                    state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                }
            }
            "CopperGolemStatueBlock" | "EnderChestBlock" | "DriedGhastBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal ^ 1))?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "DecoratedPotBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal))?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                state = set_bool(content, state, "cracked", false)?;
            }
            "CalibratedSculkSensorBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal))?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "LecternBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal ^ 1))?;
                state = set_bool(content, state, "has_book", false)?;
            }
            "ShelfBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal ^ 1))?;
                state = set_bool(
                    content,
                    state,
                    "powered",
                    self.neighbor_signal(position, world, content, entities)?,
                )?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "RedstoneLampBlock" => {
                state = set_bool(
                    content,
                    state,
                    "lit",
                    self.neighbor_signal(position, world, content, entities)?,
                )?
            }
            "JigsawBlock" | "CrafterBlock" => {
                let front = if owner == "JigsawBlock" {
                    clicked
                } else {
                    context.nearest ^ 1
                };
                let top = if front < 2 {
                    if owner == "CrafterBlock" && front == 0 {
                        context.horizontal
                    } else {
                        context.horizontal ^ 1
                    }
                } else {
                    1
                };
                state = set(
                    content,
                    state,
                    "orientation",
                    format!("{}_{}", face_name(front), face_name(top)),
                )?;
                if owner == "CrafterBlock" {
                    state = set_bool(
                        content,
                        state,
                        "triggered",
                        self.neighbor_signal(position, world, content, entities)?,
                    )?;
                }
            }
            "BaseRailBlock" => {
                state = set(
                    content,
                    state,
                    "shape",
                    if context.horizontal == 4 || context.horizontal == 5 {
                        "east_west"
                    } else {
                        "north_south"
                    },
                )?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "CandleBlock" | "SeaPickleBlock" | "TurtleEggBlock" | "SnowLayerBlock"
            | "FlowerBedBlock" | "LeafLitterBlock" => {
                let key = match owner {
                    "CandleBlock" => "candles",
                    "SeaPickleBlock" => "pickles",
                    "TurtleEggBlock" => "eggs",
                    "SnowLayerBlock" => "layers",
                    "FlowerBedBlock" => "flower_amount",
                    _ => "segment_amount",
                };
                if same_block(content, old, state)? {
                    state = set(
                        content,
                        old,
                        key,
                        (number(content, old, key)? + 1)
                            .min(if owner == "SnowLayerBlock" { 8 } else { 4 })
                            .to_string(),
                    )?;
                } else {
                    if owner == "CandleBlock" || owner == "SeaPickleBlock" {
                        state =
                            set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                    }
                    if owner == "FlowerBedBlock" || owner == "LeafLitterBlock" {
                        state = set(content, state, "facing", face_name(context.horizontal ^ 1))?;
                    }
                }
            }
            "SlabBlock" => {
                if same_block(content, old, state)? {
                    state = set_bool(
                        content,
                        set(content, old, "type", "double")?,
                        "waterlogged",
                        false,
                    )?;
                } else {
                    let bottom = clicked != 0
                        && (clicked == 1 || context.hit.point[1] - f64::from(position[1]) <= 0.5);
                    state = set(
                        content,
                        state,
                        "type",
                        if bottom { "bottom" } else { "top" },
                    )?;
                    state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                }
            }
            "StairBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal))?;
                state = set(
                    content,
                    state,
                    "half",
                    if clicked != 0
                        && (clicked == 1 || context.hit.point[1] - f64::from(position[1]) <= 0.5)
                    {
                        "bottom"
                    } else {
                        "top"
                    },
                )?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                state = self.placement_stair_shape(state, position, world, content)?;
            }
            "TrapDoorBlock" => {
                let side = !context.replacing && clicked >= 2;
                state = set(
                    content,
                    state,
                    "facing",
                    face_name(if side {
                        clicked
                    } else {
                        context.horizontal ^ 1
                    }),
                )?;
                state = set(
                    content,
                    state,
                    "half",
                    if side {
                        if context.hit.point[1] - f64::from(position[1]) > 0.5 {
                            "top"
                        } else {
                            "bottom"
                        }
                    } else if clicked == 1 {
                        "bottom"
                    } else {
                        "top"
                    },
                )?;
                if self.neighbor_signal(position, world, content, entities)? {
                    state = set_bool(
                        content,
                        set_bool(content, state, "open", true)?,
                        "powered",
                        true,
                    )?;
                }
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "DoorBlock" => {
                let above = relative(position, 1)?;
                if position[1] >= world.min_y + world.height - 1
                    || !self.block_can_replace(
                        self.use_block(world, above)?,
                        &context.hit,
                        Some(stack),
                        context.replacing,
                        content,
                    )?
                {
                    return Ok(None);
                }
                state = set(content, state, "facing", face_name(context.horizontal))?;
                state = set(
                    content,
                    state,
                    "hinge",
                    self.placement_door_hinge(&context, world, content)?,
                )?;
                let powered = self.neighbor_signal(position, world, content, entities)?
                    || self.neighbor_signal(above, world, content, entities)?;
                state = set_bool(
                    content,
                    set_bool(
                        content,
                        set(content, state, "half", "lower")?,
                        "open",
                        powered,
                    )?,
                    "powered",
                    powered,
                )?;
            }
            "AbstractBedBlock" => {
                let head = relative(position, i32::from(context.horizontal))?;
                if !world.border_contains_block(head)
                    || !self.block_can_replace(
                        self.use_block(world, head)?,
                        &context.hit,
                        Some(stack),
                        context.replacing,
                        content,
                    )?
                {
                    return Ok(None);
                }
                state = set(content, state, "facing", face_name(context.horizontal))?;
            }
            "DoublePlantBlock" | "SmallDripleafBlock" | "TallSeagrassBlock" => {
                let above = relative(position, 1)?;
                if position[1] >= world.min_y + world.height - 1
                    || !self.block_can_replace(
                        self.use_block(world, above)?,
                        &context.hit,
                        Some(stack),
                        context.replacing,
                        content,
                    )?
                {
                    return Ok(None);
                }
                if owner == "SmallDripleafBlock" {
                    state = set(content, state, "facing", face_name(context.horizontal ^ 1))?;
                    state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                }
                if owner == "TallSeagrassBlock"
                    && !full_water(content, self.use_block(world, above)?)?
                {
                    return Ok(None);
                }
            }
            "BambooStalkBlock" => {
                if content.info(old)?.fluid.is_some() {
                    return Ok(None);
                }
                let below = self.use_block(world, relative(position, 0)?)?;
                if !content.block_has_tag(below, "minecraft:supports_bamboo")? {
                    return Ok(None);
                }
                match content.name(below) {
                    Some("minecraft:bamboo_sapling") => state = set(content, state, "age", "0")?,
                    Some("minecraft:bamboo") => {
                        state = set(
                            content,
                            state,
                            "age",
                            if number(content, below, "age")? > 0 {
                                "1"
                            } else {
                                "0"
                            },
                        )?
                    }
                    _ => {
                        let above = self.use_block(world, relative(position, 1)?)?;
                        state = if content.name(above) == Some("minecraft:bamboo") {
                            set(content, state, "age", property(content, above, "age")?)?
                        } else {
                            content
                                .resolve_block_state("minecraft:bamboo_sapling", &BTreeMap::new())?
                        };
                    }
                }
            }
            "MangrovePropaguleBlock" => {
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                state = set(content, state, "age", "4")?;
            }
            "GrowingPlantBlock" | "KelpBlock" => {
                if owner == "KelpBlock" && !full_water(content, old)? {
                    return Ok(None);
                }
                state = self.placement_growing_plant(state, position, world, content)?;
            }
            "SeagrassBlock" => {
                if !full_water(content, old)? {
                    return Ok(None);
                }
            }
            "BigDripleafBlock" => {
                let below = self.use_block(world, relative(position, 0)?)?;
                let direction = if matches!(
                    content.name(below),
                    Some("minecraft:big_dripleaf" | "minecraft:big_dripleaf_stem")
                ) {
                    face(property(content, below, "facing")?)?
                } else {
                    context.horizontal ^ 1
                };
                state = set(content, state, "facing", face_name(direction))?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "ScaffoldingBlock" => {
                let distance = self.placement_scaffolding_distance(position, world, content)?;
                state = set(content, state, "distance", distance.to_string())?;
                state = set_bool(
                    content,
                    state,
                    "bottom",
                    distance > 0
                        && content.name(self.use_block(world, relative(position, 0)?)?)
                            != Some("minecraft:scaffolding"),
                )?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
            }
            "SnowyBlock" => {
                state = set_bool(
                    content,
                    state,
                    "snowy",
                    content.block_has_tag(
                        self.use_block(world, relative(position, 1)?)?,
                        "minecraft:snow",
                    )?,
                )?
            }
            "LeavesBlock" => {
                state = set_bool(
                    content,
                    set_bool(content, state, "persistent", true)?,
                    "waterlogged",
                    water_source(content, old)?,
                )?;
                state = self.placement_leaves_distance(state, position, world, content)?;
            }
            "CampfireBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal))?;
                state = set_bool(content, state, "waterlogged", water_source(content, old)?)?;
                state = set_bool(content, state, "lit", !water_source(content, old)?)?;
                state = set_bool(
                    content,
                    state,
                    "signal_fire",
                    content.name(self.use_block(world, relative(position, 0)?)?)
                        == Some("minecraft:hay_block"),
                )?;
            }
            "FenceGateBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal))?;
                let powered = self.neighbor_signal(position, world, content, entities)?;
                state = set_bool(
                    content,
                    set_bool(content, state, "open", powered)?,
                    "powered",
                    powered,
                )?;
                let sides = if axis(context.horizontal) == "z" {
                    [4, 5]
                } else {
                    [2, 3]
                };
                let walls = content.block_has_tag(
                    self.use_block(world, relative(position, sides[0])?)?,
                    "minecraft:walls",
                )? || content.block_has_tag(
                    self.use_block(world, relative(position, sides[1])?)?,
                    "minecraft:walls",
                )?;
                state = set_bool(content, state, "in_wall", walls)?;
            }
            "FaceAttachedHorizontalDirectionalBlock"
            | "GrindstoneBlock"
            | "LanternBlock"
            | "LadderBlock"
            | "WallTorchBlock"
            | "RedstoneWallTorchBlock"
            | "WallBannerBlock"
            | "WallSignBlock"
            | "WallSkullBlock"
            | "WallHangingSignBlock"
            | "BaseCoralWallFanBlock"
            | "CocoaBlock"
            | "TripWireHookBlock" => {
                return self.placement_attached_state(
                    state, owner, context, stack, world, content, entities, tick,
                );
            }
            "BellBlock" => {
                return self.placement_bell(state, context, world, content, entities, tick);
            }
            "CeilingHangingSignBlock" => {
                state = self.placement_ceiling_sign(state, context, world, content)?
            }
            "VineBlock" | "MultifaceBlock" => {
                return self.placement_multiface_state(state, owner, context, world, content);
            }
            "FenceBlock" | "IronBarsBlock" | "WallBlock" | "TripWireBlock" | "ChorusPlantBlock"
            | "HugeMushroomBlock" => {
                state = self.placement_connections(state, owner, position, world, content)?
            }
            "ChestBlock" | "CopperChestBlock" => {
                state = self.placement_chest(state, owner, context, world, content)?
            }
            "ConcretePowderBlock" => {
                state = self.placement_concrete(state, position, old, world, content)?
            }
            "PathBlock" | "FarmlandBlock" => {
                if !self.can_survive(state, position, world, content, entities, tick)? {
                    state = content.resolve_block_state("minecraft:dirt", &BTreeMap::new())?;
                }
            }
            "BaseFireBlock" | "FireBlock" => {
                state = self.placement_fire(state, owner, position, world, content)?
            }
            "NoteBlock" => state = self.placement_note(state, position, world, content)?,
            "RepeaterBlock" => {
                state = set(content, state, "facing", face_name(context.horizontal ^ 1))?;
                state = set_bool(
                    content,
                    state,
                    "locked",
                    self.placement_repeater_locked(state, position, world, content, entities)?,
                )?;
            }
            "SpeleothemBlock" => {
                return self.placement_dripstone(state, context, world, content);
            }
            "RedstoneWireBlock" => state = self.placement_wire(state, position, world, content)?,
            "MossyCarpetBlock" => state = self.placement_mossy(state, position, world, content)?,
            "TestBlock" => {
                if let Some(mode) = stack
                    .component("minecraft:block_state")
                    .and_then(|properties| properties.get("mode"))
                    .and_then(Value::as_str)
                {
                    if content
                        .compare_property(state, "mode", mode, mode)?
                        .is_some()
                    {
                        state = set(content, state, "mode", mode)?;
                    }
                }
            }
            "CoralBlock" => {
                let mut wet = false;
                for toward in 0..6 {
                    if water(content, self.use_block(world, relative(position, toward)?)?)? {
                        wet = true;
                        break;
                    }
                }
                if !wet {
                    let _ = world.next_int_bound(40)?;
                }
            }
            owner => return Err(format!("unknown Source placement owner {owner}")),
        }
        Ok(Some(state))
    }
}

fn copy_properties(content: &Content, target: u32, source: u32) -> Result<u32> {
    let mut result = content.block_default_state(target)?;
    for (key, value) in content
        .state_properties(source)
        .ok_or("unknown_source_state_properties")?
    {
        if content
            .compare_property(result, key, value, value)?
            .is_some()
        {
            result = set(content, result, key, value)?;
        }
    }
    Ok(result)
}
fn is_owner(content: &Content, state: u32, owner: &str) -> Result<bool> {
    Ok(short_class(content.block_placement_owners(state)?.0) == owner)
}
fn connection_exception(content: &Content, state: u32) -> Result<bool> {
    Ok(is_owner(content, state, "LeavesBlock")?
        || matches!(
            content.name(state),
            Some(
                "minecraft:barrier"
                    | "minecraft:carved_pumpkin"
                    | "minecraft:jack_o_lantern"
                    | "minecraft:melon"
                    | "minecraft:pumpkin"
            )
        )
        || content.block_has_tag(state, "minecraft:shulker_boxes")?)
}
impl Player {
    fn placement_creaking_heart(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let state_axis = property(content, state, "axis")?;
        let directions = match state_axis {
            "x" => [4, 5],
            "y" => [0, 1],
            "z" => [2, 3],
            _ => return Err("invalid_creaking_heart_axis".into()),
        };
        for direction in directions {
            let neighbor = self.use_block(world, relative(position, direction)?)?;
            if !content.block_has_tag(neighbor, "minecraft:pale_oak_logs")?
                || property(content, neighbor, "axis")? != state_axis
            {
                return Ok(state);
            }
        }
        if property(content, state, "creaking_heart_state")? != "uprooted" {
            return Ok(state);
        }
        set(
            content,
            state,
            "creaking_heart_state",
            if world.creaking_active(position, content)? {
                "awake"
            } else {
                "dormant"
            },
        )
    }
    fn placement_stair_shape(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let facing = face(property(content, state, "facing")?)?;
        let half = property(content, state, "half")?;
        let compatible = |neighbor: u32| -> Result<bool> {
            Ok(is_owner(content, neighbor, "StairBlock")?
                && property(content, neighbor, "half")? == half)
        };
        let can_take = |direction: u8| -> Result<bool> {
            let neighbor = self.use_block(world, relative(position, i32::from(direction))?)?;
            Ok(!is_owner(content, neighbor, "StairBlock")?
                || property(content, neighbor, "facing")? != face_name(facing)
                || property(content, neighbor, "half")? != half)
        };
        let front = self.use_block(world, relative(position, i32::from(facing))?)?;
        if compatible(front)? {
            let neighbor_facing = face(property(content, front, "facing")?)?;
            if axis(neighbor_facing) != axis(facing) && can_take(neighbor_facing ^ 1)? {
                return set(
                    content,
                    state,
                    "shape",
                    if neighbor_facing == clockwise(facing) ^ 1 {
                        "outer_left"
                    } else {
                        "outer_right"
                    },
                );
            }
        }
        let back = self.use_block(world, relative(position, i32::from(facing ^ 1))?)?;
        if compatible(back)? {
            let neighbor_facing = face(property(content, back, "facing")?)?;
            if axis(neighbor_facing) != axis(facing) && can_take(neighbor_facing)? {
                return set(
                    content,
                    state,
                    "shape",
                    if neighbor_facing == clockwise(facing) ^ 1 {
                        "inner_left"
                    } else {
                        "inner_right"
                    },
                );
            }
        }
        set(content, state, "shape", "straight")
    }
    fn placement_door_hinge(
        &self,
        context: &PlacementContext,
        world: &World,
        content: &Content,
    ) -> Result<&'static str> {
        let left_pos = relative(
            context.position,
            i32::from(clockwise(context.horizontal) ^ 1),
        )?;
        let right_pos = relative(context.position, i32::from(clockwise(context.horizontal)))?;
        let left = self.use_block(world, left_pos)?;
        let right = self.use_block(world, right_pos)?;
        let balance = -(i32::from(content.info(left)?.collision_full)
            + i32::from(
                content
                    .info(self.use_block(world, relative(left_pos, 1)?)?)?
                    .collision_full,
            ))
            + i32::from(content.info(right)?.collision_full)
            + i32::from(
                content
                    .info(self.use_block(world, relative(right_pos, 1)?)?)?
                    .collision_full,
            );
        let left_door =
            is_owner(content, left, "DoorBlock")? && property(content, left, "half")? == "lower";
        let right_door =
            is_owner(content, right, "DoorBlock")? && property(content, right, "half")? == "lower";
        if (!left_door || right_door) && balance <= 0 {
            if (!right_door || left_door) && balance >= 0 {
                let [step_x, _, step_z] = DIRECTIONS[usize::from(context.horizontal)];
                let x = context.hit.point[0] - f64::from(context.position[0]);
                let z = context.hit.point[2] - f64::from(context.position[2]);
                Ok(
                    if (step_x >= 0 || !(z < 0.5))
                        && (step_x <= 0 || !(z > 0.5))
                        && (step_z >= 0 || !(x > 0.5))
                        && (step_z <= 0 || !(x < 0.5))
                    {
                        "left"
                    } else {
                        "right"
                    },
                )
            } else {
                Ok("left")
            }
        } else {
            Ok("right")
        }
    }
    fn placement_growing_plant(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let metadata = content.placement_metadata(state)?;
        let direction = face(metadata_string(metadata, "growth_direction")?)?;
        let head = metadata_state(metadata, "head_state")?;
        let body = metadata_state(metadata, "body_state")?;
        let neighbor = self.use_block(world, relative(position, i32::from(direction))?)?;
        if same_block(content, neighbor, head)? || same_block(content, neighbor, body)? {
            return content.block_default_state(body);
        }
        match short_class(metadata_string(metadata, "random_placement_owner")?) {
            "GrowingPlantBlock" => Ok(state),
            "GrowingPlantHeadBlock" => {
                set(content, state, "age", world.next_int_bound(25)?.to_string())
            }
            "CaveVinesBlock" => set_bool(
                content,
                set(content, state, "age", world.next_int_bound(25)?.to_string())?,
                "berries",
                world.next_float()? < 0.11,
            ),
            owner => Err(format!(
                "unknown Source random plant placement owner {owner}"
            )),
        }
    }
    fn placement_leaves_distance(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let mut distance = 7;
        for direction in 0..6 {
            let neighbor = self.use_block(world, relative(position, direction)?)?;
            let neighbor_distance = if content.block_has_tag(neighbor, "minecraft:logs")? {
                0
            } else if content
                .state_properties(neighbor)
                .ok_or("unknown_leaf_neighbor")?
                .contains_key("distance")
            {
                number(content, neighbor, "distance")?
            } else {
                7
            };
            distance = distance.min(neighbor_distance + 1);
            if distance == 1 {
                break;
            }
        }
        set(content, state, "distance", distance.to_string())
    }
    fn placement_attached_state(
        &self,
        base: u32,
        owner: &str,
        context: &PlacementContext,
        stack: &Stack,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Option<u32>> {
        if owner == "LadderBlock" && !context.replacing {
            let behind =
                self.use_block(world, relative(context.position, context.hit.face ^ 1)?)?;
            if same_block(content, behind, base)?
                && face(property(content, behind, "facing")?)? == context.hit.face as u8
            {
                return Ok(None);
            }
        }
        let old = self.use_block(world, context.position)?;
        for direction in context.directions {
            let mut candidate = base;
            match owner {
                "FaceAttachedHorizontalDirectionalBlock" | "GrindstoneBlock" => {
                    candidate = set(
                        content,
                        candidate,
                        "face",
                        if direction == 0 {
                            "floor"
                        } else if direction == 1 {
                            "ceiling"
                        } else {
                            "wall"
                        },
                    )?;
                    candidate = set(
                        content,
                        candidate,
                        "facing",
                        face_name(if direction < 2 {
                            context.horizontal
                        } else {
                            direction ^ 1
                        }),
                    )?;
                }
                "LanternBlock" => {
                    if direction >= 2 {
                        continue;
                    }
                    candidate = set_bool(content, candidate, "hanging", direction == 1)?;
                }
                "LadderBlock"
                | "WallTorchBlock"
                | "RedstoneWallTorchBlock"
                | "WallBannerBlock"
                | "WallSignBlock"
                | "WallSkullBlock"
                | "WallHangingSignBlock"
                | "BaseCoralWallFanBlock"
                | "CocoaBlock"
                | "TripWireHookBlock" => {
                    if direction < 2
                        || owner == "WallHangingSignBlock"
                            && axis(direction) == axis(context.hit.face as u8)
                    {
                        continue;
                    }
                    candidate = set(
                        content,
                        candidate,
                        "facing",
                        face_name(if owner == "CocoaBlock" {
                            direction
                        } else {
                            direction ^ 1
                        }),
                    )?;
                    if owner == "TripWireHookBlock" {
                        candidate = set_bool(
                            content,
                            set_bool(content, candidate, "powered", false)?,
                            "attached",
                            false,
                        )?;
                    }
                    if owner == "WallSkullBlock" {
                        if !self.block_can_replace(
                            self.use_block(
                                world,
                                relative(context.position, i32::from(direction))?,
                            )?,
                            &context.hit,
                            Some(stack),
                            context.replacing,
                            content,
                        )? {
                            return Ok(Some(candidate));
                        }
                        continue;
                    }
                    if owner == "BaseCoralWallFanBlock" {
                        candidate =
                            set_bool(content, candidate, "waterlogged", full_water(content, old)?)?;
                    }
                }
                owner => return Err(format!("unknown Source attachment owner {owner}")),
            }
            if !self.can_survive(candidate, context.position, world, content, entities, tick)? {
                continue;
            }
            if owner == "WallHangingSignBlock"
                && !self.placement_wall_hanging_support(
                    candidate,
                    context.position,
                    world,
                    content,
                )?
            {
                continue;
            }
            if matches!(
                owner,
                "LadderBlock" | "WallSignBlock" | "WallHangingSignBlock" | "LanternBlock"
            ) {
                candidate = set_bool(
                    content,
                    candidate,
                    "waterlogged",
                    water_source(content, old)?,
                )?;
            }
            return Ok(Some(candidate));
        }
        Ok(None)
    }
    fn placement_bell(
        &self,
        base: u32,
        context: &PlacementContext,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Option<u32>> {
        let clicked = context.hit.face as u8;
        let mut state = set(
            content,
            base,
            "facing",
            face_name(if clicked < 2 {
                context.horizontal
            } else {
                clicked ^ 1
            }),
        )?;
        let attachment = if clicked == 0 {
            "ceiling"
        } else if clicked == 1 {
            "floor"
        } else {
            if self.placement_supports(context.position, clicked, 0, world, content)?
                && self.placement_supports(context.position, clicked ^ 1, 0, world, content)?
            {
                "double_wall"
            } else {
                "single_wall"
            }
        };
        state = set(content, state, "attachment", attachment)?;
        if self.can_survive(state, context.position, world, content, entities, tick)? {
            return Ok(Some(state));
        }
        if clicked >= 2 {
            state = set(
                content,
                state,
                "attachment",
                if self.placement_supports(context.position, 0, 0, world, content)? {
                    "floor"
                } else {
                    "ceiling"
                },
            )?;
            if self.can_survive(state, context.position, world, content, entities, tick)? {
                return Ok(Some(state));
            }
        }
        Ok(None)
    }
    fn placement_ceiling_sign(
        &self,
        state: u32,
        context: &PlacementContext,
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let above = self.use_block(world, relative(context.position, 1)?)?;
        let secondary = self.last_sent_input & 32 != 0;
        let mut attached = !content.block_collision_face_full(above, 0)? || secondary;
        if content.block_has_tag(above, "minecraft:all_hanging_signs")? && !secondary {
            let properties = content
                .state_properties(above)
                .ok_or("unknown_hanging_sign_neighbor")?;
            if properties.contains_key("facing") {
                if axis(face(property(content, above, "facing")?)?) == axis(context.horizontal) {
                    attached = false;
                }
            } else if properties.contains_key("rotation") {
                let above_rotation = number(content, above, "rotation")?;
                if above_rotation % 4 == 0 {
                    let above_facing = [2, 5, 3, 4][(above_rotation / 4) as usize];
                    if axis(above_facing) == axis(context.horizontal) {
                        attached = false;
                    }
                }
            }
        }
        let rotation = if attached {
            rotation(self.yaw + 180.)
        } else {
            let direction = context.horizontal ^ 1;
            (match direction {
                3 => 0,
                4 => 4,
                2 => 8,
                5 => 12,
                _ => return Err("invalid_hanging_sign_direction".into()),
            })
            .to_string()
        };
        set_bool(
            content,
            set(
                content,
                set_bool(content, state, "attached", attached)?,
                "rotation",
                rotation,
            )?,
            "waterlogged",
            water_source(content, self.use_block(world, context.position)?)?,
        )
    }
    fn placement_multiface_state(
        &self,
        base: u32,
        owner: &str,
        context: &PlacementContext,
        world: &World,
        content: &Content,
    ) -> Result<Option<u32>> {
        let old = self.use_block(world, context.position)?;
        let existing = same_block(content, old, base)?;
        let mut state = if existing { old } else { base };
        for direction in context.directions {
            let key = face_name(direction);
            if !content
                .state_properties(base)
                .ok_or("unknown_multiface_state")?
                .contains_key(key)
                || existing && boolean(content, old, key)?
            {
                continue;
            }
            let support = if owner == "VineBlock" {
                self.placement_vine_support(context.position, direction, world, content)?
            } else {
                self.placement_multiface_support(context.position, direction, world, content)?
            };
            if support {
                if owner == "MultifaceBlock" && !existing && water_source(content, old)? {
                    state = set_bool(content, state, "waterlogged", true)?;
                }
                return Ok(Some(set_bool(content, state, key, true)?));
            }
        }
        Ok(if owner == "VineBlock" && existing {
            Some(state)
        } else {
            None
        })
    }
}

impl Player {
    fn placement_connections(
        &self,
        mut state: u32,
        owner: &str,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        if matches!(owner, "FenceBlock" | "IronBarsBlock" | "WallBlock") {
            state = set_bool(
                content,
                state,
                "waterlogged",
                water_source(content, self.use_block(world, position)?)?,
            )?;
        }
        let mut wall_sides = ["none"; 6];
        let above = if owner == "WallBlock" {
            Some(self.use_block(world, relative(position, 1)?)?)
        } else {
            None
        };
        let hook = if owner == "TripWireBlock" {
            Some(metadata_state(
                content.placement_metadata(state)?,
                "hook_state",
            )?)
        } else {
            None
        };
        for direction in 0u8..6 {
            if direction < 2 && !matches!(owner, "HugeMushroomBlock" | "ChorusPlantBlock") {
                continue;
            }
            let neighbor = self.use_block(world, relative(position, i32::from(direction))?)?;
            let gate = || -> Result<bool> {
                Ok(is_owner(content, neighbor, "FenceGateBlock")?
                    && axis(face(property(content, neighbor, "facing")?)?) != axis(direction))
            };
            let connected = match owner {
                "HugeMushroomBlock" => !same_block(content, neighbor, state)?,
                "ChorusPlantBlock" => {
                    same_block(content, neighbor, state)?
                        || content.name(neighbor) == Some("minecraft:chorus_flower")
                        || direction == 0
                            && content.block_has_tag(neighbor, "minecraft:supports_chorus_plant")?
                }
                "TripWireBlock" => {
                    if same_block(content, neighbor, hook.ok_or("missing_tripwire_hook")?)? {
                        face(property(content, neighbor, "facing")?)? == direction ^ 1
                    } else {
                        same_block(content, neighbor, state)?
                    }
                }
                "FenceBlock" => {
                    (!connection_exception(content, neighbor)?
                        && content.block_supports(neighbor, direction ^ 1, 0)?)
                        || content.block_has_tag(neighbor, "minecraft:fences")?
                            && content.block_has_tag(neighbor, "minecraft:wooden_fences")?
                                == content.block_has_tag(state, "minecraft:wooden_fences")?
                        || gate()?
                }
                "IronBarsBlock" => {
                    (!connection_exception(content, neighbor)?
                        && content.block_supports(neighbor, direction ^ 1, 0)?)
                        || is_owner(content, neighbor, "IronBarsBlock")?
                        || content.block_has_tag(neighbor, "minecraft:walls")?
                }
                "WallBlock" => {
                    content.block_has_tag(neighbor, "minecraft:walls")?
                        || (!connection_exception(content, neighbor)?
                            && content.block_supports(neighbor, direction ^ 1, 0)?)
                        || is_owner(content, neighbor, "IronBarsBlock")?
                        || gate()?
                }
                owner => return Err(format!("unknown Source connection owner {owner}")),
            };
            if let Some(above) = above {
                let rectangle = match direction {
                    2 => [7. / 16., 0., 9. / 16., 9. / 16.],
                    3 => [7. / 16., 7. / 16., 9. / 16., 1.],
                    4 => [0., 7. / 16., 9. / 16., 9. / 16.],
                    5 => [7. / 16., 7. / 16., 1., 9. / 16.],
                    _ => return Err("invalid_wall_direction".into()),
                };
                let side = if !connected {
                    "none"
                } else if content.block_collision_face_covers(above, 0, rectangle)? {
                    "tall"
                } else {
                    "low"
                };
                wall_sides[usize::from(direction)] = side;
                state = set(content, state, face_name(direction), side)?;
            } else {
                state = set_bool(content, state, face_name(direction), connected)?;
            }
        }
        if let Some(above) = above {
            let [north, south, west, east] =
                [wall_sides[2], wall_sides[3], wall_sides[4], wall_sides[5]];
            let corner = [north, south, west, east]
                .iter()
                .all(|side| *side == "none")
                || (north == "none") != (south == "none")
                || (west == "none") != (east == "none");
            let high = north == "tall" && south == "tall" || west == "tall" && east == "tall";
            let post = is_owner(content, above, "WallBlock")? && boolean(content, above, "up")?
                || corner
                || !high
                    && (content.block_has_tag(above, "minecraft:wall_post_override")?
                        || content.block_collision_face_covers(
                            above,
                            0,
                            [7. / 16., 7. / 16., 9. / 16., 9. / 16.],
                        )?);
            state = set_bool(content, state, "up", post)?;
        }
        Ok(state)
    }
    fn placement_chest(
        &self,
        base: u32,
        owner: &str,
        context: &PlacementContext,
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let candidate = |direction: u8| -> Result<Option<u8>> {
            let neighbor =
                self.use_block(world, relative(context.position, i32::from(direction))?)?;
            let connects = if owner == "CopperChestBlock" {
                content.block_has_tag(neighbor, "minecraft:copper_chests")?
                    && content
                        .state_properties(neighbor)
                        .ok_or("unknown_chest_neighbor")?
                        .contains_key("type")
            } else {
                same_block(content, neighbor, base)?
            };
            Ok(
                if connects && property(content, neighbor, "type")? == "single" {
                    Some(face(property(content, neighbor, "facing")?)?)
                } else {
                    None
                },
            )
        };
        let mut facing = context.horizontal ^ 1;
        let mut chest_type = "single";
        let secondary = self.last_sent_input & 32 != 0;
        let clicked = context.hit.face as u8;
        if clicked >= 2 && secondary {
            if let Some(neighbor_facing) = candidate(clicked ^ 1)? {
                if axis(neighbor_facing) != axis(clicked) {
                    facing = neighbor_facing;
                    chest_type = if clockwise(neighbor_facing) ^ 1 == clicked ^ 1 {
                        "right"
                    } else {
                        "left"
                    };
                }
            }
        }
        if chest_type == "single" && !secondary {
            chest_type = if candidate(clockwise(facing))? == Some(facing) {
                "left"
            } else if candidate(clockwise(facing) ^ 1)? == Some(facing) {
                "right"
            } else {
                "single"
            };
        }
        let mut state = set_bool(
            content,
            set(
                content,
                set(content, base, "facing", face_name(facing))?,
                "type",
                chest_type,
            )?,
            "waterlogged",
            water_source(content, self.use_block(world, context.position)?)?,
        )?;
        if owner == "CopperChestBlock" && chest_type != "single" {
            let connected_direction = if chest_type == "left" {
                clockwise(facing)
            } else {
                clockwise(facing) ^ 1
            };
            let connected = self.use_block(
                world,
                relative(context.position, i32::from(connected_direction))?,
            )?;
            if is_owner(content, connected, "CopperChestBlock")? {
                let own_metadata = content.placement_metadata(state)?;
                let connected_metadata = content.placement_metadata(connected)?;
                let waxed = |metadata: &Value| {
                    metadata
                        .get("waxed")
                        .and_then(Value::as_bool)
                        .ok_or("missing_copper_chest_wax_state")
                };
                let weathering = |metadata: &Value| {
                    metadata
                        .get("weathering")
                        .and_then(Value::as_u64)
                        .ok_or("missing_copper_chest_weathering")
                };
                let mut connected_prediction = connected;
                if waxed(own_metadata)? != waxed(connected_metadata)? {
                    if waxed(own_metadata)? {
                        state = content.block_unwaxed_state(state)?.unwrap_or(state);
                    }
                    if waxed(connected_metadata)? {
                        connected_prediction =
                            content.block_unwaxed_state(connected)?.unwrap_or(connected);
                    }
                }
                if weathering(own_metadata)? > weathering(connected_metadata)? {
                    state = copy_properties(content, connected_prediction, state)?;
                }
            }
        }
        Ok(state)
    }
    fn placement_concrete(
        &self,
        state: u32,
        position: [i32; 3],
        old: u32,
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let concrete = metadata_state(content.placement_metadata(state)?, "concrete_state")?;
        if fluid_tag(content, old, "minecraft:water")? {
            return content.block_default_state(concrete);
        }
        for direction in 1u8..6 {
            let neighbor = self.use_block(world, relative(position, i32::from(direction))?)?;
            if fluid_tag(content, neighbor, "minecraft:water")?
                && !content.block_supports(neighbor, direction ^ 1, 0)?
            {
                return content.block_default_state(concrete);
            }
        }
        Ok(state)
    }
    fn placement_fire(
        &self,
        state: u32,
        owner: &str,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let below = self.use_block(world, relative(position, 0)?)?;
        if owner == "BaseFireBlock"
            && content.block_has_tag(below, "minecraft:soul_fire_base_blocks")?
        {
            return content.resolve_block_state("minecraft:soul_fire", &BTreeMap::new());
        }
        let mut result = if owner == "BaseFireBlock" {
            content.resolve_block_state("minecraft:fire", &BTreeMap::new())?
        } else {
            state
        };
        if content.block_can_burn(below)? || content.block_supports(below, 1, 0)? {
            return Ok(result);
        }
        for direction in 1u8..6 {
            result = set_bool(
                content,
                result,
                face_name(direction),
                content.block_can_burn(
                    self.use_block(world, relative(position, i32::from(direction))?)?,
                )?,
            )?;
        }
        Ok(result)
    }
    fn placement_note(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let above =
            content.block_note_instrument(self.use_block(world, relative(position, 1)?)?)?;
        let instrument = if above.1 {
            above.0
        } else {
            let below =
                content.block_note_instrument(self.use_block(world, relative(position, 0)?)?)?;
            if below.1 { "harp" } else { below.0 }
        };
        set(content, state, "instrument", instrument)
    }
    fn placement_repeater_locked(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
        entities: &EntityStore,
    ) -> Result<bool> {
        let facing = face(property(content, state, "facing")?)?;
        let mut maximum = 0;
        for direction in [clockwise(facing), clockwise(facing) ^ 1] {
            let neighbor_position = relative(position, i32::from(direction))?;
            let neighbor = self.use_block(world, neighbor_position)?;
            if content
                .placement_metadata(neighbor)?
                .get("hierarchy")
                .and_then(Value::as_array)
                .ok_or("missing_source_block_hierarchy")?
                .iter()
                .any(|class| class.as_str() == Some("net.minecraft.world.level.block.DiodeBlock"))
            {
                maximum = maximum.max(self.block_signal(
                    neighbor_position,
                    neighbor,
                    direction,
                    true,
                    world,
                    content,
                    entities,
                )?);
            }
        }
        Ok(maximum > 0)
    }
    fn placement_dripstone(
        &self,
        state: u32,
        context: &PlacementContext,
        world: &World,
        content: &Content,
    ) -> Result<Option<u32>> {
        let default_tip = if self.pitch < 0. { 0 } else { 1 };
        let tip =
            if self.placement_dripstone_support(context.position, default_tip, world, content)? {
                default_tip
            } else if self.placement_dripstone_support(
                context.position,
                default_tip ^ 1,
                world,
                content,
            )? {
                default_tip ^ 1
            } else {
                return Ok(None);
            };
        let front = self.use_block(world, relative(context.position, i32::from(tip))?)?;
        let pointed = |neighbor: u32, direction: u8| -> Result<bool> {
            Ok(
                content.name(neighbor) == Some("minecraft:pointed_dripstone")
                    && face(property(content, neighbor, "vertical_direction")?)? == direction,
            )
        };
        let thickness = if pointed(front, tip ^ 1)? {
            if self.last_sent_input & 32 != 0
                && property(content, front, "thickness")? != "tip_merge"
            {
                "tip"
            } else {
                "tip_merge"
            }
        } else if !pointed(front, tip)? {
            "tip"
        } else if matches!(property(content, front, "thickness")?, "tip" | "tip_merge") {
            "frustum"
        } else if pointed(
            self.use_block(world, relative(context.position, i32::from(tip ^ 1))?)?,
            tip,
        )? {
            "middle"
        } else {
            "base"
        };
        Ok(Some(set_bool(
            content,
            set(
                content,
                set(content, state, "vertical_direction", face_name(tip))?,
                "thickness",
                thickness,
            )?,
            "waterlogged",
            water_source(content, self.use_block(world, context.position)?)?,
        )?))
    }
    fn placement_wire(
        &self,
        mut state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let can_connect_up =
            !content.block_redstone_conductor(self.use_block(world, relative(position, 1)?)?)?;
        let mut connected = [false; 4];
        for direction in 2u8..6 {
            let neighbor_position = relative(position, i32::from(direction))?;
            let neighbor = self.use_block(world, neighbor_position)?;
            let mut side = None;
            if can_connect_up {
                let placeable_above = is_owner(content, neighbor, "TrapDoorBlock")?
                    || content.block_supports(neighbor, 1, 0)?
                    || content.name(neighbor) == Some("minecraft:hopper");
                if placeable_above
                    && self.wire_connects_to(
                        self.use_block(world, relative(neighbor_position, 1)?)?,
                        None,
                        content,
                    )?
                {
                    side = Some(if content.block_supports(neighbor, direction ^ 1, 0)? {
                        "up"
                    } else {
                        "side"
                    });
                }
            }
            let side = if let Some(side) = side {
                side
            } else if self.wire_connects_to(neighbor, Some(direction), content)?
                || !content.block_redstone_conductor(neighbor)?
                    && self.wire_connects_to(
                        self.use_block(world, relative(neighbor_position, 0)?)?,
                        None,
                        content,
                    )?
            {
                "side"
            } else {
                "none"
            };
            connected[usize::from(direction - 2)] = side != "none";
            state = set(content, state, face_name(direction), side)?;
        }
        let north_south_empty = !connected[0] && !connected[1];
        let east_west_empty = !connected[2] && !connected[3];
        for direction in 2u8..6 {
            if !connected[usize::from(direction - 2)]
                && if direction < 4 {
                    east_west_empty
                } else {
                    north_south_empty
                }
            {
                state = set(content, state, face_name(direction), "side")?;
            }
        }
        Ok(state)
    }
    fn placement_mossy(
        &self,
        mut state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<u32> {
        let base = boolean(content, state, "bottom")?;
        let mut above = None;
        let mut below = None;
        for direction in 2u8..6 {
            let key = face_name(direction);
            let mut side =
                if self.placement_multiface_support(position, direction, world, content)? {
                    "low"
                } else {
                    "none"
                };
            if side == "low" {
                let top = match above {
                    Some(state) => state,
                    None => {
                        let state = self.use_block(world, relative(position, 1)?)?;
                        above = Some(state);
                        state
                    }
                };
                if content.name(top) == Some("minecraft:pale_moss_carpet")
                    && property(content, top, key)? != "none"
                    && !boolean(content, top, "bottom")?
                {
                    side = "tall";
                }
                if !base {
                    let bottom = match below {
                        Some(state) => state,
                        None => {
                            let state = self.use_block(world, relative(position, 0)?)?;
                            below = Some(state);
                            state
                        }
                    };
                    if content.name(bottom) == Some("minecraft:pale_moss_carpet")
                        && property(content, bottom, key)? == "none"
                    {
                        side = "none";
                    }
                }
            }
            state = set(content, state, key, side)?;
        }
        Ok(state)
    }
}

impl Player {
    fn source_set_placed_by(
        &mut self,
        state: u32,
        position: [i32; 3],
        stack: &Stack,
        world: &World,
        content: &Content,
        _entities: &EntityStore,
        _tick: u64,
    ) -> Result<u32> {
        match short_class(metadata_string(
            content.placement_metadata(state)?,
            "set_placed_by_owner",
        )?) {
            // These original client methods either do nothing or schedule server-only block ticks.
            "Block"
            | "AbstractBedBlock"
            | "SignBlock"
            | "PistonBaseBlock"
            | "CommandBlock"
            | "StructureBlock"
            | "WitherSkullBlock"
            | "WitherWallSkullBlock"
            | "PitcherCropBlock"
            | "SmallDripleafBlock"
            | "MossyCarpetBlock"
            | "DiodeBlock"
            | "CrafterBlock" => {}
            "DoorBlock" => {
                self.predict_block(
                    world,
                    relative(position, 1)?,
                    set(content, state, "half", "upper")?,
                    3,
                )?;
            }
            "DoublePlantBlock" => {
                let above = relative(position, 1)?;
                let mut upper = set(
                    content,
                    content.block_default_state(state)?,
                    "half",
                    "upper",
                )?;
                if content
                    .state_properties(upper)
                    .is_some_and(|p| p.contains_key("waterlogged"))
                {
                    upper = set_bool(
                        content,
                        upper,
                        "waterlogged",
                        water_source(content, self.use_block(world, above)?)?,
                    )?;
                }
                self.predict_block(world, above, upper, 3)?;
            }
            "JukeboxBlock" => {
                if stack
                    .component("minecraft:block_entity_data")
                    .is_some_and(|data| data.get("RecordItem").is_some())
                {
                    self.predict_block(
                        world,
                        position,
                        set_bool(content, state, "has_record", true)?,
                        2,
                    )?;
                }
            }
            "DriedGhastBlock" => {
                let event = if boolean(content, state, "waterlogged")? {
                    "minecraft:block.dried_ghast.place_in_water"
                } else {
                    "minecraft:block.dried_ghast.place"
                };
                self.play_use_level_sound(
                    world,
                    event,
                    crate::audio::SoundCategory::Blocks,
                    position.map(|v| f64::from(v) + 0.5),
                    1.,
                    1.,
                    false,
                )?;
            }
            "TripWireHookBlock" => self.placement_tripwire_hook(state, position, world, content)?,
            owner => return Err(format!("unknown Source setPlacedBy owner {owner}")),
        }
        // BlockItem retains this local state for sound; setPlacedBy does not replace its variable.
        Ok(state)
    }
    fn placement_tripwire_hook(
        &mut self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<()> {
        let facing = face(property(content, state, "facing")?)?;
        let was_attached = boolean(content, state, "attached")?;
        let was_powered = boolean(content, state, "powered")?;
        let mut wires = [None; 42];
        let mut receiver = 0usize;
        let mut attached = true;
        let mut powered = false;
        let step = DIRECTIONS[usize::from(facing)];
        let at = |distance: usize| -> Result<[i32; 3]> {
            let distance = distance as i32;
            Ok([
                position[0]
                    .checked_add(step[0] * distance)
                    .ok_or("tripwire coordinate overflow")?,
                position[1]
                    .checked_add(step[1] * distance)
                    .ok_or("tripwire coordinate overflow")?,
                position[2]
                    .checked_add(step[2] * distance)
                    .ok_or("tripwire coordinate overflow")?,
            ])
        };
        for distance in 1..42 {
            let wire = self.use_block(world, at(distance)?)?;
            if content.name(wire) == Some("minecraft:tripwire_hook") {
                if face(property(content, wire, "facing")?)? == facing ^ 1 {
                    receiver = distance;
                }
                break;
            }
            if content.name(wire) != Some("minecraft:tripwire") {
                attached = false;
            } else {
                powered |=
                    !boolean(content, wire, "disarmed")? && boolean(content, wire, "powered")?;
                wires[distance] = Some(wire);
            }
        }
        attached &= receiver > 1;
        powered &= attached;
        let updated = set_bool(
            content,
            set_bool(
                content,
                content.block_default_state(state)?,
                "attached",
                attached,
            )?,
            "powered",
            powered,
        )?;
        if receiver > 0 {
            self.predict_block(
                world,
                at(receiver)?,
                set(content, updated, "facing", face_name(facing ^ 1))?,
                3,
            )?;
            self.placement_hook_sound(
                at(receiver)?,
                attached,
                powered,
                was_attached,
                was_powered,
                world,
            )?;
        }
        self.placement_hook_sound(
            position,
            attached,
            powered,
            was_attached,
            was_powered,
            world,
        )?;
        self.predict_block(
            world,
            position,
            set(content, updated, "facing", face_name(facing))?,
            3,
        )?;
        if was_attached != attached {
            for (distance, wire) in wires.iter().enumerate().take(receiver).skip(1) {
                if let Some(wire) = wire {
                    let position = at(distance)?;
                    if matches!(
                        content.name(self.use_block(world, position)?),
                        Some("minecraft:tripwire" | "minecraft:tripwire_hook")
                    ) {
                        self.predict_block(
                            world,
                            position,
                            set_bool(content, *wire, "attached", attached)?,
                            3,
                        )?;
                    }
                }
            }
        }
        Ok(())
    }
    fn placement_hook_sound(
        &mut self,
        position: [i32; 3],
        attached: bool,
        powered: bool,
        was_attached: bool,
        was_powered: bool,
        world: &World,
    ) -> Result<()> {
        let effect = if powered && !was_powered {
            Some(("minecraft:block.tripwire.click_on", 0.6))
        } else if !powered && was_powered {
            Some(("minecraft:block.tripwire.click_off", 0.5))
        } else if attached && !was_attached {
            Some(("minecraft:block.tripwire.attach", 0.7))
        } else if !attached && was_attached {
            Some((
                "minecraft:block.tripwire.detach",
                1.2 / (world.next_float()? * 0.2 + 0.9),
            ))
        } else {
            None
        };
        if let Some((event, pitch)) = effect {
            self.play_use_level_sound(
                world,
                event,
                crate::audio::SoundCategory::Blocks,
                position.map(|v| f64::from(v) + 0.5),
                0.4,
                pitch,
                false,
            )?;
        }
        Ok(())
    }
    pub(super) fn source_item_on(
        &mut self,
        owner: &str,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let Some(item) =
            super::super::interaction::held(inventory, hand).filter(|item| !item.empty())
        else {
            return Ok(Outcome::Pass);
        };
        if short_class(owner) == "ArmorStandItem" && hit.face == 0 {
            return Ok(Outcome::Fail);
        }
        let state = self.use_block(world, hit.position)?;
        let owner = source_tool_action(item, content)?.unwrap_or_else(|| short_class(owner));
        match owner {
            "Item" | "LeadItem" | "PlaceOnWaterBlockItem" => Ok(Outcome::Pass),
            "DebugStickItem" | "SpawnEggItem" => Ok(Outcome::CLIENT),
            "ArmorStandItem" => {
                // ArmorStandItem only asks BlockPlaceContext.getClickedPos, not canPlace.
                let (position, _) = self.placement_target(hit, item, world, content)?;
                let [width, height] = content.source_entity_dimensions("minecraft:armor_stand")?;
                let radius = f64::from(width / 2.);
                let center = [
                    f64::from(position[0]) + 0.5,
                    f64::from(position[1]),
                    f64::from(position[2]) + 0.5,
                ];
                let bounds = Bounds {
                    min: [center[0] - radius, center[1], center[2] - radius],
                    max: [
                        center[0] + radius,
                        center[1] + f64::from(height),
                        center[2] + radius,
                    ],
                };
                // CollisionContext.empty(): scaffolding is stable, powder snow empty.
                super::super::collision::gather_with_context(
                    world,
                    content,
                    entities,
                    tick,
                    bounds,
                    Some(super::super::collision::Context {
                        // Empty.isAbove(_, _, true) is true regardless of the cell.
                        bottom: f64::INFINITY,
                        descending: false,
                        fall_distance: 0.,
                        walks_powder_snow: false,
                    }),
                    &mut self.colliders,
                )?;
                if self.colliders.iter().any(|shape| shape.intersects(bounds))
                    || entities.armor_stand_item_blocked(
                        bounds,
                        self.bounds(),
                        self.entity_id,
                        self.game_mode == 3,
                        tick,
                    )?
                {
                    return Ok(Outcome::Fail);
                }
                // Spawning, placement sound and ENTITY_PLACE are server-only.
                let mut remaining = item.clone();
                remaining.count -= 1;
                self.predict_held(
                    hand,
                    (!remaining.empty()).then_some(remaining),
                    inventory,
                    false,
                )?;
                Ok(Outcome::CLIENT)
            }
            "BlockItem" | "SolidBucketItem" => {
                let result =
                    self.place_item(hit, hand, world, content, inventory, entities, tick)?;
                if result.consumes() && short_class(owner) == "SolidBucketItem" {
                    if !self.abilities.instabuild {
                        let bucket = crate::inventory::creative_stack(
                            content,
                            content.item_id("minecraft:bucket")?,
                            1,
                        )?;
                        self.predict_held(hand, Some(bucket), inventory, true)?;
                    }
                }
                if !result.consumes() && item.component("minecraft:consumable").is_some() {
                    return self.generic_item_use(hand, world, content, inventory, entities, tick);
                }
                Ok(result)
            }
            "BrushItem" => {
                if self.brush_hit(world, content, entities, tick)?.is_some() {
                    self.start_using(Some(item), hand, content, inventory, tick)?;
                }
                Ok(Outcome::CONSUME)
            }
            "FireworkRocketItem" => Ok(if self.pose == Pose::FallFlying {
                Outcome::Pass
            } else {
                Outcome::CLIENT
            }),
            "EnderEyeItem" => Ok(
                if content.name(state) == Some("minecraft:end_portal_frame")
                    && !boolean(content, state, "eye")?
                {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                },
            ),
            "MapItem" => Ok(if content.block_has_tag(state, "minecraft:banners")? {
                Outcome::CLIENT
            } else {
                Outcome::Pass
            }),
            "BlockTransformerHoe" => {
                let tillable = match content.name(state) {
                    Some("minecraft:rooted_dirt") => true,
                    Some(
                        "minecraft:grass_block"
                        | "minecraft:dirt_path"
                        | "minecraft:dirt"
                        | "minecraft:coarse_dirt",
                    ) => {
                        hit.face != 0
                            && content
                                .block_is_air(self.use_block(world, relative(hit.position, 1)?)?)?
                    }
                    _ => false,
                };
                if !tillable {
                    return Ok(Outcome::Pass);
                }
                self.play_use_level_sound(
                    world,
                    "minecraft:item.hoe.till",
                    crate::audio::SoundCategory::Blocks,
                    hit.position.map(|v| f64::from(v) + 0.5),
                    1.,
                    1.,
                    true,
                )?;
                Ok(Outcome::CLIENT) // Hoe mutations/durability are ServerLevel-only.
            }
            "BlockTransformerShovel" => self.source_shovel_on(state, hit, world, content),
            "BlockTransformerAxe" => {
                if hand == 0
                    && super::super::interaction::held(inventory, 1)
                        .is_some_and(|s| s.component("minecraft:blocks_attacks").is_some())
                    && self.last_sent_input & 32 == 0
                {
                    return Ok(Outcome::Pass);
                }
                let (updated, sound, event) =
                    if let Some(new) = content.block_stripped_state(state)? {
                        (new, "minecraft:item.axe.strip", None)
                    } else if let Some(new) = content.block_copper_previous_state(state)? {
                        (new, "minecraft:item.axe.scrape", Some(3005))
                    } else if let Some(new) = content.block_unwaxed_state(state)? {
                        (new, "minecraft:item.axe.wax_off", Some(3004))
                    } else {
                        return Ok(Outcome::Pass);
                    };
                self.play_use_level_sound(
                    world,
                    sound,
                    crate::audio::SoundCategory::Blocks,
                    hit.position.map(|v| f64::from(v) + 0.5),
                    1.,
                    1.,
                    true,
                )?;
                if let Some(event) = event {
                    self.source_copper_events(state, hit.position, event, world, content)?;
                }
                self.predict_block(world, hit.position, updated, 11)?;
                Ok(Outcome::CLIENT)
            }
            "HoneycombItem" => {
                let Some(updated) = content.block_waxed_state(state)? else {
                    return Ok(Outcome::Pass);
                };
                self.consume_held(hand, inventory, 1)?;
                self.predict_block(world, hit.position, updated, 11)?;
                self.source_copper_events(state, hit.position, 3003, world, content)?;
                Ok(Outcome::CLIENT)
            }
            "ShearsItem" => {
                if !content.placement_metadata(state)?["hierarchy"]
                    .as_array()
                    .is_some_and(|classes| {
                        classes.iter().any(|c| {
                            c.as_str()
                                == Some("net.minecraft.world.level.block.GrowingPlantHeadBlock")
                        })
                    })
                    || number(content, state, "age")? == 25
                {
                    return Ok(Outcome::Pass);
                }
                self.play_use_level_sound(
                    world,
                    "minecraft:block.growing_plant.crop",
                    crate::audio::SoundCategory::Blocks,
                    hit.position.map(|v| f64::from(v) + 0.5),
                    1.,
                    1.,
                    true,
                )?;
                self.predict_block(world, hit.position, set(content, state, "age", "25")?, 3)?;
                Ok(Outcome::CLIENT)
            }
            "CompassItem" => {
                if content.name(state) != Some("minecraft:lodestone") {
                    return Ok(Outcome::Pass);
                }
                self.play_use_level_sound(
                    world,
                    "minecraft:item.lodestone_compass.lock",
                    crate::audio::SoundCategory::Players,
                    hit.position.map(|v| f64::from(v) + 0.5),
                    1.,
                    1.,
                    false,
                )?;
                let mut components = item.components.as_ref().clone();
                components.insert("minecraft:lodestone_tracker".into(), serde_json::json!({"target":{"dimension":world.dimension_id()?,"pos":hit.position},"tracked":true}));
                if !self.abilities.instabuild && item.count == 1 {
                    let updated =
                        crate::inventory::stack_with_components(content, item, &components)?;
                    self.predict_held(hand, Some(updated), inventory, false)?;
                } else {
                    let compass = crate::inventory::creative_stack_with_components(
                        content,
                        content.item_id("minecraft:compass")?,
                        1,
                        &components,
                    )?;
                    self.consume_held(hand, inventory, 1)?;
                    self.predict_use(UsePrediction::ExtraItem {
                        stack: compass,
                        drop_remainder: true,
                    })?;
                }
                Ok(Outcome::CLIENT)
            }
            "PotionItem" => {
                if hit.face == 0
                    || !content.block_has_tag(state, "minecraft:convertable_to_mud")?
                    || item
                        .component("minecraft:potion_contents")
                        .and_then(|p| p.get("potion"))
                        .and_then(Value::as_str)
                        != Some("minecraft:water")
                    || item
                        .component("minecraft:potion_contents")
                        .and_then(|p| p.get("custom_effects"))
                        .and_then(Value::as_array)
                        .is_some_and(|effects| !effects.is_empty())
                {
                    return Ok(Outcome::Pass);
                }
                let position = hit.position.map(|v| f64::from(v) + 0.5);
                self.play_use_level_sound(
                    world,
                    "minecraft:entity.generic.splash",
                    crate::audio::SoundCategory::Blocks,
                    position,
                    1.,
                    1.,
                    false,
                )?;
                let bottle = crate::inventory::creative_stack(
                    content,
                    content.item_id("minecraft:glass_bottle")?,
                    1,
                )?;
                self.create_filled_result(item, bottle, hand, inventory, true)?;
                self.play_use_level_sound(
                    world,
                    "minecraft:item.bottle.empty",
                    crate::audio::SoundCategory::Blocks,
                    position,
                    1.,
                    1.,
                    false,
                )?;
                self.predict_block(
                    world,
                    hit.position,
                    content.resolve_block_state("minecraft:mud", &BTreeMap::new())?,
                    3,
                )?;
                Ok(Outcome::CLIENT)
            }
            "FlintAndSteelItem" => {
                // FlintAndSteelItem.useOn: light a campfire/candle/candle cake, else
                // place fire on the clicked face when BaseFireBlock.canBePlacedAt.
                let lightable = (content.block_has_tag(state, "minecraft:campfires")?
                    && !boolean(content, state, "waterlogged")?
                    && !boolean(content, state, "lit")?)
                    || (content.block_has_tag(state, "minecraft:candles")?
                        && !boolean(content, state, "waterlogged")?
                        && !boolean(content, state, "lit")?)
                    || (content.block_has_tag(state, "minecraft:candle_cakes")?
                        && !boolean(content, state, "lit")?);
                let target = if lightable {
                    hit.position
                } else {
                    relative(hit.position, i32::from(hit.face))?
                };
                let placed = if lightable {
                    set_bool(content, state, "lit", true)?
                } else {
                    let old = self.use_block(world, target)?;
                    if !content.block_is_air(old)? {
                        return Ok(Outcome::Fail);
                    }
                    let fire = self.placement_fire(old, "BaseFireBlock", target, world, content)?;
                    let below = self.use_block(world, relative(target, 0)?)?;
                    let survives =
                        if content.block_has_tag(below, "minecraft:soul_fire_base_blocks")? {
                            true
                        } else {
                            let mut flammable_neighbor = false;
                            for direction in 0u8..6 {
                                flammable_neighbor |= content.block_can_burn(
                                    self.use_block(world, relative(target, i32::from(direction))?)?,
                                )?;
                            }
                            content.block_supports(below, 1, 0)? || flammable_neighbor
                        };
                    // Portal ignition is server-side; the packet is sent regardless.
                    if !survives {
                        return Ok(Outcome::Fail);
                    }
                    fire
                };
                self.play_use_level_sound(
                    world,
                    "minecraft:item.flintandsteel.use",
                    crate::audio::SoundCategory::Blocks,
                    target.map(|v| f64::from(v) + 0.5),
                    1.,
                    1.,
                    true,
                )?;
                self.predict_block(world, target, placed, 11)?;
                Ok(Outcome::CLIENT)
            }
            owner => Err(format!("unknown Source item-on owner {owner}")),
        }
    }
    fn source_shovel_on(
        &mut self,
        state: u32,
        hit: &BlockHit,
        world: &World,
        content: &Content,
    ) -> Result<Outcome> {
        if hit.face == 0 {
            return Ok(Outcome::Pass);
        }
        let flattenable = matches!(
            content.name(state),
            Some(
                "minecraft:grass_block"
                    | "minecraft:dirt"
                    | "minecraft:podzol"
                    | "minecraft:coarse_dirt"
                    | "minecraft:mycelium"
                    | "minecraft:rooted_dirt"
            )
        );
        if flattenable
            && content.block_is_air(self.use_block(world, relative(hit.position, 1)?)?)?
        {
            self.play_use_level_sound(
                world,
                "minecraft:item.shovel.flatten",
                crate::audio::SoundCategory::Blocks,
                hit.position.map(|v| f64::from(v) + 0.5),
                1.,
                1.,
                true,
            )?;
            return Ok(Outcome::CLIENT);
        }
        if content.placement_metadata(state)?["hierarchy"]
            .as_array()
            .is_some_and(|classes| {
                classes
                    .iter()
                    .any(|c| c.as_str() == Some("net.minecraft.world.level.block.CampfireBlock"))
            })
            && boolean(content, state, "lit")?
        {
            self.campfire_dowse(state, hit.position, world, content)?;
            return Ok(Outcome::CLIENT);
        }
        Ok(Outcome::Pass)
    }
    fn source_copper_events(
        &mut self,
        state: u32,
        position: [i32; 3],
        event: i32,
        world: &World,
        content: &Content,
    ) -> Result<()> {
        self.predict_use(UsePrediction::LevelEvent {
            event,
            position,
            data: 0,
        })?;
        if content.placement_metadata(state)?["hierarchy"]
            .as_array()
            .is_some_and(|classes| {
                classes
                    .iter()
                    .any(|c| c.as_str() == Some("net.minecraft.world.level.block.ChestBlock"))
            })
            && property(content, state, "type")? != "single"
        {
            let facing = face(property(content, state, "facing")?)?;
            let neighbor = relative(
                position,
                i32::from(if property(content, state, "type")? == "left" {
                    clockwise(facing)
                } else {
                    clockwise(facing) ^ 1
                }),
            )?;
            self.use_block(world, neighbor)?;
            self.predict_use(UsePrediction::LevelEvent {
                event,
                position: neighbor,
                data: 0,
            })?;
        }
        Ok(())
    }
}

impl Player {
    pub(super) fn candle_extinguish(
        &mut self,
        state: u32,
        position: [i32; 3],
        _except_local: bool,
        world: &World,
        content: &Content,
    ) -> Result<()> {
        self.predict_block(world, position, set_bool(content, state, "lit", false)?, 11)?;
        let offsets = content.candle_particle_offsets(state)?;
        if offsets.len() > 16 {
            return Err("Source candle particle offset bound".into());
        }
        let mut samples = [UseParticleSample::default(); 16];
        for (sample, offset) in samples.iter_mut().zip(offsets) {
            sample.position = std::array::from_fn(|i| f64::from(position[i]) + offset[i]);
            sample.velocity = [0., 0.10000000149011612, 0.];
        }
        self.predict_use(UsePrediction::Particles {
            kind: UseParticle::Smoke,
            samples,
            count: offsets.len() as u8,
        })?;
        self.play_use_level_sound(
            world,
            "minecraft:block.candle.extinguish",
            crate::audio::SoundCategory::Blocks,
            position.map(|v| f64::from(v) + 0.5),
            1.,
            1.,
            false,
        )
    }
    pub(super) fn campfire_dowse(
        &mut self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<()> {
        let signal = boolean(content, state, "signal_fire")?;
        for _ in 0..20 {
            let mut samples = [UseParticleSample::default(); 16];
            samples[0].position = [
                f64::from(position[0])
                    + 0.5
                    + world.next_double()? / 3.
                        * if world.next_int_bound(2)? != 0 {
                            1.
                        } else {
                            -1.
                        },
                f64::from(position[1]) + world.next_double()? + world.next_double()?,
                f64::from(position[2])
                    + 0.5
                    + world.next_double()? / 3.
                        * if world.next_int_bound(2)? != 0 {
                            1.
                        } else {
                            -1.
                        },
            ];
            samples[0].velocity = [0., 0.07, 0.];
            self.predict_use(UsePrediction::Particles {
                kind: if signal {
                    UseParticle::CampfireSignalSmoke
                } else {
                    UseParticle::CampfireCosySmoke
                },
                samples,
                count: 1,
            })?;
            samples[0].position = [
                f64::from(position[0])
                    + 0.5
                    + world.next_double()? / 4.
                        * if world.next_int_bound(2)? != 0 {
                            1.
                        } else {
                            -1.
                        },
                f64::from(position[1]) + 0.4,
                f64::from(position[2])
                    + 0.5
                    + world.next_double()? / 4.
                        * if world.next_int_bound(2)? != 0 {
                            1.
                        } else {
                            -1.
                        },
            ];
            samples[0].velocity = [0., 0.005, 0.];
            self.predict_use(UsePrediction::Particles {
                kind: UseParticle::Smoke,
                samples,
                count: 1,
            })?;
        }
        Ok(())
    }
}
