//! 26.3 locally destroys blocks inside prediction, before the server reply.
use super::*;
use crate::{player::InteractionIntent, protocol::{self, packet_ids::play::clientbound as cb}};

const TARGET: [i32; 3] = [4, 5, 6];

fn fixture(creative: bool, hardness: f32) -> (Session, std::net::TcpListener) {
    let mut session = super::mining_start_tests::session();
    let peer = super::mining_start_tests::connect(&mut session);
    let content = session.content.as_mut().unwrap();
    content.states[1].hardness = hardness;
    content.insert_test_block_sound(1, 1., 1., BTreeMap::from([
        ("break".into(), "test:break".into()), ("hit".into(), "test:hit".into()),
    ]));
    session.player.game_mode = u8::from(creative);
    session.world.set(TARGET, 1, |state| Ok(state == 0)).unwrap();
    session.input(InputIntent::Look { yaw: 0., pitch: 0. }).unwrap();
    (session, peer)
}

fn finish(session: &mut Session) -> i32 {
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

fn ack(session: &mut Session, sequence: i32) {
    let mut packet = Writer::default();
    packet.var(sequence);
    session.apply(Phase::Play, cb::BLOCK_CHANGED_ACK, &packet.0).unwrap();
}

fn update(session: &mut Session, state: i32) {
    let mut packet = Writer::default();
    packet.i64(protocol::pack_pos(TARGET));
    packet.var(state);
    session.apply(Phase::Play, cb::BLOCK_UPDATE, &packet.0).unwrap();
}

#[test]
fn s24_finished_break_is_locally_air_before_any_server_reply() {
    let (mut session, _peer) = fixture(false, 0.1);
    session.interact(InteractionIntent::Attack).unwrap();
    assert_eq!(session.world.block(TARGET), Some(1), "unfinished mining stays solid");
    let sequence = finish(&mut session);
    assert_eq!(session.world.block(TARGET), Some(0), "finished mining must not wait for the server");
    assert_eq!(session.received_block(TARGET), Some(1), "prediction is not server observation");
    assert!(session.content.as_ref().unwrap().collision_boxes(session.world.block(TARGET).unwrap()).unwrap().is_empty());
    assert_eq!(session.predicted_blocks[&TARGET].sequence, sequence);
    ack(&mut session, sequence - 1);
    assert_eq!(session.world.block(TARGET), Some(0));
    assert!(session.predicted_blocks.contains_key(&TARGET));
    update(&mut session, 0);
    assert_eq!(session.received_block(TARGET), Some(0));
    assert!(session.predicted_blocks.contains_key(&TARGET), "retain prediction until its ack");
    ack(&mut session, sequence);
    assert_eq!(session.world.block(TARGET), Some(0));
    assert!(!session.predicted_blocks.contains_key(&TARGET));
}

#[test]
fn s24_instant_and_creative_breaks_predict_and_rejected_ack_restores_collision() {
    for (creative, hardness) in [(false, 0.), (true, 50.)] {
        let (mut session, _peer) = fixture(creative, hardness);
        let sequence = finish(&mut session);
        assert_eq!(session.world.block(TARGET), Some(0));
        assert_eq!(session.received_block(TARGET), Some(1));
        update(&mut session, 1);
        assert_eq!(session.world.block(TARGET), Some(0), "server state waits for the pending ack");
        assert!(session.predicted_blocks.contains_key(&TARGET));
        ack(&mut session, sequence);
        assert_eq!(session.world.block(TARGET), Some(1));
        assert!(!session.content.as_ref().unwrap().collision_boxes(1).unwrap().is_empty());
        assert!(!session.predicted_blocks.contains_key(&TARGET));
    }
}

#[test]
fn s24_waterlogged_break_preserves_its_source_fluid_until_ack() {
    use crate::protocol::content::{FluidKind, FluidState};
    let (mut session, _peer) = fixture(false, 0.);
    let content = session.content.as_mut().unwrap();
    let water = FluidState { kind: FluidKind::Water, amount: 8, falling: false, waterlogged: false };
    let mut fluid_block = content.states[0].clone();
    fluid_block.is_air = false;
    fluid_block.fluid = Some(water);
    content.states.push(fluid_block);
    content.states[1].fluid = Some(FluidState { waterlogged: true, ..water });
    content.insert_test_fluid_legacy_state("minecraft:water", 2);
    let sequence = finish(&mut session);
    assert_eq!(session.world.block(TARGET), Some(2), "destroyBlock leaves the original fluid, not air");
    assert_eq!(session.received_block(TARGET), Some(1));
    ack(&mut session, sequence);
    assert_eq!(session.world.block(TARGET), Some(1));
}
