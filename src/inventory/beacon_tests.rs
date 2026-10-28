use super::*;
use crate::protocol::packet_ids::play::serverbound as sb;
use crate::ui::{ClickMode, UiIntent};

fn content() -> Content {
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    content.item_names.insert(1, "minecraft:iron_ingot".into());
    content.insert_test_item_components(
        1,
        BTreeMap::from([("minecraft:max_stack_size".into(), json!(64))]),
    );
    content.set_registries(BTreeMap::from([
        (
            "minecraft:item".into(),
            vec![
                ("minecraft:air".into(), json!({})),
                ("minecraft:iron_ingot".into(), json!({})),
            ],
        ),
        (
            "minecraft:mob_effect".into(),
            [
                "speed",
                "haste",
                "resistance",
                "jump_boost",
                "strength",
                "regeneration",
            ]
            .into_iter()
            .map(|name| (format!("minecraft:{name}"), json!({})))
            .collect(),
        ),
    ]));
    content
        .set_tags(BTreeMap::from([(
            "minecraft:item".into(),
            BTreeMap::from([
                ("minecraft:beacon_payment_items".into(), vec![1]),
                ("minecraft:bundles".into(), vec![]),
            ]),
        )]))
        .unwrap();
    content
}

fn apply(store: &mut InventoryStore, content: &Content, id: i32, bytes: &[u8]) {
    assert!(store.apply(id, &mut Reader::new(bytes), content).unwrap());
}

fn open(store: &mut InventoryStore, content: &Content) {
    // Window 0: 46 slots, hotbar slot 0 contains a single authored ingot, empty cursor.
    let mut player = vec![0, 3, 46];
    player.extend([0; 36]);
    player.extend([1, 1, 0, 0]);
    player.extend([0; 9]);
    player.push(0);
    apply(store, content, cb::CONTAINER_SET_CONTENT, &player);
    // Beacon screen with network-NBT string title, followed by level data.
    apply(store, content, cb::OPEN_SCREEN, &[1, 9, 8, 0, 1, b'B']);
    apply(store, content, cb::CONTAINER_SET_DATA, &[1, 0, 0, 0, 1]);
}

#[test]
fn beacon_open_predicts_local_slots_and_emits_payment_and_effect_packets() {
    let content = content();
    let mut store = InventoryStore::default();
    open(&mut store, &content);
    assert_eq!(store.menu().slots[28].as_ref().unwrap().id, 1);
    assert!(store.menu().slots[0].as_ref().unwrap().empty());
    assert!(store.received_menu().slots[0].is_none());
    assert!(store.received_menu().slots[28].is_none());

    let click = |store: &mut InventoryStore, slot| {
        let intent = UiIntent::ClickSlot {
            context: store.context(),
            slot,
            button: 0,
            mode: ClickMode::Pickup,
        };
        store.execute_gesture(&[intent], &content).unwrap()
    };
    let first = click(&mut store, 28);
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].id, sb::CONTAINER_CLICK);
    assert_eq!(
        first[0].payload.0,
        [1, 0, 0, 28, 0, 0, 1, 0, 28, 0, 1, 1, 1, 0, 0]
    );
    let second = click(&mut store, 0);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id, sb::CONTAINER_CLICK);
    assert_eq!(
        second[0].payload.0,
        [1, 0, 0, 0, 0, 0, 1, 0, 0, 1, 1, 1, 0, 0, 0]
    );
    let intent = UiIntent::Beacon {
        context: store.context(),
        primary: Some(0),
        secondary: None,
    };
    let commands = store.execute_gesture(&[intent], &content).unwrap();
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0].id, sb::SET_BEACON);
    assert_eq!(commands[0].payload.0, [1, 0, 0]);
    assert_eq!(commands[1].id, sb::CONTAINER_CLOSE);
    assert_eq!(commands[1].payload.0, [1]);
}

#[test]
fn beacon_server_updates_override_prediction_without_fabricating_observation() {
    let content = content();
    let mut store = InventoryStore::default();
    open(&mut store, &content);
    // A subsequent authoritative slot delta overrides the constructor prediction.
    apply(
        &mut store,
        &content,
        cb::CONTAINER_SET_SLOT,
        &[1, 4, 0, 0, 1, 1, 0, 0],
    );
    assert_eq!(store.menu().slots[0].as_ref().unwrap().id, 1);
    assert_eq!(store.received_menu().slots[0].as_ref().unwrap().id, 1);
    // An authoritative full image is allowed to clear both payment and player tail.
    assert_eq!(store.received_player_slot(36).unwrap().unwrap().id, 1);
    let mut image = vec![1, 5, 37];
    image.extend([0; 37]);
    image.push(0);
    apply(&mut store, &content, cb::CONTAINER_SET_CONTENT, &image);
    assert!(store.menu().slots[0].as_ref().unwrap().empty());
    assert!(store.menu().slots[28].as_ref().unwrap().empty());
    assert!(store.received_menu().slots[28].as_ref().unwrap().empty());
    assert!(store.received_player_slot(36).unwrap().unwrap().empty());
}

#[test]
fn beacon_semantic_ui_exposes_player_payment_and_effect_confirmation() {
    use crate::ui::{UiCommand, UiState};
    let content = content();
    let mut store = InventoryStore::default();
    open(&mut store, &content);
    let mut ui = UiState::new_state(&content).unwrap();
    ui.resize(1280, 720, 1.);
    ui.synchronize(&store, &content).unwrap();
    ui.rebuild_for_test(&store, &content).unwrap();
    assert_eq!(
        ui.observed_nodes()
            .iter()
            .filter(|node| node["role"] == "slot" && node["enabled"] == true)
            .count(),
        37
    );
    let mut packets = Vec::new();
    for element in ["slot/28", "slot/0", "beacon/primary/0", "beacon/confirm"] {
        let context = ui.context(&store);
        let effects = ui
            .execute(
                UiIntent::Activate {
                    context,
                    element_id: element.into(),
                },
                &mut store,
                &content,
            )
            .unwrap();
        for command in effects.commands {
            if let UiCommand::Inventory(intent) = command {
                packets.extend(store.execute(&intent, &content).unwrap());
            }
        }
        ui.synchronize(&store, &content).unwrap();
        ui.rebuild_for_test(&store, &content).unwrap();
    }
    assert_eq!(
        packets.iter().map(|packet| packet.id).collect::<Vec<_>>(),
        [
            sb::CONTAINER_CLICK,
            sb::CONTAINER_CLICK,
            sb::SET_BEACON,
            sb::CONTAINER_CLOSE
        ]
    );
    assert_eq!(packets[2].payload.0, [1, 0, 0]);
    assert_eq!(packets[3].payload.0, [1]);
}
