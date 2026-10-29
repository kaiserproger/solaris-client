//! Block-entity lifecycle must follow ordinary local block predictions and reconciliation.
use super::*;
use crate::{
    inventory::{nbt, read_stack, Tag},
    player::InteractionIntent,
    protocol::{self, Reader, packet_ids::play::clientbound as cb},
};

const TARGET: [i32; 3] = [4, 5, 6];
const SUPPORT: [i32; 3] = [4, 5, 7];

fn fixture(kind: &str) -> (Session, std::net::TcpListener) {
    let mut session = super::mining_start_tests::session();
    let peer = super::mining_start_tests::connect(&mut session);
    let content = session.content.as_mut().unwrap();
    content.block_entity_names.insert(0, kind.into());
    content.insert_test_placement_metadata(1, json!({"block_entity_type": kind}));
    content.states[1].hardness = 0.;
    content.insert_test_block_sound(1, 1., 1., BTreeMap::from([
        ("break".into(), "test:break".into()),
        ("hit".into(), "test:hit".into()),
        ("place".into(), "test:place".into()),
    ]));
    session.input(InputIntent::Look { yaw: 0., pitch: 0. }).unwrap();
    (session, peer)
}

fn update(session: &mut Session, state: i32) {
    let mut packet = Writer::default();
    packet.i64(protocol::pack_pos(TARGET));
    packet.var(state);
    session.apply(Phase::Play, cb::BLOCK_UPDATE, &packet.0).unwrap();
}

fn ack(session: &mut Session, sequence: i32) {
    let mut packet = Writer::default();
    packet.var(sequence);
    session.apply(Phase::Play, cb::BLOCK_CHANGED_ACK, &packet.0).unwrap();
}

fn sign_data(session: &mut Session) -> Vec<u8> {
    sign_data_with_front(session, "received front")
}

fn sign_data_with_front(session: &mut Session, front: &str) -> Vec<u8> {
    let text = |line: &str| Tag::record([
        ("messages", Tag::List([line, "", "", ""].into_iter().map(Tag::string).collect())),
        ("color", Tag::string("black")),
        ("has_glowing_text", Tag::Byte(0)),
    ]);
    let mut raw = Writer::default();
    nbt::write(&Tag::record([
        ("front_text", text(front)),
        ("back_text", text("received back")),
        ("is_waxed", Tag::Byte(0)),
    ]), &mut raw).unwrap();
    let mut packet = Writer::default();
    packet.i64(protocol::pack_pos(TARGET));
    packet.var(0);
    packet.0.extend_from_slice(&raw.0);
    session.apply(Phase::Play, cb::BLOCK_ENTITY_DATA, &packet.0).unwrap();
    raw.0
}

fn break_block(session: &mut Session) -> i32 {
    for _ in 0..128 {
        session.interact(InteractionIntent::Attack).unwrap();
        if let Some(value) = session.player.breaking_observation()
            && value["finished_dispatched"] == true
        {
            return value["sequence"].as_i64().unwrap() as i32;
        }
        session.client_tick += 1;
    }
    panic!("bounded ordinary attack sequence did not finish mining");
}

fn setup_placement(session: &mut Session, kind: &str) {
    let content = session.content.as_mut().unwrap();
    for state in 0..=1 {
        content.insert_test_block_interaction(state, state == 0);
        content.insert_test_block_features(state);
        content.insert_test_placement_metadata(state, if state == 1 {
            json!({"set_placed_by_owner":"Block", "collision_owner":"Block", "block_entity_type":kind})
        } else {
            json!({"block_entity_type":null})
        });
        content.insert_test_placement_state(state, json!({
            "is_solid": state == 1, "blocks_motion": state == 1,
            "liquid": false, "can_burn": false, "instrument": "harp",
            "instrument_above": false, "static_support_cache": true,
            "supports": null, "collision_face_full": null, "support_face_full": null,
            "collision_boxes": if state == 1 { json!([[0., 0., 0., 1., 1., 1.]]) } else { json!([]) },
            "support_boxes": []
        }));
    }
    content.insert_test_item_interaction(1, "BlockItem", "Item", "BlockItem");
    content.insert_test_item_block(1, 1);
    content.insert_test_item_components(1, BTreeMap::new());
    content.insert_test_item_placement_metadata(1, json!({
        "update_context_owner":"BlockItem", "placement_state_owner":"BlockItem",
        "must_survive":true, "place_block_owner":"BlockItem", "place_sound_owner":"BlockItem"
    }));
    content.set_registries(BTreeMap::from([("minecraft:item".into(), vec![
        ("minecraft:air".into(), Value::Null), ("test:item_1".into(), Value::Null)
    ])]));
    session.world.set(SUPPORT, 1, |state| Ok(state == 0)).unwrap();
    let mut wire = Writer::default();
    wire.var(3);
    wire.var(1);
    wire.var(0);
    wire.var(0);
    session.inventory.player.slots[36] = Some(read_stack(&mut Reader::new(&wire.0), content).unwrap());
}

#[test]
fn accepted_break_removes_chest_and_sign_entities_immediately_and_after_ack() {
    for kind in ["minecraft:chest", "minecraft:sign"] {
        let (mut session, _peer) = fixture(kind);
        update(&mut session, 1);
        assert_eq!(session.entities.block_entity_name(TARGET), Some(kind));
        let sequence = break_block(&mut session);
        assert_eq!(session.world.block(TARGET), Some(0));
        assert_eq!(session.entities.block_entity_name(TARGET), None, "predicted air must not retain {kind}");
        assert_eq!(session.entities.block_entity_data(TARGET), None);
        update(&mut session, 0);
        assert_eq!(session.world.block(TARGET), Some(0));
        assert_eq!(session.entities.block_entity_name(TARGET), None);
        ack(&mut session, sequence);
        assert_eq!(session.world.block(TARGET), Some(0));
        assert_eq!(session.entities.block_entity_name(TARGET), None);
        assert_eq!(session.entities.block_entity_nbt(TARGET), None);
    }
}

#[test]
fn predicted_sign_placement_accepts_data_before_ack() {
    let (mut session, _peer) = fixture("minecraft:sign");
    setup_placement(&mut session, "minecraft:sign");
    assert_eq!(session.interact(InteractionIntent::Use { hand: 0 }).unwrap()["consumes_action"], true);
    assert_eq!(session.world.block(TARGET), Some(1));
    assert_eq!(session.entities.block_entity_name(TARGET), Some("minecraft:sign"));
    let sequence = session.predicted_blocks[&TARGET].sequence;
    let raw = sign_data(&mut session);
    let received = session.entities.block_entity_data(TARGET).unwrap().clone();
    assert_eq!(received["front_text"]["messages"][0], "received front");
    assert_eq!(session.entities.block_entity_nbt(TARGET), Some(raw.as_slice()));
    update(&mut session, 1);
    ack(&mut session, sequence);
    assert_eq!(session.world.block(TARGET), Some(1));
    assert_eq!(session.entities.block_entity_name(TARGET), Some("minecraft:sign"));
    assert_eq!(session.entities.block_entity_data(TARGET), Some(&received));
    assert_eq!(session.entities.block_entity_nbt(TARGET), Some(raw.as_slice()));
}

#[test]
fn rejected_sign_break_restores_received_nbt_and_local_text() {
    let (mut session, _peer) = fixture("minecraft:sign");
    update(&mut session, 1);
    let raw = sign_data(&mut session);
    session.entities.predict_sign_text(TARGET, true, &[
        "locally edited".into(), String::new(), String::new(), String::new(),
    ]).unwrap();
    let before = session.entities.block_entity_data(TARGET).unwrap().clone();
    assert_eq!(before["front_text"]["messages"][0], "locally edited");
    assert_eq!(before["back_text"]["messages"][0], "received back");
    let sequence = break_block(&mut session);
    assert_eq!(session.world.block(TARGET), Some(0));
    assert_eq!(session.entities.block_entity_name(TARGET), None);
    update(&mut session, 1);
    assert_eq!(session.world.block(TARGET), Some(0));
    ack(&mut session, sequence);
    assert_eq!(session.world.block(TARGET), Some(1));
    assert_eq!(session.entities.block_entity_name(TARGET), Some("minecraft:sign"));
    assert_eq!(session.entities.block_entity_data(TARGET), Some(&before));
    assert_eq!(session.entities.block_entity_nbt(TARGET), Some(raw.as_slice()));
}

#[test]
fn rejected_break_same_type_state_and_stale_ack_preserve_sign_data() {
    let (mut session, _peer) = fixture("minecraft:sign");
    let content = session.content.as_mut().unwrap();
    content.states.push(content.states[1].clone());
    content.insert_test_placement_metadata(2, json!({"block_entity_type":"minecraft:sign"}));
    update(&mut session, 1);
    let raw = sign_data(&mut session);
    let before = session.entities.block_entity_data(TARGET).unwrap().clone();
    let sequence = break_block(&mut session);
    update(&mut session, 2);
    ack(&mut session, sequence - 1);
    assert_eq!(session.world.block(TARGET), Some(0));
    assert_eq!(session.entities.block_entity_name(TARGET), None);
    ack(&mut session, sequence);
    assert_eq!(session.world.block(TARGET), Some(2));
    assert_eq!(session.entities.block_entity_name(TARGET), Some("minecraft:sign"));
    assert_eq!(session.entities.block_entity_data(TARGET), Some(&before));
    assert_eq!(session.entities.block_entity_nbt(TARGET), Some(raw.as_slice()));
}

#[test]
fn sign_data_received_during_predicted_break_updates_rollback() {
    let (mut session, _peer) = fixture("minecraft:sign");
    update(&mut session, 1);
    let original = sign_data(&mut session);
    let sequence = break_block(&mut session);
    assert_eq!(session.world.block(TARGET), Some(0));
    assert_eq!(session.entities.block_entity_name(TARGET), None);
    update(&mut session, 1);
    let latest = sign_data_with_front(&mut session, "new server text");
    assert_ne!(latest, original);
    assert_eq!(session.world.block(TARGET), Some(0));
    assert_eq!(session.entities.block_entity_name(TARGET), None);
    ack(&mut session, sequence);
    assert_eq!(session.world.block(TARGET), Some(1));
    assert_eq!(session.entities.block_entity_name(TARGET), Some("minecraft:sign"));
    assert_eq!(
        session.entities.block_entity_data(TARGET).unwrap()["front_text"]["messages"][0],
        "new server text"
    );
    assert_eq!(session.entities.block_entity_nbt(TARGET), Some(latest.as_slice()));
}

#[test]
fn new_server_block_entity_data_during_predicted_break_survives_ack() {
    let (mut session, _peer) = fixture("minecraft:sign");
    let content = session.content.as_mut().unwrap();
    content.insert_test_placement_metadata(1, json!({"block_entity_type":null}));
    content.names.push("test:sign_block".into());
    let mut sign_state = content.states[1].clone();
    sign_state.block = 2;
    sign_state.registry_id = 2;
    content.states.push(sign_state);
    content.insert_test_placement_metadata(2, json!({"block_entity_type":"minecraft:sign"}));
    update(&mut session, 1);
    assert_eq!(session.entities.block_entity_name(TARGET), None);
    let sequence = break_block(&mut session);
    assert_eq!(session.world.block(TARGET), Some(0));
    update(&mut session, 2);
    let raw = sign_data_with_front(&mut session, "new entity");
    assert_eq!(session.world.block(TARGET), Some(0));
    assert_eq!(session.entities.block_entity_name(TARGET), None);
    ack(&mut session, sequence);
    assert_eq!(session.world.block(TARGET), Some(2));
    assert_eq!(session.entities.block_entity_name(TARGET), Some("minecraft:sign"));
    assert_eq!(
        session.entities.block_entity_data(TARGET).unwrap()["front_text"]["messages"][0],
        "new entity"
    );
    assert_eq!(session.entities.block_entity_nbt(TARGET), Some(raw.as_slice()));
}
