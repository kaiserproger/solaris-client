//! Authored fixtures from the verified 26.3 component STREAM_CODEC field shapes.
use super::*;
use crate::protocol::Writer;
use std::collections::BTreeMap;

fn content() -> Content {
    let mut c = Content::test_cube();
    let mut registries = BTreeMap::new();
    for (registry, names) in [
        ("minecraft:block_transformer", vec!["test:axe"]),
        ("minecraft:decorated_pot_pattern", vec!["test:pattern"]),
        ("minecraft:entity_type", vec!["test:first", "test:second"]),
        ("minecraft:item", vec!["test:brick"]),
    ] {
        registries.insert(registry.into(), names.into_iter()
            .map(|name| (name.into(), serde_json::Value::Null)).collect());
    }
    c.set_registries(registries);
    c.item_names.insert(0, "test:brick".into());
    c.insert_test_item_components(0, BTreeMap::new());
    c.components.insert(120, "minecraft:waxed".into());
    c
}

fn fixture(name: &str, bytes: &[u8], expected: H) {
    let c = content();
    let mut r = Reader::new(bytes);
    let value = read(&mut r, &c, &format!("minecraft:{name}"), 0)
        .unwrap_or_else(|error| panic!("{name}: {error}"));
    r.done().unwrap_or_else(|error| panic!("{name}: {error}"));
    assert_eq!(value, expected, "{name}");
}

#[test]
fn components_263_animation_names_and_registry_values() {
    for name in ["attack_animation", "interact_animation"] {
        fixture(name, &[2, 130, 1], H::record([
            ("type", H::string("stab")), ("duration", H::Int(130))]));
        fixture(name, &[1, 6], H::Map(vec![]));
    }
    fixture("block_transformer", &[0], H::string("test:axe"));
    fixture("provides_pottery_pattern", &[0], H::string("test:pattern"));
    fixture("villager_food", &[130, 1], H::record([("nutrition", H::Int(130))]));
    fixture("waxed", &[], H::Map(vec![]));
    fixture("cushion/color", &[14], H::string("red"));
}

#[test]
fn components_263_resolvable_values_use_fixed_ints_and_floats() {
    fixture("compostable", &[1, 0, 0, 0, 2], H::record([("layers", H::Int(2))]));
    for (name, key) in [("cooking_fuel", "burn_time"), ("brewing_fuel", "uses")] {
        fixture(name, &[1, 0, 0, 6, 64, 1, 0x3f, 0xc0, 0, 0],
            H::record([(key, H::Int(1600)), ("speed_multiplier", H::Float(1.5))]));
        let mut bytes = Writer::default();
        bytes.byte(0);
        bytes.string("test:value");
        bytes.byte(0);
        bytes.string("test:speed");
        fixture(name, &bytes.0, H::record([
            (key, H::string("test:value")), ("speed_multiplier", H::string("test:speed"))]));
    }
}

#[test]
fn components_263_visibility_named_and_direct_entity_sets() {
    fixture("mob_visibility", &[3, 0, 1, 0x3f, 0, 0, 0], H::record([
        ("targeting_entity_types", H::List(vec![H::string("test:first"), H::string("test:second")])),
        ("visibility", H::Float(0.5))]));
    let mut bytes = Writer::default();
    bytes.var(0);
    bytes.string("test:targets");
    bytes.0.extend_from_slice(&[0x40, 0, 0, 0]);
    fixture("mob_visibility", &bytes.0, H::record([
        ("targeting_entity_types", H::string("#test:targets")), ("visibility", H::Float(2.))]));
}

#[test]
fn components_263_sign_text_has_four_fixed_lines_and_optional_filtered_lines() {
    for name in ["sign_text_front", "sign_text_back"] {
        for filtered in [false, true] {
            let mut bytes = Vec::new();
            for line in b"ABCD" { bytes.extend_from_slice(&[8, 0, 1, *line]); }
            bytes.push(u8::from(filtered));
            if filtered {
                for line in b"abcd" { bytes.extend_from_slice(&[8, 0, 1, *line]); }
            }
            bytes.extend_from_slice(&[11, 1]);
            let mut expected = vec![
                (H::string("messages"), H::List(["A", "B", "C", "D"].map(H::string).to_vec())),
                (H::string("color"), H::string("blue")),
                (H::string("has_glowing_text"), H::Bool(true)),
            ];
            if filtered { expected.insert(1, (H::string("filtered_messages"),
                H::List(["a", "b", "c", "d"].map(H::string).to_vec()))); }
            fixture(name, &bytes, H::Map(expected));
        }
    }
}

#[test]
fn components_263_pot_sides_and_sulfur_retain_nested_item_template_patches() {
    // Template item=0, count=2, one added component, none removed, waxed=120.
    let template = [0, 2, 1, 0, 120];
    let expected_item = H::record([
        ("id", H::string("test:brick")), ("count", H::Int(2)),
        ("components", H::record([("minecraft:waxed", H::Map(vec![]))])),
    ]);
    fixture("sulfur_cube_content", &template, expected_item.clone());
    let mut bytes = vec![1];
    bytes.extend_from_slice(&template);
    bytes.extend_from_slice(&[0, 0, 0]);
    fixture("pot_decorations", &bytes, H::record([("back", expected_item)]));
    fixture("pot_decorations", &[0, 0, 0, 0], H::Map(vec![]));
    let c = content();
    let mut reader = Reader::new(&bytes);
    let (_, stacks) = read_retained(&mut reader, &c, "minecraft:pot_decorations", 0).unwrap();
    reader.done().unwrap();
    assert_eq!(stacks.len(), 1);
    assert_eq!(stacks[0].count, 2);
    assert!(stacks[0].components.contains_key("minecraft:waxed"));
}
