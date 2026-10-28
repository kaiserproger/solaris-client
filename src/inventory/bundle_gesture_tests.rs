use super::*;
use crate::{
    inventory::{PacketCommand, read_stack},
    protocol::{Reader, packet_ids::play::serverbound},
    ui::{ClickMode, UiIntent},
};

const BUNDLE: u32 = 1;
const STONE: u32 = 2;
const CONTENTS: i32 = 10;
const SLOT: i16 = 37;

fn content() -> Content {
    let mut content = Content::test_cube();
    content.item_names.insert(BUNDLE, "minecraft:bundle".into());
    content
        .item_names
        .insert(STONE, "minecraft:cobblestone".into());
    content.insert_test_item_interaction(BUNDLE, "BundleItem", "BundleItem", "Item");
    content.insert_test_item_interaction(STONE, "Item", "Item", "Item");
    content.mark_test_bundle(BUNDLE);
    content.insert_test_item_components(
        BUNDLE,
        BTreeMap::from([
            ("minecraft:max_stack_size".into(), json!(1)),
            ("minecraft:bundle_contents".into(), json!([])),
        ]),
    );
    content.insert_test_item_components(
        STONE,
        BTreeMap::from([("minecraft:max_stack_size".into(), json!(64))]),
    );
    content
        .components
        .insert(CONTENTS, "minecraft:bundle_contents".into());
    content
}

fn stack(content: &Content, id: u32, count: i32) -> Stack {
    let mut wire = Writer::default();
    wire.var(count);
    wire.var(id as i32);
    wire.var(0);
    wire.var(0);
    read_stack(&mut Reader::new(&wire.0), content).unwrap()
}

fn filled_bundle(content: &Content, count: i32) -> Stack {
    let mut wire = Writer::default();
    wire.var(1);
    wire.var(BUNDLE as i32);
    wire.var(1);
    wire.var(0);
    wire.var(CONTENTS);
    wire.var(1);
    wire.var(STONE as i32);
    wire.var(count);
    wire.var(0);
    wire.var(0);
    read_stack(&mut Reader::new(&wire.0), content).unwrap()
}

fn store(slot: Stack, cursor: Stack) -> InventoryStore {
    let mut store = InventoryStore {
        screen_open: true,
        cursor: Some(cursor),
        ..InventoryStore::default()
    };
    store.player.state_id = 7;
    store.player.slots.fill(Some(Stack::default()));
    store.player.slots[SLOT as usize] = Some(slot);
    store
}

fn click(store: &mut InventoryStore, content: &Content, button: u8) -> PacketCommand {
    let intent = UiIntent::ClickSlot {
        context: store.context(),
        slot: SLOT,
        button,
        mode: ClickMode::Pickup,
    };
    let mut packets = store.execute_gesture(&[intent], content).unwrap();
    assert_eq!(packets.len(), 1);
    packets.pop().unwrap()
}

fn packet(packet: PacketCommand, button: u8, changed: Option<&Stack>, cursor: &Stack) {
    assert_eq!(packet.id, serverbound::CONTAINER_CLICK);
    let mut expected = Writer::default();
    expected.var(0);
    expected.var(7);
    expected.0.extend(SLOT.to_be_bytes());
    expected.byte(button);
    expected.var(0);
    expected.var(i32::from(changed.is_some()));
    if let Some(changed) = changed {
        expected.0.extend(SLOT.to_be_bytes());
        super::super::hashed(changed, &mut expected).unwrap();
    }
    super::super::hashed(cursor, &mut expected).unwrap();
    assert_eq!(packet.payload.0, expected.0);
}

#[test]
fn vanilla_bundle_slot_insert_then_secondary_extract() {
    let content = content();
    let empty = stack(&content, BUNDLE, 1);
    assert!(empty.patch.added.is_empty());
    assert_eq!(
        empty.component("minecraft:bundle_contents"),
        Some(&json!([]))
    );
    let mut store = store(empty.clone(), stack(&content, STONE, 10));
    let filled = filled_bundle(&content, 10);
    let command = click(&mut store, &content, 0);
    assert_eq!(store.player.slots[SLOT as usize].as_ref().unwrap(), &filled);
    assert!(store.cursor.as_ref().unwrap().empty());
    packet(command, 0, Some(&filled), &Stack::default());

    let command = click(&mut store, &content, 1);
    assert_eq!(store.player.slots[SLOT as usize].as_ref().unwrap(), &empty);
    assert_eq!(store.cursor.as_ref().unwrap().count, 10);
    packet(command, 1, Some(&empty), &stack(&content, STONE, 10));
}

#[test]
fn vanilla_cursor_bundle_insert_then_secondary_extract() {
    let content = content();
    let empty = stack(&content, BUNDLE, 1);
    let filled = filled_bundle(&content, 10);
    let mut store = store(stack(&content, STONE, 10), empty.clone());
    let command = click(&mut store, &content, 0);
    assert!(store.player.slots[SLOT as usize].as_ref().unwrap().empty());
    assert_eq!(store.cursor.as_ref().unwrap(), &filled);
    packet(command, 0, Some(&Stack::default()), &filled);

    let command = click(&mut store, &content, 1);
    assert_eq!(
        store.player.slots[SLOT as usize].as_ref().unwrap().count,
        10
    );
    assert_eq!(store.cursor.as_ref().unwrap(), &empty);
    packet(command, 1, Some(&stack(&content, STONE, 10)), &empty);
}

#[test]
fn vanilla_bundle_capacity_limits_insertion() {
    let content = content();
    let mut store = store(filled_bundle(&content, 60), stack(&content, STONE, 10));
    let command = click(&mut store, &content, 0);
    let filled = filled_bundle(&content, 64);
    assert_eq!(store.player.slots[SLOT as usize].as_ref().unwrap(), &filled);
    assert_eq!(store.cursor.as_ref().unwrap().count, 6);
    packet(command, 0, Some(&filled), &stack(&content, STONE, 6));
}

#[test]
fn removed_bundle_component_does_not_recreate_default() {
    let content = content();
    let mut wire = Writer::default();
    wire.var(1);
    wire.var(BUNDLE as i32);
    wire.var(0);
    wire.var(1);
    wire.var(CONTENTS);
    let removed = read_stack(&mut Reader::new(&wire.0), &content).unwrap();
    assert!(removed.component("minecraft:bundle_contents").is_none());
    assert_eq!(removed.patch.removed, vec![CONTENTS]);
    let stone = stack(&content, STONE, 10);
    let mut store = store(removed.clone(), stone.clone());
    let command = click(&mut store, &content, 0);
    assert_eq!(store.player.slots[SLOT as usize].as_ref().unwrap(), &stone);
    assert_eq!(store.cursor.as_ref().unwrap(), &removed);
    packet(command, 0, Some(&stone), &removed);
}

#[test]
fn secondary_with_occupied_bundle_and_cursor_swaps_normally() {
    let content = content();
    let bundle = stack(&content, BUNDLE, 1);
    let stone = stack(&content, STONE, 10);
    let mut store = store(bundle.clone(), stone.clone());
    let command = click(&mut store, &content, 1);
    assert_eq!(store.player.slots[SLOT as usize].as_ref().unwrap(), &stone);
    assert_eq!(store.cursor.as_ref().unwrap(), &bundle);
    packet(command, 1, Some(&stone), &bundle);
}
