//! Ordinary block placement must affect local collision before server confirmation.
use super::*;
use crate::{
    inventory::Stack,
    player::{InteractionIntent, collision::{self, Bounds}},
    protocol::{self, packet_ids::play::clientbound as cb},
};

const SUPPORT: [i32; 3] = [4, 5, 7];
const PLACED: [i32; 3] = [4, 5, 6];

fn placement_session() -> Session {
    let mut session = super::mining_start_tests::session();
    let content = session.content.as_mut().unwrap();
    for state in 0..=1 {
        content.insert_test_block_interaction(state, state == 0);
        content.insert_test_block_features(state);
        content.insert_test_placement_metadata(state, if state == 1 {
            json!({"set_placed_by_owner":"Block", "collision_owner":"Block"})
        } else {
            json!({})
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
    content.insert_test_block_sound(1, 1., 1., BTreeMap::from([("place".into(), "test:place".into())]));
    content.set_registries(BTreeMap::from([("minecraft:item".into(), vec![
        ("minecraft:air".into(), Value::Null), ("test:item_1".into(), Value::Null)
    ])]));
    session.world.set(SUPPORT, 1, |state| Ok(state == 0)).unwrap();
    let mut stack = Stack::default();
    stack.id = 1;
    stack.count = 3;
    session.inventory.player.slots[36] = Some(stack);
    session.input(InputIntent::Look { yaw: 0., pitch: 0. }).unwrap();
    session
}

fn collides(session: &Session) -> bool {
    let mut shapes = Vec::new();
    collision::gather(
        &session.world, session.content.as_ref().unwrap(), &session.entities,
        session.client_tick, Bounds::new([4.1, 5.1, 6.1, 4.9, 5.9, 6.9]), &mut shapes,
    ).unwrap();
    !shapes.is_empty()
}

fn ack(session: &mut Session, sequence: i32) {
    let mut packet = Writer::default();
    packet.var(sequence);
    session.apply(Phase::Play, cb::BLOCK_CHANGED_ACK, &packet.0).unwrap();
}

fn update(session: &mut Session, state: i32) {
    let mut packet = Writer::default();
    packet.i64(protocol::pack_pos(PLACED));
    packet.var(state);
    session.apply(Phase::Play, cb::BLOCK_UPDATE, &packet.0).unwrap();
}

#[test]
fn ordinary_placement_is_immediate_and_reconciles_server_updates_and_ack() {
    for server_state in [None, Some(0), Some(1)] {
        let mut session = placement_session();
        let _peer = super::mining_start_tests::connect(&mut session);
        assert_eq!(session.world.block(PLACED), Some(0));
        assert!(!collides(&session));
        let result = session.interact(InteractionIntent::Use { hand: 0 }).unwrap();
        assert_eq!(result["consumes_action"], true);
        assert_eq!(session.world.block(PLACED), Some(1));
        assert!(collides(&session), "predicted placement must immediately collide");
        assert_eq!(session.received_block(PLACED), Some(0));
        let pending = session.predicted_blocks.get(&PLACED).expect("placement must be pending");
        assert_eq!(pending.received, 0);
        assert_eq!(pending.sequence, 1);
        let sequence = pending.sequence;
        ack(&mut session, sequence - 1);
        assert_eq!(session.world.block(PLACED), Some(1));
        assert_eq!(session.received_block(PLACED), Some(0));
        assert_eq!(session.predicted_blocks.get(&PLACED).unwrap().sequence, sequence);
        if let Some(state) = server_state {
            update(&mut session, state);
            assert_eq!(session.received_block(PLACED), Some(state as u32));
            assert_eq!(session.world.block(PLACED), Some(1));
            assert!(collides(&session));
            let pending = session.predicted_blocks.get(&PLACED).expect("server update must retain prediction until ack");
            assert_eq!(pending.received, state as u32);
            assert_eq!(pending.sequence, sequence);
        }
        ack(&mut session, sequence);
        let expected = server_state.unwrap_or(0) as u32;
        assert_eq!(session.received_block(PLACED), Some(expected));
        assert_eq!(session.world.block(PLACED), Some(expected));
        assert_eq!(collides(&session), expected == 1);
        assert!(!session.predicted_blocks.contains_key(&PLACED));
    }
}

#[test]
fn door_use_predicts_both_halves_and_reconciles_server_rejection() {
    for (clicked_upper, accepted) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut session = super::mining_start_tests::session();
        let _peer = super::mining_start_tests::connect(&mut session);
        session.content.as_mut().unwrap().insert_test_door();
        let upper = [PLACED[0], PLACED[1] + 1, PLACED[2]];
        session.world.set(PLACED, 1, |state| Ok(state == 0)).unwrap();
        session.world.set(upper, 3, |state| Ok(state == 0)).unwrap();
        session.use_item_on(if clicked_upper { upper } else { PLACED }, 3, 0).unwrap();
        assert_eq!(session.world.block(PLACED), Some(2));
        assert_eq!(session.world.block(upper), Some(4), "the paired door half follows the click");
        assert_eq!(session.predicted_blocks[&PLACED].sequence, 1);
        assert_eq!(session.predicted_blocks[&upper].sequence, 1);
        for (position, state) in [(PLACED, if accepted { 2 } else { 1 }), (upper, if accepted { 4 } else { 3 })] {
            let mut packet = Writer::default();
            packet.i64(protocol::pack_pos(position));
            packet.var(state);
            session.apply(Phase::Play, cb::BLOCK_UPDATE, &packet.0).unwrap();
        }
        ack(&mut session, 1);
        assert_eq!(session.world.block(PLACED), Some(if accepted { 2 } else { 1 }));
        assert_eq!(session.world.block(upper), Some(if accepted { 4 } else { 3 }));
        assert!(session.predicted_blocks.is_empty());
    }
}
