use super::*;
use crate::entities::Models;
use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap};
use crate::protocol::{Reader, Writer};

fn fixture() -> (Player, World, Content, InventoryStore, EntityStore) {
    let mut content = Content::test_cube();
    content.insert_test_block_interaction(0, true);
    content.insert_test_block_interaction(1, false);
    content.insert_test_item_interaction(2, "ArmorStandItem", "Item", "ArmorStandItem");
    content.insert_test_entity_dimensions("minecraft:armor_stand", [0.5, 1.975]);
    let mut world = World::new(0, 16);
    let palette = |size| Palette {
        bits: 0,
        values: vec![0],
        words: Vec::new(),
        size,
    };
    world
        .insert(
            [0, 0],
            Chunk {
                sections: vec![Arc::new(Section {
                    blocks: palette(4096),
                    biomes: palette(64),
                    non_empty_block_count: 0,
                })],
                sky: BTreeMap::new(),
                block: BTreeMap::new(),
                block_entities: Arc::new(Vec::new()),
                surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                surface_heightmap_received: false,
            },
        )
        .unwrap();
    let mut inventory = InventoryStore::default();
    let mut stack = Stack::default();
    stack.id = 2;
    stack.count = 3;
    inventory.player.slots[36] = Some(stack);
    let mut player = Player::new();
    player.position = [12., 4., 12.];
    let mut entities = EntityStore::new();
    entities.models = Some(Arc::new(models(false)));
    (player, world, content, inventory, entities)
}

fn models(hard: bool) -> Models {
    let schema = serde_json::from_value(serde_json::json!({
        "protocol_id": 1, "width": 0.5, "height": 0.5, "eye_height": 0.25,
        "fire_immune": false, "class": "net.minecraft.world.entity.Entity",
        "dimension_constants": {}, "enum_ids": {}, "enum_numbers": {},
        "hierarchy": ["net.minecraft.world.entity.Entity"],
        "method_owners": {"canBeCollidedWith": if hard {
            "net.minecraft.world.entity.vehicle.boat.AbstractBoat"
        } else { "net.minecraft.world.entity.Entity" }},
        "dimensions": {"width": 0.5, "height": 0.5, "eye_height": 0.25,
            "fixed": false, "attachments": {}},
        "default_attributes": {}, "accessors": {}
    }))
    .unwrap();
    Models {
        rigs: BTreeMap::new(),
        schemas: BTreeMap::from([("test:entity".into(), schema)]),
        families: BTreeMap::new(),
        builtin_variants: BTreeMap::new(),
        simple_particles: Vec::new(),
        default_skins: Vec::new(),
        poses: BTreeMap::from([(0, "STANDING".into())]),
        attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(),
        digest: String::new(),
        armor_models: BTreeMap::new(),
        motion: BTreeMap::new(),
    }
}

fn hit(state: u32, face: i32) -> BlockHit {
    BlockHit {
        position: [4, 4, 4],
        state,
        face,
        point: [4.5, 5., 4.5],
        distance: 2.,
        inside: false,
    }
}

fn use_on(
    player: &mut Player,
    hit: &BlockHit,
    world: &World,
    content: &Content,
    inventory: &InventoryStore,
    entities: &EntityStore,
) -> Outcome {
    player
        .item_on_result(hit, 0, world, content, inventory, entities, 1)
        .unwrap()
}

fn spawn(entities: &mut EntityStore, content: &mut Content, position: [f64; 3], hard: bool) {
    content.entity_names.insert(1, "test:entity".into());
    entities.models = None;
    let mut packet = Writer::default();
    packet.var(42);
    packet.0.extend([0u8; 16]);
    packet.var(1);
    for coordinate in position {
        packet.f64(coordinate);
    }
    // Zero compressed velocity, pitch, yaw, head yaw, spawn data.
    packet.0.extend([0u8; 5]);
    entities
        .apply(1, &mut Reader::new(&packet.0), content, 1)
        .unwrap();
    entities.models = Some(Arc::new(models(hard)));
}

#[test]
fn p1_armor_stand_down_fails_without_debit() {
    let (mut player, _world, content, inventory, entities) = fixture();
    let world = World::new(0, 16);
    assert_eq!(
        use_on(
            &mut player,
            &hit(0, 0),
            &world,
            &content,
            &inventory,
            &entities
        ),
        Outcome::Fail
    );
    assert!(player.use_predictions.is_empty());
}

#[test]
fn p1_armor_stand_placement_selects_replaceable_or_adjacent_bottom_center() {
    let (mut player, mut world, content, inventory, entities) = fixture();
    let stack = inventory.player.slots[36].as_ref().unwrap();
    assert_eq!(
        player
            .placement_target(&hit(0, 1), stack, &world, &content)
            .unwrap(),
        ([4, 4, 4], true)
    );
    world.set([4, 4, 4], 1, |state| Ok(state == 0)).unwrap();
    assert_eq!(
        player
            .placement_target(&hit(1, 1), stack, &world, &content)
            .unwrap(),
        ([4, 5, 4], false)
    );
    player.position = [4.5, 5., 4.5];
    assert_eq!(
        use_on(
            &mut player,
            &hit(1, 1),
            &world,
            &content,
            &inventory,
            &entities
        ),
        Outcome::Fail
    );
    assert!(player.use_predictions.is_empty());
}

#[test]
fn p1_armor_stand_empty_context_obstructs_scaffolding_but_not_powder_snow() {
    for (block, expected) in [
        ("minecraft:scaffolding", Outcome::Fail),
        ("minecraft:powder_snow", Outcome::CLIENT),
    ] {
        let (mut player, mut world, mut content, inventory, entities) = fixture();
        content.names[1] = block.into();
        world.set([4, 5, 4], 1, |state| Ok(state == 0)).unwrap();
        assert_eq!(
            use_on(
                &mut player,
                &hit(0, 1),
                &world,
                &content,
                &inventory,
                &entities
            ),
            expected,
            "{block}"
        );
        if expected == Outcome::Fail {
            assert!(player.use_predictions.is_empty());
        } else {
            assert!(player.use_predictions.iter().any(|prediction| matches!(
                prediction,
                UsePrediction::PlayerSlot { slot: 36, stack: Some(stack), .. }
                    if stack.count == 2
            )));
        }
    }
}

#[test]
fn p1_armor_stand_block_collision_uses_full_entity_height() {
    let (mut player, mut world, content, inventory, entities) = fixture();
    world.set([4, 5, 4], 1, |state| Ok(state == 0)).unwrap();
    assert_eq!(
        use_on(
            &mut player,
            &hit(0, 1),
            &world,
            &content,
            &inventory,
            &entities
        ),
        Outcome::Fail
    );
    assert!(player.use_predictions.is_empty());
}

#[test]
fn p1_armor_stand_adjacent_empty_nonreplaceable_does_not_require_can_place() {
    let (mut player, mut world, mut content, inventory, entities) = fixture();
    world.set([4, 4, 4], 1, |state| Ok(state == 0)).unwrap();
    world.set([4, 5, 4], 1, |state| Ok(state == 0)).unwrap();
    content.states[1].collision = crate::protocol::content::Collision::Empty;
    assert_eq!(
        use_on(
            &mut player,
            &hit(1, 1),
            &world,
            &content,
            &inventory,
            &entities
        ),
        Outcome::CLIENT
    );
}

#[test]
fn p1_armor_stand_all_entities_block_even_nonliving_noncollidable() {
    let (mut player, world, mut content, inventory, mut entities) = fixture();
    spawn(&mut entities, &mut content, [4.5, 4., 4.5], false);
    assert_eq!(
        use_on(
            &mut player,
            &hit(0, 1),
            &world,
            &content,
            &inventory,
            &entities
        ),
        Outcome::Fail
    );
    assert!(player.use_predictions.is_empty());
}

#[test]
fn p1_armor_stand_hard_collision_checks_inflated_query() {
    for (hard, expected) in [(false, Outcome::CLIENT), (true, Outcome::Fail)] {
        let (mut player, world, mut content, inventory, mut entities) = fixture();
        // Entity edge just outside placement bounds, within EntityGetter's 1e-7 inflation.
        spawn(&mut entities, &mut content, [5.00000005, 4., 4.5], hard);
        assert_eq!(
            use_on(
                &mut player,
                &hit(0, 1),
                &world,
                &content,
                &inventory,
                &entities
            ),
            expected
        );
    }
}

#[test]
fn p1_armor_stand_local_player_blocks_but_spectator_does_not() {
    let (mut player, world, content, inventory, entities) = fixture();
    player.position = [4.5, 4., 4.5];
    assert_eq!(
        use_on(
            &mut player,
            &hit(0, 1),
            &world,
            &content,
            &inventory,
            &entities
        ),
        Outcome::Fail
    );
    player.game_mode = 3;
    assert_eq!(
        use_on(
            &mut player,
            &hit(0, 1),
            &world,
            &content,
            &inventory,
            &entities
        ),
        Outcome::CLIENT
    );
}

#[test]
fn p1_armor_stand_success_debits_and_swings_without_local_spawn() {
    let (mut player, world, content, mut inventory, entities) = fixture();
    for count in [3, 1] {
        inventory.player.slots[36].as_mut().unwrap().count = count;
        assert_eq!(
            use_on(
                &mut player,
                &hit(0, 1),
                &world,
                &content,
                &inventory,
                &entities
            ),
            Outcome::Success {
                swing: true,
                item_interaction: true
            }
        );
        match player.take_use_prediction().unwrap() {
            UsePrediction::PlayerSlot {
                slot: 36,
                stack,
                replacement: false,
            } => {
                assert_eq!(
                    stack.map(|stack| stack.count),
                    if count == 1 { None } else { Some(count - 1) }
                );
            }
            prediction => panic!("unexpected prediction: {prediction:?}"),
        }
        assert!(player.take_use_prediction().is_none());
        assert!(entities.is_empty());
    }
}

#[test]
fn p1_armor_stand_outer_creative_semantics_restore_held_count() {
    let (mut player, world, content, mut inventory, entities) = fixture();
    player.abilities.instabuild = true;
    for count in [3, 1] {
        inventory.player.slots[36].as_mut().unwrap().count = count;
        assert_eq!(
            use_on(
                &mut player,
                &hit(0, 1),
                &world,
                &content,
                &inventory,
                &entities
            ),
            Outcome::CLIENT
        );
        match player.take_use_prediction().unwrap() {
            UsePrediction::PlayerSlot {
                slot: 36,
                stack: Some(stack),
                replacement: false,
            } => assert_eq!(stack.count, count),
            prediction => panic!("unexpected prediction: {prediction:?}"),
        }
        assert!(player.take_use_prediction().is_none());
        assert!(entities.is_empty());
    }
}
