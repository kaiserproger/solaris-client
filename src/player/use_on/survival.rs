use super::*;

pub(super) const FACE_NAMES: [&str; 6] = ["down", "up", "north", "south", "west", "east"];
pub(super) fn face_name(face: u8) -> &'static str {
    FACE_NAMES[usize::from(face)]
}
pub(super) fn face(value: &str) -> Result<u8> {
    Ok(facing_face(value)? as u8)
}
pub(super) fn clockwise(face: u8) -> u8 {
    match face {
        2 => 5,
        5 => 3,
        3 => 4,
        4 => 2,
        _ => face,
    }
}
pub(super) fn same_block(content: &Content, left: u32, right: u32) -> Result<bool> {
    Ok(content.info(left)?.block == content.info(right)?.block)
}
pub(super) fn water(content: &Content, state: u32) -> Result<bool> {
    Ok(content
        .info(state)?
        .fluid
        .is_some_and(|fluid| fluid.kind == crate::protocol::content::FluidKind::Water))
}
pub(super) fn water_source(content: &Content, state: u32) -> Result<bool> {
    Ok(content.info(state)?.fluid.is_some_and(|fluid| {
        fluid.kind == crate::protocol::content::FluidKind::Water
            && fluid.amount == 8
            && !fluid.falling
    }))
}
pub(super) fn full_water(content: &Content, state: u32) -> Result<bool> {
    Ok(content.info(state)?.fluid.is_some_and(|fluid| {
        fluid.kind == crate::protocol::content::FluidKind::Water && fluid.amount == 8
    }))
}
pub(super) fn fluid_tag(content: &Content, state: u32, tag: &str) -> Result<bool> {
    content.registry_tag_contains(
        "minecraft:fluid",
        tag,
        content.block_fluid_holder(state)? as i32,
    )
}
pub(super) fn metadata_string<'a>(metadata: &'a Value, key: &str) -> Result<&'a str> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing Source placement metadata {key}"))
}
pub(super) fn metadata_state(metadata: &Value, key: &str) -> Result<u32> {
    metadata
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| format!("missing Source placement state {key}"))
}

impl Player {
    pub(super) fn placement_supports(
        &self,
        position: [i32; 3],
        toward: u8,
        support: u8,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let neighbor_position = relative(position, i32::from(toward))?;
        let neighbor = self.use_block(world, neighbor_position)?;
        content.block_supports_at(neighbor, neighbor_position, toward ^ 1, support)
    }
    pub(super) fn placement_center(
        &self,
        position: [i32; 3],
        toward: u8,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let neighbor = self.use_block(world, relative(position, i32::from(toward))?)?;
        if toward == 1 && content.block_has_tag(neighbor, "minecraft:unstable_bottom_center")? {
            return Ok(false);
        }
        content.block_supports_at(
            neighbor,
            relative(position, i32::from(toward))?,
            toward ^ 1,
            1,
        )
    }
    pub(super) fn placement_multiface_support(
        &self,
        position: [i32; 3],
        toward: u8,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let neighbor = self.use_block(world, relative(position, i32::from(toward))?)?;
        Ok(content.block_support_face_full(neighbor, toward ^ 1)?
            || content.block_collision_face_full(neighbor, toward ^ 1)?)
    }
    pub(super) fn placement_vine_support(
        &self,
        position: [i32; 3],
        toward: u8,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        if toward == 0 {
            return Ok(false);
        }
        if self.placement_multiface_support(position, toward, world, content)? {
            return Ok(true);
        }
        if toward == 1 {
            return Ok(false);
        }
        let above = self.use_block(world, relative(position, 1)?)?;
        Ok(content.name(above) == Some("minecraft:vine")
            && boolean(content, above, face_name(toward))?)
    }
    pub(super) fn placement_scaffolding_distance(
        &self,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<i32> {
        let below = self.use_block(world, relative(position, 0)?)?;
        let mut distance = if content.name(below) == Some("minecraft:scaffolding") {
            number(content, below, "distance")?
        } else {
            if content.block_supports(below, 1, 0)? {
                return Ok(0);
            }
            7
        };
        for toward in [2, 3, 4, 5] {
            let neighbor = self.use_block(world, relative(position, toward)?)?;
            if content.name(neighbor) == Some("minecraft:scaffolding") {
                distance = distance.min(number(content, neighbor, "distance")? + 1);
                if distance == 1 {
                    break;
                }
            }
        }
        Ok(distance)
    }
    pub(super) fn placement_dripstone_support(
        &self,
        position: [i32; 3],
        tip: u8,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let behind = self.use_block(world, relative(position, i32::from(tip ^ 1))?)?;
        Ok(content.block_supports(behind, tip, 0)?
            || content.name(behind) == Some("minecraft:pointed_dripstone")
                && face(property(content, behind, "vertical_direction")?)? == tip)
    }
    pub(super) fn placement_wall_hanging_support(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let facing = face(property(content, state, "facing")?)?;
        for toward in [clockwise(facing), clockwise(facing) ^ 1] {
            let neighbor = self.use_block(world, relative(position, i32::from(toward))?)?;
            let attaches = if content.block_has_tag(neighbor, "minecraft:wall_hanging_signs")? {
                face_axis(i32::from(face(property(content, neighbor, "facing")?)?))
                    == face_axis(i32::from(facing))
            } else {
                content.block_supports(neighbor, toward ^ 1, 0)?
            };
            if attaches {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn placement_vegetation_support(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let below = self.use_block(world, relative(position, 0)?)?;
        let metadata = content.placement_metadata(state)?;
        let owner = short_class(metadata_string(metadata, "may_place_on_owner")?);
        Ok(match owner {
            "VegetationBlock" => content.block_has_tag(below, "minecraft:supports_vegetation")?,
            "CropBlock" => content.block_has_tag(below, "minecraft:supports_crops")?,
            "PitcherCropBlock" => content.block_has_tag(below, "minecraft:supports_crops")?,
            "DryVegetationBlock" => {
                content.block_has_tag(below, "minecraft:supports_dry_vegetation")?
            }
            "NetherWartBlock" => content.block_has_tag(below, "minecraft:supports_nether_wart")?,
            "NetherSproutsBlock" => {
                content.block_has_tag(below, "minecraft:supports_nether_sprouts")?
            }
            "AzaleaBlock" => content.block_has_tag(below, "minecraft:supports_azalea")?,
            "WitherRoseBlock" => content.block_has_tag(below, "minecraft:supports_wither_rose")?,
            "MangrovePropaguleBlock" => {
                content.block_has_tag(below, "minecraft:supports_mangrove_propagule")?
            }
            "NetherFungusBlock" | "NetherRootsBlock" | "AttachedStemBlock" | "StemBlock" => {
                content.block_has_tag(below, metadata_string(metadata, "support_tag")?)?
            }
            "SeagrassBlock" | "TallSeagrassBlock" => {
                content.block_supports(below, 1, 0)?
                    && !content.block_has_tag(below, "minecraft:cannot_support_seagrass")?
            }
            "CactusFlowerBlock" => {
                content.block_has_tag(below, "minecraft:support_override_cactus_flower")?
                    || content.block_supports(below, 1, 1)?
            }
            "LilyPadBlock" => {
                (fluid_tag(content, below, "minecraft:supports_lily_pad")?
                    || content.block_has_tag(below, "minecraft:supports_lily_pad")?)
                    && content
                        .info(self.use_block(world, position)?)?
                        .fluid
                        .is_none()
            }
            "SmallDripleafBlock" => {
                content.block_has_tag(below, "minecraft:supports_small_dripleaf")?
                    || water_source(content, self.use_block(world, position)?)?
                        && content.block_has_tag(below, "minecraft:supports_vegetation")?
            }
            "MushroomBlock" => content.info(below)?.solid_render,
            owner => return Err(format!("unknown Source mayPlaceOn owner {owner}")),
        })
    }
    /// Source canSurvive dispatch. Unknown/unloaded neighbors are errors, not air.
    pub(super) fn can_survive(
        &self,
        state: u32,
        position: [i32; 3],
        world: &World,
        content: &Content,
        _entities: &EntityStore,
        _tick: u64,
    ) -> Result<bool> {
        let owner = short_class(content.block_placement_owners(state)?.2);
        let read = |toward| self.use_block(world, relative(position, toward)?);
        let below = || read(0);
        let above = || read(1);
        let tag_below = |tag| content.block_has_tag(below()?, tag);
        let solid_below = || content.block_is_solid(below()?);
        let face_support =
            |toward, support| self.placement_supports(position, toward, support, world, content);
        let attached = || -> Result<u8> {
            Ok(match property(content, state, "face")? {
                "floor" => 0,
                "ceiling" => 1,
                "wall" => face(property(content, state, "facing")?)? ^ 1,
                _ => return Err("invalid Source attached face".into()),
            })
        };
        Ok(match owner {
            "BlockBehaviour" | "GrindstoneBlock" => true,
            "AmethystClusterBlock"
            | "BaseCoralWallFanBlock"
            | "LadderBlock"
            | "WallTorchBlock"
            | "RedstoneWallTorchBlock"
            | "TripWireHookBlock" => {
                let facing = face(property(content, state, "facing")?)?;
                (owner != "TripWireHookBlock" || facing >= 2) && face_support(facing ^ 1, 0)?
            }
            "BannerBlock" | "CakeBlock" | "CandleCakeBlock" | "StandingSignBlock" => solid_below()?,
            "WallBannerBlock" | "WallSignBlock" => content.block_is_solid(read(i32::from(
                face(property(content, state, "facing")?)? ^ 1,
            ))?)?,
            "BaseCoralPlantTypeBlock" | "LeafLitterBlock" => face_support(0, 0)?,
            "BaseRailBlock" | "DiodeBlock" => face_support(0, 2)?,
            "BasePressurePlateBlock" => {
                face_support(0, 2)? || self.placement_center(position, 0, world, content)?
            }
            "BaseTorchBlock" | "CandleBlock" => {
                self.placement_center(position, 0, world, content)?
            }
            "CeilingHangingSignBlock" => face_support(1, 1)?,
            "HangingRootsBlock" => face_support(1, 0)?,
            "LanternBlock" => self.placement_center(
                position,
                if boolean(content, state, "hanging")? {
                    1
                } else {
                    0
                },
                world,
                content,
            )?,
            "FaceAttachedHorizontalDirectionalBlock" => face_support(attached()?, 0)?,
            "BellBlock" => {
                let toward = match property(content, state, "attachment")? {
                    "floor" => 0,
                    "ceiling" => 1,
                    "single_wall" | "double_wall" => face(property(content, state, "facing")?)?,
                    _ => return Err("invalid Source bell attachment".into()),
                };
                if toward == 1 {
                    self.placement_center(position, 1, world, content)?
                } else {
                    face_support(toward, 0)?
                }
            }
            "BambooSaplingBlock" | "BambooStalkBlock" => tag_below("minecraft:supports_bamboo")?,
            "BigDripleafBlock" => {
                same_block(content, below()?, state)?
                    || content.name(below()?) == Some("minecraft:big_dripleaf_stem")
                    || tag_below("minecraft:supports_big_dripleaf")?
            }
            "BigDripleafStemBlock" => {
                (same_block(content, below()?, state)?
                    || tag_below("minecraft:supports_big_dripleaf")?)
                    && (same_block(content, above()?, state)?
                        || content.name(above()?) == Some("minecraft:big_dripleaf"))
            }
            "BubbleColumnBlock" => {
                same_block(content, below()?, state)?
                    || tag_below("minecraft:enables_bubble_column_push_up")?
                    || tag_below("minecraft:enables_bubble_column_drag_down")?
            }
            "CactusBlock" => {
                for toward in [2, 3, 4, 5] {
                    let neighbor = read(toward)?;
                    if content.block_is_solid(neighbor)?
                        || fluid_tag(content, neighbor, "minecraft:lava")?
                    {
                        return Ok(false);
                    }
                }
                (same_block(content, below()?, state)? || tag_below("minecraft:supports_cactus")?)
                    && !content.block_liquid(above()?)?
            }
            "CarpetBlock" => !content.block_is_air(below()?)?,
            "MossyCarpetBlock" => {
                if boolean(content, state, "bottom")? {
                    !content.block_is_air(below()?)?
                } else {
                    same_block(content, below()?, state)? && boolean(content, below()?, "bottom")?
                }
            }
            "ChorusFlowerBlock" => {
                if content.name(below()?) == Some("minecraft:chorus_plant")
                    || tag_below("minecraft:supports_chorus_flower")?
                {
                    true
                } else if !content.block_is_air(below()?)? {
                    false
                } else {
                    let mut one = false;
                    for toward in [2, 3, 4, 5] {
                        let neighbor = read(toward)?;
                        if content.name(neighbor) == Some("minecraft:chorus_plant") {
                            if one {
                                return Ok(false);
                            }
                            one = true;
                        } else if !content.block_is_air(neighbor)? {
                            return Ok(false);
                        }
                    }
                    one
                }
            }
            "ChorusPlantBlock" => {
                let blocked = !content.block_is_air(above()?)? && !content.block_is_air(below()?)?;
                for toward in [2, 3, 4, 5] {
                    let neighbor_pos = relative(position, toward)?;
                    let neighbor = self.use_block(world, neighbor_pos)?;
                    if same_block(content, neighbor, state)? {
                        if blocked {
                            return Ok(false);
                        }
                        let support = self.use_block(world, relative(neighbor_pos, 0)?)?;
                        if same_block(content, support, state)?
                            || content.block_has_tag(support, "minecraft:supports_chorus_plant")?
                        {
                            return Ok(true);
                        }
                    }
                }
                same_block(content, below()?, state)?
                    || tag_below("minecraft:supports_chorus_plant")?
            }
            "CocoaBlock" => content.block_has_tag(
                read(i32::from(face(property(content, state, "facing")?)?))?,
                "minecraft:supports_cocoa",
            )?,
            "CropBlock" => {
                world.source_raw_brightness(position)? >= 8
                    && self.placement_vegetation_support(state, position, world, content)?
            }
            "DirtPathBlock" => {
                !content.block_is_solid(above()?)?
                    || short_class(content.block_placement_owners(above()?)?.0) == "FenceGateBlock"
            }
            "FarmlandBlock" => {
                !content.block_is_solid(above()?)?
                    || content.block_has_tag(above()?, "minecraft:maintains_farmland")?
            }
            "DoorBlock" => {
                if property(content, state, "half")? == "lower" {
                    face_support(0, 0)?
                } else {
                    same_block(content, below()?, state)?
                }
            }
            "DoublePlantBlock" | "PitcherCropBlock" | "SmallDripleafBlock"
            | "TallSeagrassBlock" => {
                if property(content, state, "half")? == "upper" {
                    same_block(content, below()?, state)?
                        && property(content, below()?, "half")? == "lower"
                } else {
                    (owner != "PitcherCropBlock" || world.source_raw_brightness(position)? >= 8)
                        && self.placement_vegetation_support(state, position, world, content)?
                        && (owner != "TallSeagrassBlock"
                            || full_water(content, self.use_block(world, position)?)?)
                }
            }
            "FireBlock" => {
                if face_support(0, 0)? {
                    true
                } else {
                    let mut burns = false;
                    for toward in 0..6 {
                        if content.block_can_burn(read(toward)?)? {
                            burns = true;
                            break;
                        }
                    }
                    burns
                }
            }
            "SoulFireBlock" => tag_below("minecraft:soul_fire_base_blocks")?,
            "FrogspawnBlock" => {
                (fluid_tag(content, below()?, "minecraft:supports_frogspawn")?
                    || tag_below("minecraft:supports_frogspawn")?)
                    && content
                        .info(self.use_block(world, position)?)?
                        .fluid
                        .is_none()
            }
            "GrowingPlantBlock" => {
                let metadata = content.placement_metadata(state)?;
                let growth = face(metadata_string(metadata, "growth_direction")?)?;
                let support = read(i32::from(growth ^ 1))?;
                let head = metadata_state(metadata, "head_state")?;
                let body = metadata_state(metadata, "body_state")?;
                let kelp = content.name(head) == Some("minecraft:kelp");
                (!kelp || !content.block_has_tag(support, "minecraft:cannot_support_kelp")?)
                    && (same_block(content, support, head)?
                        || same_block(content, support, body)?
                        || content.block_supports(support, growth, 0)?)
            }
            "HangingMossBlock" => {
                same_block(content, above()?, state)?
                    || self.placement_multiface_support(position, 1, world, content)?
            }
            "MangrovePropaguleBlock" => {
                if boolean(content, state, "hanging")? {
                    content
                        .block_has_tag(above()?, "minecraft:supports_hanging_mangrove_propagule")?
                } else {
                    self.placement_vegetation_support(state, position, world, content)?
                }
            }
            "MultifaceBlock" => {
                let mut any = false;
                for toward in 0..6 {
                    if content
                        .state_properties(state)
                        .is_some_and(|properties| properties.contains_key(face_name(toward)))
                        && boolean(content, state, face_name(toward))?
                    {
                        if !self.placement_multiface_support(position, toward, world, content)? {
                            return Ok(false);
                        }
                        any = true;
                    }
                }
                any
            }
            "MushroomBlock" => {
                tag_below("minecraft:overrides_mushroom_light_requirement")?
                    || world.source_raw_brightness(position)? < 13
                        && content.info(below()?)?.solid_render
            }
            "PointedDripstoneBlock" => self.placement_dripstone_support(
                position,
                face(property(content, state, "vertical_direction")?)?,
                world,
                content,
            )?,
            "RedStoneWireBlock" => {
                face_support(0, 0)? || content.name(below()?) == Some("minecraft:hopper")
            }
            "ScaffoldingBlock" => {
                self.placement_scaffolding_distance(position, world, content)? < 7
            }
            "SeaPickleBlock" => {
                let support = below()?;
                content.block_supports(support, 1, 0)?
                    || content
                        .collision_boxes(support)
                        .ok_or("unknown_survival_collision")?
                        .iter()
                        .any(|bounds| {
                            bounds[4] >= 1.
                                && bounds[1] < 1.
                                && bounds[3] > bounds[0]
                                && bounds[5] > bounds[2]
                        })
            }
            "SnowLayerBlock" => {
                !tag_below("minecraft:cannot_support_snow_layer")?
                    && (tag_below("minecraft:support_override_snow_layer")?
                        || content.block_collision_face_full(below()?, 1)?
                        || same_block(content, below()?, state)?
                            && number(content, below()?, "layers")? == 8)
            }
            "SporeBlossomBlock" => {
                self.placement_center(position, 1, world, content)?
                    && !water(content, self.use_block(world, position)?)?
            }
            "SugarCaneBlock" => {
                if same_block(content, below()?, state)? {
                    true
                } else if !tag_below("minecraft:supports_sugar_cane")? {
                    false
                } else {
                    let below_pos = relative(position, 0)?;
                    let mut supports = false;
                    for toward in [2, 3, 4, 5] {
                        let neighbor = self.use_block(world, relative(below_pos, toward)?)?;
                        if fluid_tag(
                            content,
                            neighbor,
                            "minecraft:supports_sugar_cane_adjacently",
                        )? || content
                            .block_has_tag(neighbor, "minecraft:supports_sugar_cane_adjacently")?
                        {
                            supports = true;
                            break;
                        }
                    }
                    supports
                }
            }
            "VegetationBlock" => {
                self.placement_vegetation_support(state, position, world, content)?
            }
            "VineBlock" => {
                let mut any = false;
                for toward in 1..6 {
                    if boolean(content, state, face_name(toward))? {
                        let supported = if toward == 1 {
                            let above_state = above()?;
                            content.block_support_face_full(above_state, 1)?
                                || content.block_collision_face_full(above_state, 1)?
                        } else {
                            self.placement_vine_support(position, toward, world, content)?
                        };
                        if supported {
                            any = true;
                            break;
                        }
                    }
                }
                any
            }
            "PistonHeadBlock" => {
                let facing = face(property(content, state, "facing")?)?;
                let base = read(i32::from(facing ^ 1))?;
                let base_name = content.name(base).ok_or("unknown_piston_base")?;
                let expected = if property(content, state, "type")? == "normal" {
                    "minecraft:piston"
                } else {
                    "minecraft:sticky_piston"
                };
                (base_name == expected && boolean(content, base, "extended")?
                    || base_name == "minecraft:moving_piston")
                    && property(content, base, "facing")? == property(content, state, "facing")?
            }
            owner => return Err(format!("unknown Source survival owner {owner}")),
        })
    }
}
