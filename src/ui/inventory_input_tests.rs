use super::*;
use crate::inventory::{InventoryStore, PacketCommand, Stack};
use crate::protocol::{Reader, content::Content, packet_ids::play::serverbound as sb};
use std::{collections::BTreeMap, sync::Arc};
use winit::keyboard::KeyCode;

type EncodedStack = Option<(i32, i32)>;
type SlotChanges = BTreeMap<i16, EncodedStack>;

fn fixture() -> (Content, InventoryStore, UiState) {
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    content.item_names.insert(0, "minecraft:stone".into());
    content.insert_test_item_interaction(0, "Item", "Item", "Item");
    content.insert_test_item_components(
        0,
        BTreeMap::from([("minecraft:max_stack_size".into(), serde_json::json!(64))]),
    );
    content.set_registries(BTreeMap::from([(
        "minecraft:item".into(),
        vec![("minecraft:stone".into(), serde_json::json!({}))],
    )]));
    content
        .set_tags(BTreeMap::from([(
            "minecraft:item".into(),
            BTreeMap::from([("minecraft:bundles".into(), vec![])]),
        )]))
        .unwrap();
    let mut inventory = InventoryStore::default();
    inventory.player.slots.fill(Some(Stack::default()));
    inventory.cursor = Some(Stack::default());
    inventory.open_inventory().unwrap();
    let mut ui = UiState::new_state(&content).unwrap();
    ui.resize(1280, 720, 1.);
    ui.synchronize(&inventory, &content).unwrap();
    ui.rebuild_for_test(&inventory, &content).unwrap();
    (content, inventory, ui)
}
fn stack(count: i32) -> Stack {
    let mut stack = Stack::default();
    stack.id = 0;
    stack.count = count;
    stack.components = Arc::new(BTreeMap::from([(
        "minecraft:max_stack_size".into(),
        serde_json::json!(64),
    )]));
    stack
}
fn dispatch(
    ui: &mut UiState,
    inv: &mut InventoryStore,
    content: &Content,
    intent: UiIntent,
) -> Vec<PacketCommand> {
    let effects = ui.execute(intent, inv, content).unwrap();
    let mut packets = Vec::new();
    for command in effects.commands {
        match command {
            UiCommand::Inventory(intent) => packets.extend(inv.execute(&intent, content).unwrap()),
            UiCommand::InventoryGesture(intents) => {
                packets.extend(inv.execute_gesture(&intents, content).unwrap())
            }
            _ => {}
        }
    }
    ui.synchronize(inv, content).unwrap();
    ui.rebuild_for_test(inv, content).unwrap();
    packets
}
fn point(ui: &UiState, slot: usize) -> (f64, f64) {
    let n = ui
        .observed_nodes()
        .into_iter()
        .find(|n| n["id"] == format!("slot/{slot}"))
        .unwrap();
    let b = n["bounds"].as_array().unwrap();
    (
        b[0].as_f64().unwrap() + b[2].as_f64().unwrap() / 2.,
        b[1].as_f64().unwrap() + b[3].as_f64().unwrap() / 2.,
    )
}
fn pointer(
    ui: &mut UiState,
    inv: &mut InventoryStore,
    content: &Content,
    slot: usize,
    phase: PointerPhase,
) -> Vec<PacketCommand> {
    let (x, y) = point(ui, slot);
    dispatch(
        ui,
        inv,
        content,
        UiIntent::Pointer {
            phase,
            x,
            y,
            button: Some(PointerButton::Left),
            modifiers: Modifiers::default(),
        },
    )
}
fn hashed(r: &mut Reader<'_>) -> EncodedStack {
    if !r.bool().unwrap() {
        return None;
    }
    let item = r.var().unwrap();
    let count = r.var().unwrap();
    for _ in 0..r.var().unwrap() {
        r.var().unwrap();
        r.i32().unwrap();
    }
    for _ in 0..r.var().unwrap() {
        r.var().unwrap();
    }
    Some((item, count))
}
fn click(
    packet: &PacketCommand,
    slot: i16,
    button: u8,
    mode: i32,
) -> (SlotChanges, EncodedStack) {
    assert_eq!(packet.id, sb::CONTAINER_CLICK);
    let mut r = Reader::new(&packet.payload.0);
    assert_eq!(r.var().unwrap(), 0);
    assert_eq!(r.var().unwrap(), 0);
    assert_eq!(r.i16().unwrap(), slot);
    assert_eq!(r.u8().unwrap(), button);
    assert_eq!(r.var().unwrap(), mode);
    let mut changes = BTreeMap::new();
    for _ in 0..r.var().unwrap() {
        let index = r.i16().unwrap();
        changes.insert(index, hashed(&mut r));
    }
    let cursor = hashed(&mut r);
    r.done().unwrap();
    (changes, cursor)
}
#[test]
fn inventory_double_click_collects_matching_stacks_on_cursor() {
    let (content, mut inv, mut ui) = fixture();
    inv.player.slots[9] = Some(stack(10));
    inv.player.slots[10] = Some(stack(20));
    let first = pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Down);
    click(&first[0], 9, 0, 0);
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Up);
    let mut second = pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Down);
    second.extend(pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Up));
    assert_eq!(second.len(), 1);
    let (changed, carried) = click(&second[0], 9, 0, 6);
    assert_eq!(changed, BTreeMap::from([(10, None)]));
    assert_eq!(carried, Some((0, 30)));
    assert!(inv.player.slots[10].as_ref().unwrap().empty());
}
#[test]
fn inventory_native_number_keys_and_offhand_swap_hovered_stack() {
    for (code, button, destination) in [
        (KeyCode::Digit1, 0, 36),
        (KeyCode::Digit2, 1, 37),
        (KeyCode::Digit3, 2, 38),
        (KeyCode::Digit4, 3, 39),
        (KeyCode::Digit5, 4, 40),
        (KeyCode::Digit6, 5, 41),
        (KeyCode::Digit7, 6, 42),
        (KeyCode::Digit8, 7, 43),
        (KeyCode::Digit9, 8, 44),
        (KeyCode::KeyF, 40, 45),
    ] {
        let (content, mut inv, mut ui) = fixture();
        inv.player.slots[9] = Some(stack(10));
        pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Move);
        let native = crate::native_item_key(code, true, Modifiers::default(), true);
        let mut packets = Vec::new();
        if let Some(crate::api::InputIntent::Ui { intent }) = native {
            packets = dispatch(&mut ui, &mut inv, &content, intent);
        }
        assert_eq!(
            packets.len(),
            1,
            "native {code:?} must enter container click path"
        );
        let (changed, carried) = click(&packets[0], 9, button, 2);
        assert_eq!(
            changed,
            BTreeMap::from([(9, None), (destination, Some((0, 10)))])
        );
        assert_eq!(carried, None);
    }
}
#[test]
fn inventory_native_q_and_control_q_throw_hovered_items() {
    for (control, remaining, button) in [(false, 9, 0), (true, 0, 1)] {
        let (content, mut inv, mut ui) = fixture();
        inv.player.slots[9] = Some(stack(10));
        pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Move);
        let modifiers = Modifiers {
            control,
            ..Modifiers::default()
        };
        let mut packets = Vec::new();
        if let Some(crate::api::InputIntent::Ui { intent }) =
            crate::native_item_key(KeyCode::KeyQ, true, modifiers, true)
        {
            packets = dispatch(&mut ui, &mut inv, &content, intent);
        }
        assert_eq!(
            packets.len(),
            1,
            "Q control={control} must enter container click path"
        );
        let (changed, carried) = click(&packets[0], 9, button, 4);
        assert_eq!(
            changed,
            BTreeMap::from([(
                9,
                if remaining == 0 {
                    None
                } else {
                    Some((0, remaining))
                }
            )])
        );
        assert_eq!(carried, None);
    }
}
#[test]
fn inventory_recipe_book_button_opens_shared_panel_and_sends_settings() {
    let (content, mut inv, mut ui) = fixture();
    let context = ui.context(&inv);
    let packets = dispatch(
        &mut ui,
        &mut inv,
        &content,
        UiIntent::Activate {
            context,
            element_id: "recipe/book".into(),
        },
    );
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].id, sb::RECIPE_BOOK_CHANGE_SETTINGS);
    assert_eq!(packets[0].payload.0, [0, 1, 0]);
    assert!(
        ui.observed_nodes()
            .iter()
            .any(|n| n["id"] == "recipe/search")
    );
}

#[test]
fn inventory_native_shortcuts_preserve_gameplay_and_ignore_releases() {
    assert!(matches!(
        crate::native_item_key(KeyCode::Digit2, true, Modifiers::default(), false),
        Some(crate::api::InputIntent::Hotbar { slot: 1 })
    ));
    assert!(matches!(
        crate::native_item_key(KeyCode::KeyQ, true, Modifiers::default(), false),
        Some(crate::api::InputIntent::Key { key, pressed: true }) if key == "drop"
    ));
    for key in [KeyCode::Digit1, KeyCode::KeyQ, KeyCode::KeyF] {
        assert!(crate::native_item_key(key, false, Modifiers::default(), true).is_none());
    }
}

#[test]
fn inventory_clicking_a_different_slot_places_instead_of_collecting() {
    let (content, mut inv, mut ui) = fixture();
    inv.player.slots[9] = Some(stack(10));
    inv.player.slots[11] = Some(stack(20));
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Down);
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Up);
    pointer(&mut ui, &mut inv, &content, 10, PointerPhase::Down);
    let packets = pointer(&mut ui, &mut inv, &content, 10, PointerPhase::Up);
    let (changed, carried) = click(&packets[0], 10, 0, 0);
    assert_eq!(changed, BTreeMap::from([(10, Some((0, 10)))]));
    assert_eq!(carried, None);
    assert_eq!(inv.player.slots[11].as_ref().unwrap().count, 20);
}

#[test]
fn inventory_swapping_while_carrying_does_not_change_slots() {
    let (content, mut inv, mut ui) = fixture();
    inv.player.slots[9] = Some(stack(10));
    inv.cursor = Some(stack(3));
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Move);
    let Some(crate::api::InputIntent::Ui { intent }) =
        crate::native_item_key(KeyCode::Digit2, true, Modifiers::default(), true)
    else {
        panic!("inventory key must reach UI");
    };
    assert!(dispatch(&mut ui, &mut inv, &content, intent).is_empty());
    assert_eq!(inv.player.slots[9].as_ref().unwrap().count, 10);
    assert_eq!(inv.cursor.as_ref().unwrap().count, 3);
}

#[test]
fn inventory_double_click_caps_collection_and_release_clears_click_history() {
    let (content, mut inv, mut ui) = fixture();
    inv.player.slots[9] = Some(stack(10));
    inv.player.slots[10] = Some(stack(60));
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Down);
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Up);
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Down);
    let packets = pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Up);
    let (changed, carried) = click(&packets[0], 9, 0, 6);
    assert_eq!(changed, BTreeMap::from([(10, Some((0, 6)))]));
    assert_eq!(carried, Some((0, 64)));
    ui.release_all();
    pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Down);
    let packets = pointer(&mut ui, &mut inv, &content, 9, PointerPhase::Up);
    let (_, carried) = click(&packets[0], 9, 0, 0);
    assert_eq!(carried, None);
}
