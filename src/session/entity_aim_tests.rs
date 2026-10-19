//! Entity interaction aiming: the swarm's villager `ordinary raycast does not target
//! requested entity` failures were rays through the box centre blocked by a block that
//! leaves other parts of the box visible.
use super::*;
use crate::{
    entities::{Handle, Models},
    player::Hit,
    protocol::{
        world::{Chunk, Palette, Section, SurfaceHeightmap},
        Reader, Writer,
    },
};

const VILLAGER: u32 = 1;
const ENTITY_ID: i32 = 42;

fn models() -> Models {
    let schema = serde_json::from_value(json!({
        "protocol_id": 1, "width": 0.6, "height": 1.95, "eye_height": 1.62,
        "fire_immune": false, "class": "net.minecraft.world.entity.LivingEntity",
        "dimension_constants": {}, "enum_ids": {}, "enum_numbers": {},
        "hierarchy": ["net.minecraft.world.entity.Entity", "net.minecraft.world.entity.LivingEntity"],
        "method_owners": {"isPickable": "net.minecraft.world.entity.LivingEntity"},
        "dimensions": {"width": 0.6, "height": 1.95, "eye_height": 1.62,
            "fixed": false, "attachments": {}},
        "default_attributes": {"minecraft:scale": {"id": 0, "value": 1.0}}, "accessors": {}
    }))
    .unwrap();
    Models {
        rigs: BTreeMap::new(),
        schemas: BTreeMap::from([("test:villager".into(), schema)]),
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

/// Player eye at (4.5, 5.62, 4.5); a villager standing at `villager`.
fn session(villager: [f64; 3]) -> (Session, Handle) {
    let mut session = Session::new_mode("entity-aim-test".into(), false);
    let mut content = Content::test_cube();
    content
        .entity_names
        .insert(VILLAGER, "test:villager".into());
    let palette = |size| Palette {
        bits: 0,
        values: vec![0],
        words: Vec::new(),
        size,
    };
    session.world = World::new(0, 16);
    session
        .world
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
                block_entities: crate::protocol::world::empty_block_entities(),
                surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                surface_heightmap_received: false,
            },
        )
        .unwrap();
    let mut packet = Writer::default();
    packet.var(ENTITY_ID);
    packet.0.extend([0u8; 16]);
    packet.var(VILLAGER as i32);
    for coordinate in villager {
        packet.f64(coordinate);
    }
    packet.0.extend([0u8; 5]);
    session
        .entities
        .apply(1, &mut Reader::new(&packet.0), &content, 1)
        .unwrap();
    session.entities.models = Some(Arc::new(models()));
    session.content = Some(content);
    session.phase = "play".into();
    session.player.position = [4.5, 4., 4.5];
    session.player.pose_known = true;
    let handle = session.entities.get(ENTITY_ID).unwrap().handle;
    (session, handle)
}

fn block(session: &mut Session, position: [i32; 3]) {
    session
        .world
        .set(position, 1, |state| Ok(state == 0))
        .unwrap();
}

fn target(session: &Session) -> Option<Hit> {
    session
        .player
        .raycast(
            &session.world,
            session.content.as_ref().unwrap(),
            &session.entities,
            session.client_tick,
        )
        .unwrap()
}

#[test]
fn centre_ray_blocked_but_lower_body_visible_still_aims_at_entity() {
    let (mut session, handle) = session([4.5, 4., 7.5]);
    // A one-high ledge at eye level in front of the villager hides its torso/head
    // from the centre ray but not its legs.
    block(&mut session, [4, 5, 6]);
    let (yaw, pitch) = (session.player.yaw, session.player.pitch);
    // Precondition: the old centre-only aim resolves to the ledge, not the villager.
    let bounds = session.entities.entity_bounds(handle, 0).unwrap();
    let eye = session.player.eye_position();
    let centre: [f64; 3] = std::array::from_fn(|a| (bounds.min[a] + bounds.max[a]) * 0.5);
    session
        .player
        .look(
            (((centre[2] - eye[2]).atan2(centre[0] - eye[0])).to_degrees() - 90.) as f32,
            -(centre[1] - eye[1])
                .atan2((centre[0] - eye[0]).hypot(centre[2] - eye[2]))
                .to_degrees() as f32,
        )
        .unwrap();
    assert!(matches!(target(&session), Some(Hit::Block(_))));
    session.player.look(yaw, pitch).unwrap();

    assert_eq!(session.aim_at_entity(handle, true).unwrap(), None);
    assert!(
        matches!(target(&session), Some(Hit::Entity { handle: hit, .. }) if hit == handle),
        "look must keep the successful aim"
    );
}

#[test]
fn probe_without_apply_restores_the_look() {
    let (mut session, handle) = session([4.5, 4., 7.5]);
    let (yaw, pitch) = (session.player.yaw, session.player.pitch);
    assert_eq!(session.aim_at_entity(handle, false).unwrap(), None);
    assert_eq!((session.player.yaw, session.player.pitch), (yaw, pitch));
}

#[test]
fn fully_occluded_entity_reports_the_blocking_block() {
    let (mut session, handle) = session([4.5, 4., 7.5]);
    block(&mut session, [4, 4, 6]);
    block(&mut session, [4, 5, 6]);
    let (yaw, pitch) = (session.player.yaw, session.player.pitch);
    let reason = session.aim_at_entity(handle, true).unwrap().unwrap();
    assert!(
        reason.contains("occluded by test:cube at 4,5,6"),
        "{reason}"
    );
    assert_eq!((session.player.yaw, session.player.pitch), (yaw, pitch));
}

#[test]
fn entity_beyond_interaction_reach_reports_distance() {
    let (mut session, handle) = session([4.5, 4., 9.5]);
    let reason = session.aim_at_entity(handle, true).unwrap().unwrap();
    assert!(reason.contains("out of interaction reach"), "{reason}");
    assert!(reason.contains("limit 3.00"), "{reason}");
}

#[test]
fn local_player_air_supply_is_captured_from_its_own_metadata_row() {
    let (mut session, _handle) = session([4.5, 4., 7.5]);
    let schema = serde_json::from_value(json!({
        "protocol_id": 2, "width": 0.6, "height": 1.8, "eye_height": 1.62,
        "fire_immune": false, "class": "net.minecraft.world.entity.player.Player",
        "dimension_constants": {}, "enum_ids": {}, "enum_numbers": {},
        "hierarchy": ["net.minecraft.world.entity.Entity"],
        "method_owners": {},
        "dimensions": {"width": 0.6, "height": 1.8, "eye_height": 1.62,
            "fixed": false, "attachments": {}},
        "default_attributes": {}, "accessors": {"DATA_AIR_SUPPLY_ID": [1, 1]}
    }))
    .unwrap();
    let mut models = models();
    models.schemas.insert("minecraft:player".into(), schema);
    session.entities.models = Some(Arc::new(models));
    session.player.entity_id = 7;
    session.entities.own_id = Some(7);
    assert_eq!(session.local_air_supply(), None);
    // SetEntityData for the local id: index 1, Int serializer, 250, terminator.
    let mut packet = Writer::default();
    packet.var(7);
    packet.0.extend([1u8]);
    packet.var(1);
    packet.var(250);
    packet.0.extend([255u8]);
    session
        .entities
        .apply(
            99,
            &mut Reader::new(&packet.0),
            session.content.as_ref().unwrap(),
            1,
        )
        .unwrap();
    assert_eq!(session.local_air_supply(), Some(250));
}

#[test]
fn merchant_offers_are_observable_with_effective_price() {
    use crate::inventory::{ItemCost, Stack, Trade, Trades};
    let (session, _handle) = session([4.5, 4., 7.5]);
    let stack = |id, count| {
        let mut stack = Stack::default();
        stack.id = id;
        stack.count = count;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:max_stack_size".to_owned(),
            json!(64),
        )]));
        stack
    };
    let trades = Trades {
        offers: vec![Trade {
            buy_a: ItemCost {
                stack: stack(5, 20),
                components: Arc::new(BTreeMap::new()),
            },
            buy_b: None,
            sell: stack(6, 1),
            out_of_stock: false,
            uses: 2,
            max_uses: 12,
            xp: 5,
            special_price: -3,
            price_multiplier: 0.05,
            demand: 0,
        }],
        level: 1,
        xp: 0,
        show_progress: true,
        can_restock: true,
        selected: 0,
    };
    let value = session.trades_value(&trades);
    let offer = &value["offers"][0];
    assert_eq!(offer["index"], 0);
    assert_eq!(offer["buy_a"]["base_count"], 20);
    assert_eq!(offer["buy_a"]["count"], 17);
    assert_eq!(offer["buy_b"], Value::Null);
    assert_eq!(offer["sell"]["count"], 1);
    assert_eq!(offer["available"], true);
    assert_eq!(offer["uses"], 2);
    assert_eq!(value["villager_level"], 1);
}
