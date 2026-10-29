use super::*;
use crate::{
    inventory::{Recipe, Stack},
    ui::{Modifiers, PointerPhase, UiIntent},
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn inventory_recipe_category_labels_fit_without_wrapping_and_remain_selectable() {
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
    // Authored font definition: the renderer's ordinary missing-glyph metrics
    // provide a conservative six-pixel advance without Mojang resources/GPU.
    content.insert_test_font("minecraft:default", &json!({"providers":[]}));
    let mut inventory = InventoryStore::default();
    inventory.player.slots.fill(Some(Stack::default()));
    inventory.cursor = Some(Stack::default());
    inventory.open_inventory().unwrap();
    for (id, category) in [
        "crafting_building_blocks",
        "crafting_equipment",
        "crafting_redstone",
        "crafting_misc",
    ]
    .into_iter()
    .enumerate()
    {
        inventory.recipes.insert(
            id as i32,
            Recipe {
                id: id as i32,
                display: Arc::new(json!({"result":{"type":"minecraft:empty"}})),
                group: None,
                category: format!("minecraft:{category}"),
                ingredients: None,
                flags: 0,
                display_stacks: Vec::new(),
                ingredient_items: None,
            },
        );
    }
    let mut ui = UiState::new_state(&content).unwrap();
    ui.reset_resources(&content).unwrap();
    ui.resize(1280, 720, 1.);
    ui.synchronize(&inventory, &content).unwrap();
    let context = ui.context(&inventory);
    ui.execute(
        UiIntent::Activate {
            context,
            element_id: "recipe/book".into(),
        },
        &mut inventory,
        &content,
    )
    .unwrap();
    for width in [1280, 960] {
        ui.resize(width, 720, 1.);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let categories: Vec<_> = ui
            .nodes
            .iter()
            .filter(|node| node.id.starts_with("recipe/category/"))
            .cloned()
            .collect();
        assert_eq!(categories.len(), 5);
        let search = ui
            .nodes
            .iter()
            .find(|node| node.id == "recipe/search")
            .unwrap()
            .bounds;
        for node in categories {
            let [x, y, w, h] = node.bounds;
            let plain = json!({"text":node.label});
            let layout = ui
                .text
                .layout(node.component.as_deref().unwrap_or(&plain), w - 8.)
                .unwrap();
            assert!(
                layout.height <= h - 8.,
                "{} wraps beyond its button: {} > {}",
                node.id,
                layout.height,
                h - 8.
            );
            assert!(layout.width <= w - 8.);
            assert!(x >= 0. && y >= 0. && x + w <= ui.width && y + h <= ui.height);
            assert!(x + w <= search[0], "category overlaps recipe search/grid");
            let context = ui.context(&inventory);
            let category = node.id.strip_prefix("recipe/category/").unwrap();
            ui.execute(
                UiIntent::Activate {
                    context,
                    element_id: node.id.clone(),
                },
                &mut inventory,
                &content,
            )
            .unwrap();
            assert_eq!(
                ui.recipe_category.as_deref(),
                (!category.is_empty()).then_some(category)
            );
        }
    }
}

#[test]
fn inventory_creative_tab_labels_fit_without_wrapping_and_keep_full_semantics() {
    let mut content = Content::test_cube();
    let titles = [
        "Building Blocks",
        "Colored Blocks",
        "Natural Blocks",
        "Functional Blocks",
        "Redstone Blocks",
        "Saved Hotbars",
        "Search Items",
        "Tools & Utilities",
        "Combat",
        "Food & Drinks",
        "Ingredients",
        "Spawn Eggs",
        "Survival Inventory",
    ];
    let tabs: Vec<_> = titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            json!({
                "id":format!("minecraft:tab_{index}"), "title_component":{"text":title},
                "type":"CATEGORY", "items":[]
            })
        })
        .collect();
    content.creative_tabs_report = Arc::from(json!({"tabs":tabs}).to_string().into_bytes());
    content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
    content.insert_test_font("minecraft:default", &json!({"providers":[]}));
    let mut inventory = InventoryStore::default();
    inventory.game_mode = 1;
    inventory.player.slots.fill(Some(Stack::default()));
    inventory.open_inventory().unwrap();
    let mut ui = UiState::new_state(&content).unwrap();
    ui.reset_resources(&content).unwrap();
    for width in [1280, 960] {
        ui.resize(width, 720, 1.);
        ui.synchronize(&inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        for (index, title) in titles.iter().enumerate() {
            let node = ui
                .nodes
                .iter()
                .find(|node| node.id == format!("creative/tab/minecraft:tab_{index}"))
                .unwrap();
            assert_eq!(node.label, *title);
            let plain = json!({"text":node.label});
            let layout = ui
                .text
                .layout(
                    node.component.as_deref().unwrap_or(&plain),
                    node.bounds[2] - 8.,
                )
                .unwrap();
            assert!(
                layout.height <= node.bounds[3] - 8.,
                "{title} wraps outside its tab"
            );
            assert!(layout.width <= node.bounds[2] - 8.);
        }
    }
}

#[test]
fn creative_tab_hover_shows_full_title_and_hides_on_leave() {
    let mut content = Content::test_cube();
    let titles = [
        "Building Blocks",
        "Colored Blocks",
        "Natural Blocks",
        "Functional Blocks",
        "Redstone Blocks",
        "Saved Hotbars",
        "Search Items",
        "Tools & Utilities",
        "Combat",
        "Food & Drinks",
        "Ingredients",
        "Spawn Eggs",
        "Survival Inventory",
    ];
    let tabs: Vec<_> = titles
        .iter()
        .enumerate()
        .map(|(index, title)| {
            json!({
                "id": format!("minecraft:tab_{index}"),
                "title_component": {"text": title},
                "type": "CATEGORY",
                "items": []
            })
        })
        .collect();
    content.creative_tabs_report = Arc::from(json!({"tabs": tabs}).to_string().into_bytes());
    content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
    content.insert_test_font("minecraft:default", &json!({"providers":[]}));
    let mut inventory = InventoryStore::default();
    inventory.game_mode = 1;
    inventory.player.slots.fill(Some(Stack::default()));
    inventory.open_inventory().unwrap();
    let mut ui = UiState::new_state(&content).unwrap();
    ui.reset_resources(&content).unwrap();
    ui.resize(1280, 720, 1.);
    ui.synchronize(&inventory, &content).unwrap();

    let mut baseline = crate::api::MeshGeometry::default();
    ui.render(&content, &inventory, &mut baseline).unwrap();
    for index in [0, 3] {
        let node = ui.nodes.iter()
            .find(|node| node.id == format!("creative/tab/minecraft:tab_{index}"))
            .unwrap();
        assert_eq!(node.label, titles[index]);
        let painted = ui.text.language.plain(node.component.as_deref().unwrap()).unwrap();
        assert!(painted.ends_with('…') && painted != titles[index]);
        let [x, y, w, h] = node.bounds;
        ui.execute(
            UiIntent::Pointer {
                phase: PointerPhase::Move,
                x: f64::from((x + w / 2.) * ui.scale as f32),
                y: f64::from((y + h / 2.) * ui.scale as f32),
                button: None,
                modifiers: Modifiers::default(),
            },
            &mut inventory,
            &content,
        ).unwrap();
        let tree = ui.semantic_tree(&inventory, &content).unwrap();
        let nodes = tree["nodes"].as_array().unwrap();
        let tooltip = nodes.iter().find(|node| node["id"] == "component/hover/text").unwrap();
        assert_eq!(tooltip["label"], titles[index]);
        assert!(nodes.iter().any(|node| node["id"] == "component/hover/background"));
        let mut hovered = crate::api::MeshGeometry::default();
        ui.render(&content, &inventory, &mut hovered).unwrap();
        assert!(hovered.vertices.len() > baseline.vertices.len());

        ui.execute(
            UiIntent::Pointer {
                phase: PointerPhase::Move,
                x: 0.,
                y: 0.,
                button: None,
                modifiers: Modifiers::default(),
            },
            &mut inventory,
            &content,
        ).unwrap();
        let tree = ui.semantic_tree(&inventory, &content).unwrap();
        assert!(!tree["nodes"].as_array().unwrap().iter().any(|node| {
            node["id"] == "component/hover/text" || node["id"] == "component/hover/background"
        }));
        let mut left = crate::api::MeshGeometry::default();
        ui.render(&content, &inventory, &mut left).unwrap();
        assert_eq!(left.vertices.len(), baseline.vertices.len());
    }
}
