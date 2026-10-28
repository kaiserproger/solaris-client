//! Authored fixtures follow the pinned 26.3 packet codecs (see HUD fix receipt).
use super::*;
use crate::protocol::{nbt::Nbt, packet_ids::play::clientbound as cb};

fn session() -> Session {
    let mut session = Session::new_mode("rc1-hud-packets".into(), false);
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    for (id, name) in [(2, "minecraft:map"), (3, "minecraft:filled_map")] {
        content.item_names.insert(id, name.into());
        content.insert_test_item_components(id, BTreeMap::new());
    }
    content.components.insert(7, "minecraft:map_id".into());
    session.ui = Some(UiState::new_state(&content).unwrap());
    session.content = Some(content);
    session
}

fn slot_packet(player_inventory: bool, item: i32, map_id: Option<i32>) -> Writer {
    let mut packet = Writer::default();
    if player_inventory {
        packet.var(2); // inventory hotbar slot 2 -> menu slot 38
    } else {
        packet.var(0); // player inventory menu
        packet.var(2); // state id
        packet.0.extend(38i16.to_be_bytes());
    }
    packet.var(1);
    packet.var(item);
    packet.var(i32::from(map_id.is_some()));
    packet.var(0);
    if let Some(id) = map_id {
        packet.var(7);
        packet.var(id);
    }
    packet
}

#[test]
fn rc1_map_slot_replacement_and_pixel_packets_update_received_state() {
    for player_inventory in [false, true] {
        let mut session = session();
        session.inventory.selected_slot = 2;
        let id = if player_inventory { cb::SET_PLAYER_INVENTORY } else { cb::CONTAINER_SET_SLOT };
        session.apply(Phase::Play, id, &slot_packet(player_inventory, 2, None).0).unwrap();
        assert_eq!(session.inventory.received_player_slot(38).unwrap().unwrap().id, 2);
        session.apply(Phase::Play, id, &slot_packet(player_inventory, 3, Some(0)).0).unwrap();
        let received = session.inventory.received_player_slot(38).unwrap().unwrap();
        assert_eq!(received.id, 3);
        assert_eq!(received.components["minecraft:map_id"], json!(0));
        assert_eq!(session.inventory.player.slots[38].as_ref().unwrap().id, 3);
        let mut map = Writer::default();
        map.var(0);
        map.byte(0); // scale
        map.byte(0); // unlocked
        map.byte(0); // decorations unchanged
        map.byte(2); // width
        map.byte(1); // height
        map.byte(4); // start x
        map.byte(5); // start y
        map.var(2);
        map.0.extend([17, 18]);
        session.apply(Phase::Play, cb::MAP_ITEM_DATA, &map.0).unwrap();
        let map = &session.ui.as_ref().unwrap().maps()[&0];
        assert_eq!(&map.colors[5 * 128 + 4..5 * 128 + 6], &[17, 18]);
        assert_eq!(map.revision, 1);
    }
}

#[test]
fn rc1_boss_remove_packet_removes_only_addressed_bar() {
    let mut session = session();
    for id in [1u128, 2] {
        let mut packet = Writer::default();
        packet.0.extend(id.to_be_bytes());
        packet.var(0);
        Nbt::String("Ender Dragon".into()).write_root(&mut packet).unwrap();
        packet.f32(1.);
        packet.var(0);
        packet.var(0);
        packet.byte(6);
        session.apply(Phase::Play, cb::BOSS_EVENT, &packet.0).unwrap();
    }
    let mut packet = Writer::default();
    packet.0.extend(1u128.to_be_bytes());
    packet.var(1);
    session.apply(Phase::Play, cb::BOSS_EVENT, &packet.0).unwrap();
    let bosses = session.ui.as_ref().unwrap().bosses();
    assert_eq!(bosses.len(), 1);
    assert!(bosses.contains_key(&uuid::Uuid::from_u128(2).to_string()));
    session.disconnect();
    assert!(session.ui.as_ref().unwrap().bosses().is_empty());
}
