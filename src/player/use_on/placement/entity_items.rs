use super::*;

fn hanging_bounds(position: [i32; 3], face: i32, width: i32, height: i32, frame: bool) -> Bounds {
    let step = match face {
        0 => [0., -1., 0.], 1 => [0., 1., 0.], 2 => [0., 0., -1.],
        3 => [0., 0., 1.], 4 => [-1., 0., 0.], _ => [1., 0., 0.],
    };
    let mut center = std::array::from_fn::<_, 3, _>(|i| f64::from(position[i]) + 0.5 - step[i] * 0.46875);
    if !frame {
        let offset = if width % 2 == 0 { 0.5 } else { 0. };
        let left = match face { 2 => [-1., 0., 0.], 3 => [1., 0., 0.], 4 => [0., 0., 1.], _ => [0., 0., -1.] };
        for i in 0..3 { center[i] += left[i] * offset; }
        if height % 2 == 0 { center[1] += 0.5; }
    }
    let w = if frame { 0.75 } else { f64::from(width) };
    let h = if frame { 0.75 } else { f64::from(height) };
    let size = [if step[0] != 0. { 0.0625 } else { w },
        if step[1] != 0. { 0.0625 } else { h },
        if step[2] != 0. { 0.0625 } else { w }];
    Bounds { min: std::array::from_fn(|i| center[i] - size[i] / 2.),
        max: std::array::from_fn(|i| center[i] + size[i] / 2.) }
}

fn painting_variant<'a>(content: &'a Content, entities: &'a EntityStore, name: &str) -> Result<&'a Value> {
    let (key, observed) = content.registry_entries("minecraft:painting_variant")?.iter()
        .find(|(key, _)| key == name).ok_or("painting variant unavailable")?;
    if observed.is_null() {
        entities.models_ref().ok_or("entity models not activated")?.builtin_variants
            .get("minecraft:painting_variant").and_then(|entries| entries.get(key))
            .ok_or_else(|| "painting source variant unavailable".into())
    } else { Ok(observed) }
}

fn painting_dimensions(content: &Content, entities: &EntityStore, variant: &Value) -> Result<[i32; 2]> {
    let variant = if let Some(name) = variant.as_str() {
        painting_variant(content, entities, name)?
    } else { variant };
    let width = variant["width"].as_i64().ok_or("painting width unavailable")?;
    let height = variant["height"].as_i64().ok_or("painting height unavailable")?;
    if !(1..=16).contains(&width) || !(1..=16).contains(&height) {
        return Err("invalid painting dimensions".into());
    }
    Ok([width as i32, height as i32])
}

fn received_hanging_face(entity: &crate::entities::Entity, entities: &EntityStore) -> Result<i32> {
    let face = entities.metadata(entity.handle, "DATA_DIRECTION")?
        .and_then(crate::entities::Metadata::int).unwrap_or(entity.spawn_data);
    if !(0..=5).contains(&face) {
        return Err("hanging direction invalid".into());
    }
    Ok(face)
}

fn received_hanging_bounds(entity: &crate::entities::Entity, content: &Content,
    entities: &EntityStore, tick: u64) -> Result<Bounds> {
    let face = received_hanging_face(entity, entities)?;
    let frame = entity.type_name == "minecraft:item_frame" || entity.type_name == "minecraft:glow_item_frame";
    let [width, height] = if frame { [1, 1] } else if entity.type_name == "minecraft:painting" {
        match entities.metadata(entity.handle, "DATA_PAINTING_VARIANT_ID")? {
            Some(crate::entities::Metadata::InlinePaintingVariant(value)) => painting_dimensions(content, entities, value)?,
            value => {
                let id = value.and_then(|v| v.int()).unwrap_or(0);
                let name = content.registry_name("minecraft:painting_variant", id)?;
                painting_dimensions(content, entities, painting_variant(content, entities, name)?)?
            }
        }
    } else {
        return entities.entity_bounds(entity.handle, tick);
    };
    // Hanging add-entity packets carry the BlockPos anchor, not the pop-box center.
    let anchor = entity.position.map(|coordinate| coordinate.floor() as i32);
    Ok(hanging_bounds(anchor, face, width, height, frame))
}

// 26.3 NewMinecartBehavior.adjustToRails projects the spawn point onto
// the half-block exit segment and adds 0.1 to its vertical coordinate.
fn experimental_cart_bounds(position: [i32; 3], shape: Option<&str>, dimensions: [f32; 2]) -> Bounds {
    let [mut x, y, mut z] = position.map(f64::from);
    x += 0.5;
    z += 0.5;
    match shape {
        Some("south_east") => { x += 0.25; z += 0.25; }
        Some("south_west") => { x -= 0.25; z += 0.25; }
        Some("north_east") => { x += 0.25; z -= 0.25; }
        Some("north_west") => { x -= 0.25; z -= 0.25; }
        _ => {}
    }
    let bottom = y + match shape {
        None => 0.0625,
        Some(s) if s.starts_with("ascending_") => 0.6,
        Some(_) => 0.1,
    };
    let radius = f64::from(dimensions[0] / 2.);
    Bounds { min: [x - radius, bottom, z - radius],
        max: [x + radius, bottom + f64::from(dimensions[1]), z + radius] }
}

fn intersects_entity_class(entities: &EntityStore, area: Bounds, class: &str, tick: u64) -> Result<bool> {
    for entity in entities.candidates(area) {
        let models = entities.models_ref().ok_or("entity models not activated")?;
        let schema = models.schemas.get(&entity.type_name).ok_or("entity schema unavailable")?;
        if schema.hierarchy.iter().any(|name| name == class)
            && entities.entity_bounds(entity.handle, tick)?.intersects(area) {
            return Ok(true);
        }
    }
    Ok(false)
}

impl Player {
    #[expect(clippy::too_many_arguments, reason = "matches the existing item-on dispatch contract")]
    fn hanging_survives(&mut self, area: Bounds, face: i32, frame: bool,
        world: &World, content: &Content, entities: &EntityStore, tick: u64) -> Result<bool> {
        super::super::super::collision::gather(world, content, entities, tick, area, &mut self.colliders)?;
        if self.colliders.iter().any(|shape| shape.intersects(area))
            || world.border_collides(std::array::from_fn(|i| (area.min[i] + area.max[i]) / 2.), area)? {
            return Ok(false);
        }
        let step = match face { 0 => [0., -1., 0.], 1 => [0., 1., 0.], 2 => [0., 0., -1.],
            3 => [0., 0., 1.], 4 => [-1., 0., 0.], _ => [1., 0., 0.] };
        let support = area.moved(step.map(|v| -0.5 * v)).deflated(1e-7);
        let lo = support.min.map(|v| v.floor() as i32);
        let hi = support.max.map(|v| v.floor() as i32);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let state = self.use_block(world, [x, y, z])?;
                    let diode = matches!(content.name(state), Some("minecraft:repeater" | "minecraft:comparator"));
                    if !(content.block_is_solid(state)? || diode && (!frame || face >= 2)) {
                        return Ok(false);
                    }
                }
            }
        }
        // The spatial index also retains packet anchors. Include the furthest
        // anchor of a supported 16-block painting before testing exact pop boxes.
        let query = Bounds {
            min: area.min.map(|coordinate| coordinate - 9.),
            max: area.max.map(|coordinate| coordinate + 9.),
        };
        for entity in entities.candidates(query) {
            let models = entities.models_ref().ok_or("entity models not activated")?;
            let schema = models.schemas.get(&entity.type_name).ok_or("entity schema unavailable")?;
            if schema.hierarchy.iter().any(|name| name == "net.minecraft.world.entity.decoration.HangingEntity")
                && ((!frame && entity.type_name == "minecraft:painting")
                    || received_hanging_face(entity, entities)? == face)
                && received_hanging_bounds(entity, content, entities, tick)?.intersects(area) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    #[expect(clippy::too_many_arguments, reason = "matches the existing item-on dispatch contract")]
    pub(super) fn hanging_item_on(&mut self, hit: &BlockHit, hand: u8, world: &World,
        content: &Content, inventory: &InventoryStore, entities: &EntityStore, tick: u64) -> Result<Outcome> {
        let Some(item) = super::super::super::interaction::held(inventory, hand).filter(|item| !item.empty()) else {
            return Ok(Outcome::Pass);
        };
        let name = content.item_names.get(&item.id).ok_or("unknown hanging item")?;
        let frame = matches!(name.as_str(), "minecraft:item_frame" | "minecraft:glow_item_frame");
        let position = relative(hit.position, hit.face)?;
        if (!frame && hit.face < 2) || (frame && (position[1] < world.min_y || position[1] >= world.min_y + world.height))
            || (!self.abilities.may_build && !super::super::super::adventure::matches(
                Some(item), "minecraft:can_place_on", hit, world, entities, content)?) {
            return Ok(Outcome::Fail);
        }
        let survives = if frame {
            self.hanging_survives(hanging_bounds(position, hit.face, 1, 1, true), hit.face, true, world, content, entities, tick)?
        } else {
            let variants = content.registry_entries("minecraft:painting_variant")?;
            let mut found = false;
            for id in content.registry_tag_optional("minecraft:painting_variant", "minecraft:placeable")?.unwrap_or(&[]) {
                let (variant_name, _) = variants.get(usize::try_from(*id).map_err(|_| "negative painting variant")?).ok_or("painting variant unavailable")?;
                let [width, height] = painting_dimensions(content, entities, painting_variant(content, entities, variant_name)?)?;
                if self.hanging_survives(hanging_bounds(position, hit.face, width, height, false), hit.face, false, world, content, entities, tick)? {
                    found = true;
                    break;
                }
            }
            if found && let Some(variant) = item.component("minecraft:painting/variant") {
                let [width, height] = painting_dimensions(content, entities, variant)?;
                found = self.hanging_survives(hanging_bounds(position, hit.face, width, height, false), hit.face, false, world, content, entities, tick)?;
            }
            found
        };
        if !survives { return Ok(Outcome::CONSUME); }
        let mut remaining = item.clone();
        remaining.count -= 1;
        self.predict_held(hand, (!remaining.empty()).then_some(remaining), inventory, false)?;
        Ok(Outcome::CLIENT)
    }
}

impl Player {
    #[expect(clippy::too_many_arguments, reason = "matches the existing item-on dispatch contract")]
    pub(super) fn minecart_item_on(
        &mut self,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let state = self.use_block(world, hit.position)?;
        if !content.block_has_tag(state, "minecraft:rails")? {
            return Ok(Outcome::Fail);
        }
        if self.enabled_features.contains("minecraft:minecart_improvements") {
            let is_rail = content.placement_metadata(state)?["hierarchy"].as_array()
                .ok_or("missing_source_block_hierarchy")?.iter()
                .any(|class| class.as_str() == Some("net.minecraft.world.level.block.BaseRailBlock"));
            let shape = if is_rail {
                Some(property(content, state, "shape")?)
            } else {
                None
            };
            let item = super::super::super::interaction::held(inventory, hand).ok_or("minecart item unavailable")?;
            let name = content.item_names.get(&item.id).ok_or("minecart item name unavailable")?;
            let bounds = experimental_cart_bounds(hit.position, shape, content.source_entity_dimensions(name)?);
            if intersects_entity_class(entities, bounds, "net.minecraft.world.entity.vehicle.minecart.AbstractMinecart", tick)? {
                return Ok(Outcome::Fail);
            }
        }
        let Some(item) = super::super::super::interaction::held(inventory, hand)
            .filter(|item| !item.empty())
        else {
            return Ok(Outcome::Pass);
        };
        let mut remaining = item.clone();
        remaining.count -= 1;
        self.predict_held(hand, (!remaining.empty()).then_some(remaining), inventory, false)?;
        Ok(Outcome::CLIENT)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap};

    #[test]
    fn received_frame_anchor_and_metadata_direction_define_pop_box() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let facts: Value = serde_json::from_str(include_str!(
            "../../../../assets/solaris/entities/entity-facts.json")).unwrap();
        let mut content = Content::test_cube();
        content.entity_names = facts["entities"].as_object().unwrap().iter()
            .map(|(name, schema)| (schema["protocol_id"].as_u64().unwrap() as u32, name.clone()))
            .collect();
        let models = Arc::new(crate::entities::Models::load_state(&content).unwrap());
        let kind = content.entity_names.iter().find(|(_, name)| *name == "minecraft:item_frame")
            .map(|(id, _)| *id).unwrap();
        let direction = models.accessor("minecraft:item_frame", "DATA_DIRECTION").unwrap();
        let mut entities = EntityStore::new();
        entities.activate_models(models);
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0; 16]);
        spawn.var(kind as i32);
        for coordinate in [4_f64, 4., 3.] {
            spawn.0.extend_from_slice(&coordinate.to_be_bytes());
        }
        spawn.0.extend_from_slice(&[0; 4]); // Zero velocity and three rotation bytes.
        spawn.var(2);
        entities.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
        let entity = entities.get(1).unwrap();
        let bounds = received_hanging_bounds(entity, &content, &entities, 1).unwrap();
        let expected = hanging_bounds([4, 4, 3], 2, 1, 1, true);
        assert_eq!(bounds.min, expected.min);
        assert_eq!(bounds.max, expected.max);
        let packet = [1, direction, 12, 5, 255];
        entities.apply(cb::SET_ENTITY_DATA, &mut Reader::new(&packet), &content, 2).unwrap();
        let entity = entities.get(1).unwrap();
        assert_eq!(entity.spawn_data, 2);
        assert_eq!(received_hanging_face(entity, &entities).unwrap(), 5);
        let bounds = received_hanging_bounds(entity, &content, &entities, 2).unwrap();
        let expected = hanging_bounds([4, 4, 3], 5, 1, 1, true);
        assert_eq!(bounds.min, expected.min);
        assert_eq!(bounds.max, expected.max);
    }

    fn fixture(rail: bool) -> (Player, World, Content, InventoryStore, EntityStore, BlockHit) {
        let mut content = Content::test_cube();
        content.item_names.insert(1, "minecraft:minecart".into());
        content.insert_test_item_components(1, BTreeMap::new());
        content.set_registries(BTreeMap::from([(
            "minecraft:block".into(),
            vec![("test:air".into(), Value::Null), ("test:rail".into(), Value::Null)],
        )]));
        content.set_tags(BTreeMap::from([(
            "minecraft:block".into(),
            BTreeMap::from([("minecraft:rails".into(), if rail { vec![1] } else { vec![] })]),
        )])).unwrap();
        let mut world = World::new(0, 16);
        let palette = |size| Palette { bits: 0, values: vec![0], words: Vec::new(), size };
        let mut blocks = palette(4096);
        blocks.set((4 << 8) | (4 << 4) | 4, 1);
        world.insert([0, 0], Chunk {
            sections: vec![Arc::new(Section { blocks, biomes: palette(64), non_empty_block_count: 1 })],
            sky: BTreeMap::new(), block: BTreeMap::new(),
            block_entities: Arc::new(Vec::new()),
            surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
            surface_heightmap_received: false,
        }).unwrap();
        let mut inventory = InventoryStore::default();
        inventory.player.slots[36] = Some(crate::inventory::read_stack(
            &mut crate::protocol::Reader::new(&[3, 1, 0, 0]), &content).unwrap());
        let hit = BlockHit { position: [4, 4, 4], state: 1, face: 1,
            point: [4.5, 5., 4.5], distance: 2., inside: false };
        (Player::new(), world, content, inventory, EntityStore::new(), hit)
    }

    #[test]
    fn minecart_nonrail_fails_without_debit() {
        let (mut player, world, content, inventory, entities, hit) = fixture(false);
        assert_eq!(player.minecart_item_on(&hit, 0, &world, &content, &inventory, &entities, 0).unwrap(), Outcome::Fail);
        assert!(player.take_use_prediction().is_none());
    }

    #[test]
    fn minecart_rail_debits_prediction() {
        let (mut player, world, content, inventory, entities, hit) = fixture(true);
        assert_eq!(player.minecart_item_on(&hit, 0, &world, &content, &inventory, &entities, 0).unwrap(), Outcome::CLIENT);
        assert!(matches!(player.take_use_prediction(), Some(UsePrediction::PlayerSlot { stack: Some(stack), replacement: false, .. }) if stack.count == 2));
        assert_eq!(inventory.player.slots[36].as_ref().unwrap().count, 3);
    }

    #[test]
    fn painting_vertical_fails_without_debit() {
        let (mut player, world, mut content, inventory, entities, hit) = fixture(false);
        content.item_names.insert(1, "minecraft:painting".into());
        assert_eq!(player.hanging_item_on(&hit, 0, &world, &content, &inventory, &entities, 0).unwrap(), Outcome::Fail);
        assert!(player.take_use_prediction().is_none());
    }

    #[test]
    fn frame_outside_build_height_fails_without_debit() {
        let (mut player, world, mut content, inventory, entities, mut hit) = fixture(false);
        content.item_names.insert(1, "minecraft:item_frame".into());
        hit.position[1] = 15;
        assert_eq!(player.hanging_item_on(&hit, 0, &world, &content, &inventory, &entities, 0).unwrap(), Outcome::Fail);
        assert!(player.take_use_prediction().is_none());
    }

    #[test]
    fn frame_and_even_painting_bounds_face_and_anchor() {
        let frame = hanging_bounds([4, 4, 3], 2, 1, 1, true);
        assert_eq!(frame.min, [4.125, 4.125, 3.9375]);
        assert_eq!(frame.max, [4.875, 4.875, 4.0]);
        let painting = hanging_bounds([4, 4, 3], 2, 2, 2, false);
        assert_eq!(painting.min, [3.0, 4.0, 3.9375]);
        assert_eq!(painting.max, [5.0, 6.0, 4.0]);
    }

    #[test]
    fn experimental_cart_adjustment_matches_source_rail_geometry() {
        for (shape, expected) in [
            (None, [4.5, 4.0625, 4.5]),
            (Some("north_south"), [4.5, 4.1, 4.5]),
            (Some("ascending_east"), [4.5, 4.6, 4.5]),
            (Some("south_east"), [4.75, 4.1, 4.75]),
            (Some("north_west"), [4.25, 4.1, 4.25]),
        ] {
            let bounds = experimental_cart_bounds([4, 4, 4], shape, [0., 0.]);
            assert_eq!(bounds.min, expected, "{shape:?}");
            assert_eq!(bounds.max, expected, "{shape:?}");
        }
    }

    #[test]
    fn supported_hanging_items_predict_debit() {
        let (_, world, mut content, inventory, entities, mut hit) = fixture(false);
        content.insert_test_placement_solid(1, true);
        content.set_registries(BTreeMap::from([("minecraft:painting_variant".into(), vec![
            ("test:one".into(), serde_json::json!({"width": 1, "height": 1, "asset_id": "test:one"})),
        ])]));
        content.set_tags(BTreeMap::from([("minecraft:painting_variant".into(),
            BTreeMap::from([("minecraft:placeable".into(), vec![0])]))])).unwrap();
        hit.face = 2;
        hit.point = [4.5, 4.5, 4.];
        for name in ["minecraft:item_frame", "minecraft:glow_item_frame", "minecraft:painting"] {
            content.item_names.insert(1, name.into());
            let mut player = Player::new();
            assert_eq!(player.hanging_item_on(&hit, 0, &world, &content, &inventory, &entities, 0).unwrap(),
                Outcome::CLIENT, "{name}");
            assert!(matches!(player.take_use_prediction(), Some(UsePrediction::PlayerSlot {
                stack: Some(stack), replacement: false, .. }) if stack.count == 2), "{name}");
        }
    }
}
