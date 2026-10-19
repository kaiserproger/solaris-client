//! Exact native screen and button identity for the canonical Minecraft tools.
use super::{
    state::{EditorState, Screen, UiState},
    UiIntent,
};
use crate::{
    inventory::{InventoryStore, MenuKind},
    protocol::{content::Content, Result},
};
use serde_json::{json, Value};

impl UiState {
    pub(crate) fn visible_container(&self) -> bool {
        self.pending_link.is_none() && matches!(self.screen, Screen::Menu | Screen::Creative)
    }
    /// Source constructor title components, not inferred labels from the menu's contents.
    /// Unadmitted/loading state has no client screen identity available to tools.
    pub(crate) fn source_screen_metadata(
        &self,
        inventory: &InventoryStore,
    ) -> Result<Option<(&'static str, String)>> {
        let (class, title): (&str, Value) = if self.pending_link.is_some() {
            (
                "net.minecraft.client.gui.screens.ConfirmLinkScreen",
                json!({"translate":"chat.link.confirm"}),
            )
        } else {
            match self.screen {
                Screen::Game | Screen::Loading => return Ok(None),
                Screen::Loader => {
                    let view = self.loader.as_ref().ok_or("loader screen state missing")?;
                    return Ok(Some((
                        "dev.solaris.loader.minecraft.LoaderViewScreen",
                        view.title.clone(),
                    )));
                }
                Screen::Menu => {
                    let class = match inventory.menu().kind {
                        MenuKind::Inventory => {
                            "net.minecraft.client.gui.screens.inventory.InventoryScreen"
                        }
                        MenuKind::Chest(_) => {
                            "net.minecraft.client.gui.screens.inventory.ContainerScreen"
                        }
                        MenuKind::Dispenser => {
                            "net.minecraft.client.gui.screens.inventory.DispenserScreen"
                        }
                        MenuKind::Crafter => {
                            "net.minecraft.client.gui.screens.inventory.CrafterScreen"
                        }
                        MenuKind::Anvil => "net.minecraft.client.gui.screens.inventory.AnvilScreen",
                        MenuKind::Beacon => {
                            "net.minecraft.client.gui.screens.inventory.BeaconScreen"
                        }
                        MenuKind::BlastFurnace => {
                            "net.minecraft.client.gui.screens.inventory.BlastFurnaceScreen"
                        }
                        MenuKind::BrewingStand => {
                            "net.minecraft.client.gui.screens.inventory.BrewingStandScreen"
                        }
                        MenuKind::Crafting => {
                            "net.minecraft.client.gui.screens.inventory.CraftingScreen"
                        }
                        MenuKind::Enchantment => {
                            "net.minecraft.client.gui.screens.inventory.EnchantmentScreen"
                        }
                        MenuKind::Furnace => {
                            "net.minecraft.client.gui.screens.inventory.FurnaceScreen"
                        }
                        MenuKind::Grindstone => {
                            "net.minecraft.client.gui.screens.inventory.GrindstoneScreen"
                        }
                        MenuKind::Hopper => {
                            "net.minecraft.client.gui.screens.inventory.HopperScreen"
                        }
                        MenuKind::Lectern => {
                            "net.minecraft.client.gui.screens.inventory.LecternScreen"
                        }
                        MenuKind::Loom => "net.minecraft.client.gui.screens.inventory.LoomScreen",
                        MenuKind::Merchant => {
                            "net.minecraft.client.gui.screens.inventory.MerchantScreen"
                        }
                        MenuKind::ShulkerBox => {
                            "net.minecraft.client.gui.screens.inventory.ShulkerBoxScreen"
                        }
                        MenuKind::Smithing => {
                            "net.minecraft.client.gui.screens.inventory.SmithingScreen"
                        }
                        MenuKind::Smoker => {
                            "net.minecraft.client.gui.screens.inventory.SmokerScreen"
                        }
                        MenuKind::Cartography => {
                            "net.minecraft.client.gui.screens.inventory.CartographyTableScreen"
                        }
                        MenuKind::Stonecutter => {
                            "net.minecraft.client.gui.screens.inventory.StonecutterScreen"
                        }
                        MenuKind::Horse => match inventory.menu().mount_kind.as_deref() {
                            Some("minecraft:nautilus" | "minecraft:zombie_nautilus") => {
                                "net.minecraft.client.gui.screens.inventory.NautilusInventoryScreen"
                            }
                            Some(_) => {
                                "net.minecraft.client.gui.screens.inventory.HorseInventoryScreen"
                            }
                            None => return Err("mount screen identity not observed".into()),
                        },
                    };
                    let title = if inventory.menu().kind == MenuKind::Inventory {
                        json!({"translate":"container.crafting"})
                    } else {
                        inventory.menu().title.clone()
                    };
                    (class, title)
                }
                Screen::Creative => (
                    "net.minecraft.client.gui.screens.inventory.CreativeModeInventoryScreen",
                    json!({"text":""}),
                ),
                Screen::Chat => (
                    "net.minecraft.client.gui.screens.ChatScreen",
                    json!({"translate":"chat_screen.title"}),
                ),
                Screen::Book => {
                    if matches!(self.editor, Some(EditorState::Book { writable: true, .. })) {
                        (
                            "net.minecraft.client.gui.screens.inventory.BookEditScreen",
                            json!({"translate":"book.edit.title"}),
                        )
                    } else {
                        (
                            "net.minecraft.client.gui.screens.inventory.BookViewScreen",
                            json!({"translate":"book.view.title"}),
                        )
                    }
                }
                Screen::Sign => (
                    "net.minecraft.client.gui.screens.inventory.SignEditScreen",
                    json!({"translate":"sign.edit"}),
                ),
                Screen::Command => {
                    let class = if matches!(&self.editor, Some(EditorState::Command(command)) if command.entity_id.is_some())
                    {
                        "net.minecraft.client.gui.screens.inventory.MinecartCommandBlockEditScreen"
                    } else {
                        "net.minecraft.client.gui.screens.inventory.CommandBlockEditScreen"
                    };
                    (class, json!({"text":""}))
                }
                Screen::Structure => (
                    "net.minecraft.client.gui.screens.inventory.StructureBlockEditScreen",
                    json!({"translate":"block.minecraft.structure_block"}),
                ),
                Screen::Jigsaw => (
                    "net.minecraft.client.gui.screens.inventory.JigsawBlockEditScreen",
                    json!({"text":""}),
                ),
                Screen::Death => (
                    "net.minecraft.client.gui.screens.DeathScreen",
                    json!({"translate":"deathScreen.title"}),
                ),
                Screen::Pause => (
                    "net.minecraft.client.gui.screens.PauseScreen",
                    json!({"translate":"menu.game"}),
                ),
                Screen::Title => (
                    "net.minecraft.client.gui.screens.TitleScreen",
                    json!({"text":"Solaris"}),
                ),
                Screen::Multiplayer => (
                    "net.minecraft.client.gui.screens.multiplayer.JoinMultiplayerScreen",
                    json!({"text":"Play Multiplayer"}),
                ),
                Screen::DirectConnect => (
                    "net.minecraft.client.gui.screens.DirectJoinServerScreen",
                    json!({"text":"Direct Connect"}),
                ),
                Screen::Singleplayer => (
                    "net.minecraft.client.gui.screens.worldselection.SelectWorldScreen",
                    json!({"text":"Singleplayer"}),
                ),
                Screen::CreateWorld => (
                    "net.minecraft.client.gui.screens.worldselection.CreateWorldScreen",
                    json!({"text":"Create New World"}),
                ),
                Screen::DeleteWorld => (
                    "net.minecraft.client.gui.screens.ConfirmScreen",
                    json!({"text":"Delete this world?"}),
                ),
                Screen::SingleplayerStarting => (
                    "net.minecraft.client.gui.screens.GenericMessageScreen",
                    json!({"text":"Starting Singleplayer World"}),
                ),
                Screen::Disconnected => (
                    "net.minecraft.client.gui.screens.DisconnectedScreen",
                    json!({"text":"Connection Lost"}),
                ),
                Screen::Settings => (
                    "net.minecraft.client.gui.screens.options.OptionsScreen",
                    json!({"translate":"options.title"}),
                ),
                Screen::Accessibility => (
                    "net.minecraft.client.gui.screens.options.AccessibilityOptionsScreen",
                    json!({"translate":"options.accessibility.title"}),
                ),
                Screen::Advancements => (
                    "net.minecraft.client.gui.screens.advancements.AdvancementsScreen",
                    json!({"translate":"gui.advancements"}),
                ),
                Screen::Statistics => (
                    "net.minecraft.client.gui.screens.achievement.StatsScreen",
                    json!({"translate":"gui.stats","fallback":"Statistics"}),
                ),
            }
        };
        Ok(Some((class, self.text.language.plain(&title)?)))
    }
    pub(crate) fn exact_button(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
        expected_class: Option<&str>,
        expected_title: &str,
        label: &str,
        confirmation: bool,
    ) -> Result<UiIntent> {
        self.synchronize(inventory, content)?;
        if confirmation && self.pending_link.is_none() {
            return Err("current screen is not a confirmation screen".into());
        }
        let (class, title) = self
            .source_screen_metadata(inventory)?
            .ok_or("no client screen is open")?;
        if expected_class.is_some_and(|expected| expected != class) {
            return Err(format!("screen class mismatch: {class}"));
        }
        if title != expected_title {
            return Err(format!("screen title mismatch: {title}"));
        }
        // Buttons, options tabs, switches and sliders are all one-shot activations
        // through the same real path; a slider activation is its existing cycle.
        let mut matches = self.nodes.iter().filter(|node| {
            matches!(node.role, "button" | "tab" | "switch" | "slider")
                && node.label == label
                && (self.pending_link.is_none() || node.id.starts_with("link/"))
        });
        let button = matches.next().ok_or("screen button match count: 0")?;
        if matches.next().is_some() {
            return Err("screen button label is ambiguous".into());
        }
        if !button.enabled || !button.visible {
            return Err("screen button is not enabled".into());
        }
        Ok(UiIntent::Activate {
            context: self.context(inventory),
            element_id: button.id.clone(),
        })
    }
    /// Resolve one actual EditBuffer-backed visible textbox. Mutation stays in UiIntent.
    pub(crate) fn exact_input(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
        expected_class: &str,
        expected_title: &str,
        label: &str,
        value: &str,
    ) -> Result<(UiIntent, usize)> {
        self.synchronize(inventory, content)?;
        let (class, title) = self
            .source_screen_metadata(inventory)?
            .ok_or("no client screen is open")?;
        if class != expected_class {
            return Err(format!("screen class mismatch: {class}"));
        }
        if title != expected_title {
            return Err(format!("screen title mismatch: {title}"));
        }
        let (id, length) = exact_input_target(&self.nodes, &self.fields, label, value)?;
        Ok((
            UiIntent::Focus {
                context: self.context(inventory),
                element_id: id.into(),
            },
            length,
        ))
    }
}

fn exact_input_target<'a>(
    nodes: &'a [super::state::SemanticNode],
    fields: &std::collections::BTreeMap<String, super::edit::EditBuffer>,
    label: &str,
    value: &str,
) -> Result<(&'a str, usize)> {
    let mut matches = nodes
        .iter()
        .filter(|n| n.role == "textbox" && n.label == label && n.visible);
    let node = matches.next().ok_or("screen input match count: 0")?;
    if matches.next().is_some() {
        return Err("screen input label is ambiguous".into());
    }
    if !node.enabled {
        return Err("screen input is not enabled".into());
    }
    if !matches!(&node.action, super::state::NodeAction::Focus(id) if id == &node.id)
        || !matches!(&node.paint, super::state::Paint::Field(id) if id == &node.id)
    {
        return Err("screen input is not editable".into());
    }
    let field = fields.get(&node.id).ok_or("screen input is not editable")?;
    if value.len() > field.max_bytes {
        return Err("screen input exceeds native byte limit; truncation refused".into());
    }
    if value
        .chars()
        .any(|c| c == '\0' || (!field.multiline && matches!(c, '\n' | '\r')))
    {
        return Err("invalid_text_input".into());
    }
    Ok((&node.id, field.text.len()))
}

#[cfg(test)]
mod tests {
    use super::super::{
        edit::EditBuffer,
        state::{NodeAction, Paint, SemanticNode},
    };
    use super::*;
    use std::collections::BTreeMap;
    fn node(id: &str) -> SemanticNode {
        SemanticNode {
            id: id.into(),
            role: "textbox",
            label: "Name".into(),
            component: None,
            bounds: [8., 8., 160., 20.],
            enabled: true,
            visible: true,
            deny_reason: None,
            focused: false,
            owner: "solaris_loader",
            action: NodeAction::Focus(id.into()),
            paint: Paint::Field(id.into()),
        }
    }
    fn statistics_fixture() -> (Content, InventoryStore, UiState) {
        let mut content = Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        // Synthetic registry order, independent of the pinned wire IDs.
        content.set_registries(BTreeMap::from([
            (
                "minecraft:stat_type".into(),
                ["minecraft:custom", "minecraft:used", "minecraft:killed"]
                    .into_iter()
                    .map(|name| (name.into(), Value::Null))
                    .collect(),
            ),
            (
                "minecraft:custom_stat".into(),
                [
                    "minecraft:walk_one_cm",
                    "minecraft:play_time",
                    "minecraft:damage_dealt",
                    "minecraft:jump",
                ]
                .into_iter()
                .map(|name| (name.into(), Value::Null))
                .collect(),
            ),
            (
                "minecraft:item".into(),
                vec![("minecraft:stone".into(), Value::Null)],
            ),
            (
                "minecraft:entity_type".into(),
                vec![("minecraft:pig".into(), Value::Null)],
            ),
        ]));
        let ui = UiState::new_state(&content).unwrap();
        (content, InventoryStore::default(), ui)
    }
    fn click_statistics_button(
        ui: &mut UiState,
        inventory: &mut InventoryStore,
        content: &Content,
        label: &str,
    ) -> super::super::state::UiEffects {
        let (class, title) = ui.source_screen_metadata(inventory).unwrap().unwrap();
        let intent = ui
            .exact_button(inventory, content, Some(class), &title, label, false)
            .unwrap();
        ui.execute(intent, inventory, content).unwrap()
    }
    fn award_statistics(ui: &mut UiState, content: &Content, entries: &[(i32, i32, i32)]) {
        let mut writer = crate::protocol::Writer::default();
        writer.var(entries.len() as i32);
        for &(kind, stat, count) in entries {
            writer.var(kind);
            writer.var(stat);
            writer.var(count);
        }
        assert!(ui
            .apply(3, &mut crate::protocol::Reader::new(&writer.0), content)
            .unwrap());
    }
    fn statistics_row_labels(ui: &UiState) -> Vec<String> {
        ui.observed_nodes()
            .into_iter()
            .filter(|node| node["id"].as_str().unwrap().starts_with("statistics/row/"))
            .map(|node| node["label"].as_str().unwrap().to_owned())
            .collect()
    }
    fn advancement_fixture() -> (Content, InventoryStore, UiState) {
        use super::super::packets::AdvancementState;
        let (content, inventory, mut ui) = statistics_fixture();
        for (id, label) in [
            ("minecraft:adventure/root", "Adventure"),
            ("minecraft:story/root", "Minecraft"),
            ("minecraft:nether/root", "Nether"),
            ("minecraft:end/root", "The End"),
            ("minecraft:husbandry/root", "Husbandry"),
            ("custom:root", "Custom"),
        ] {
            ui.advancements.insert(
                id.into(),
                AdvancementState {
                    parent: None,
                    display: Some(json!({"text":label})),
                    description: None,
                    icon: None,
                    background: None,
                    frame: 0,
                    flags: 0,
                    position: [0.; 2],
                    requirements: Vec::new(),
                    progress: BTreeMap::new(),
                    done: false,
                    telemetry: false,
                },
            );
        }
        for index in 0..30 {
            let mut row = ui.advancements["minecraft:story/root"].clone();
            row.parent = Some("minecraft:story/root".into());
            row.display = Some(json!({"text":format!("Story row {index:02}")}));
            ui.advancements
                .insert(format!("minecraft:story/row_{index:02}"), row);
        }
        (content, inventory, ui)
    }
    #[test]
    fn p1_advancements_completed_rows_use_ascii_done_marker() {
        let (content, mut inventory, mut ui) = advancement_fixture();
        ui.advancements
            .get_mut("minecraft:story/row_00")
            .unwrap()
            .done = true;
        ui.execute(UiIntent::OpenPause, &mut inventory, &content)
            .unwrap();
        click_statistics_button(&mut ui, &mut inventory, &content, "Advancements");
        click_statistics_button(&mut ui, &mut inventory, &content, "Minecraft");
        let nodes = ui.observed_nodes();
        let completed = nodes
            .iter()
            .find(|node| node["id"] == "advancement/minecraft:story/row_00")
            .unwrap();
        let pending = nodes
            .iter()
            .find(|node| node["id"] == "advancement/minecraft:story/row_01")
            .unwrap();
        assert_eq!(completed["label"], "[Done] Story row 00");
        assert_eq!(pending["label"], "Story row 01");
        assert!(completed["label"].as_str().unwrap().is_ascii());
    }

    #[test]
    fn p1_advancements_exact_tabs_scroll_and_close_share_headless_ui_path() {
        use super::super::state::UiCommand;
        let (content, mut inventory, mut ui) = advancement_fixture();
        // Auto scale 2 preserves this test's original 534x300 logical viewport.
        ui.resize(1068, 600, 1.);
        ui.execute(UiIntent::OpenPause, &mut inventory, &content)
            .unwrap();
        assert!(!ui.text.can_render());
        let open = click_statistics_button(&mut ui, &mut inventory, &content, "Advancements");
        assert!(matches!(open.commands.as_slice(),
            [UiCommand::SeenAdvancements { tab: Some(id) }] if id == "custom:root"));
        assert_eq!(ui.screen(), Screen::Advancements);
        for root in ui.advancement_tab_ids() {
            assert!(ui
                .nodes
                .iter()
                .any(|node| node.id == format!("advancement/tab/{root}")));
        }
        for (label, root) in [
            ("Adventure", "minecraft:adventure/root"),
            ("Minecraft", "minecraft:story/root"),
        ] {
            let effects = click_statistics_button(&mut ui, &mut inventory, &content, label);
            assert!(matches!(effects.commands.as_slice(),
                [UiCommand::SeenAdvancements { tab: Some(id) }] if id == root));
            assert_eq!(ui.selected_advancement_root(), Some(root));
            assert_eq!(ui.scroll, 0);
        }
        assert!(ui
            .nodes
            .iter()
            .any(|node| node.id == "advancement/minecraft:story/row_00"));
        assert!(!ui
            .nodes
            .iter()
            .any(|node| node.id == "advancement/minecraft:adventure/root"));
        let list = ui
            .nodes
            .iter()
            .find(|node| node.id == "advancements/list")
            .unwrap();
        assert_eq!(list.role, "list");
        ui.execute(
            UiIntent::ScrollTarget {
                context: ui.context(&inventory),
                element_id: "advancements/list".into(),
                delta_x: 0.,
                delta_y: -10000.,
            },
            &mut inventory,
            &content,
        )
        .unwrap();
        let (_, viewport) = ui.advancement_list_viewport();
        assert_eq!(ui.scroll, 31usize.saturating_sub(viewport));
        assert!(ui
            .nodes
            .iter()
            .any(|node| node.id == "advancement/minecraft:story/row_29"));
        let row = ui
            .nodes
            .iter()
            .find(|node| node.id == "advancement/minecraft:story/row_29")
            .unwrap();
        let x = f64::from(row.bounds[0] + 2.) * ui.scale;
        let y = f64::from(row.bounds[1] + 2.) * ui.scale;
        ui.execute(
            UiIntent::Scroll {
                x,
                y,
                delta_x: 0.,
                delta_y: 10000.,
                modifiers: Default::default(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
        assert_eq!(ui.scroll, 0);
        assert!(ui
            .nodes
            .iter()
            .any(|node| node.id == "advancement/minecraft:story/row_00"));
        ui.execute(
            UiIntent::ScrollTarget {
                context: ui.context(&inventory),
                element_id: "advancements/list".into(),
                delta_x: 0.,
                delta_y: -5.,
            },
            &mut inventory,
            &content,
        )
        .unwrap();
        assert!(ui.scroll > 0);
        for (label, root) in [
            ("Nether", "minecraft:nether/root"),
            ("The End", "minecraft:end/root"),
            ("Husbandry", "minecraft:husbandry/root"),
            ("Custom", "custom:root"),
        ] {
            let effects = click_statistics_button(&mut ui, &mut inventory, &content, label);
            assert!(matches!(effects.commands.as_slice(),
                [UiCommand::SeenAdvancements { tab: Some(id) }] if id == root));
            assert_eq!(ui.scroll, 0);
            assert!(ui
                .nodes
                .iter()
                .any(|node| node.id == format!("advancement/{root}")));
        }
        let done = click_statistics_button(&mut ui, &mut inventory, &content, "Done");
        assert!(matches!(
            done.commands.as_slice(),
            [UiCommand::SeenAdvancements { tab: None }]
        ));
        assert_eq!(ui.screen(), Screen::Pause);
        assert!(!ui
            .nodes
            .iter()
            .any(|node| node.id.starts_with("advancement")));
        click_statistics_button(&mut ui, &mut inventory, &content, "Advancements");
        let close = ui
            .execute(
                UiIntent::Close {
                    context: ui.context(&inventory),
                },
                &mut inventory,
                &content,
            )
            .unwrap();
        assert!(matches!(
            close.commands.as_slice(),
            [UiCommand::SeenAdvancements { tab: None }]
        ));
        assert!(!ui
            .nodes
            .iter()
            .any(|node| node.id.starts_with("advancement")));
    }
    #[test]
    fn p1_advancements_received_selected_tab_rebuilds_without_outbound_echo() {
        let (content, mut inventory, mut ui) = advancement_fixture();
        ui.execute(UiIntent::OpenPause, &mut inventory, &content)
            .unwrap();
        click_statistics_button(&mut ui, &mut inventory, &content, "Advancements");
        click_statistics_button(&mut ui, &mut inventory, &content, "Minecraft");
        ui.execute(
            UiIntent::ScrollTarget {
                context: ui.context(&inventory),
                element_id: "advancements/list".into(),
                delta_x: 0.,
                delta_y: -5.,
            },
            &mut inventory,
            &content,
        )
        .unwrap();
        let mut packet = crate::protocol::Writer::default();
        packet.byte(1);
        packet.string("minecraft:adventure/root");
        assert!(ui
            .apply(85, &mut crate::protocol::Reader::new(&packet.0), &content)
            .unwrap());
        ui.semantic_tree(&inventory, &content).unwrap();
        assert_eq!(
            ui.selected_advancement_root(),
            Some("minecraft:adventure/root")
        );
        assert_eq!(ui.scroll, 0);
        assert!(ui
            .nodes
            .iter()
            .any(|node| node.id == "advancement/minecraft:adventure/root"));
        assert!(!ui
            .nodes
            .iter()
            .any(|node| node.id.starts_with("advancement/minecraft:story/")));
    }
    #[test]
    fn exact_button_activates_options_tabs_switches_and_sliders() {
        use super::super::state::UiCommand;
        let (content, mut inventory, mut ui) = statistics_fixture();
        ui.resize(427, 240, 1.0);
        ui.transition(Screen::Settings);
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let (class, title) = ui.source_screen_metadata(&inventory).unwrap().unwrap();
        let label_of = |ui: &UiState, id: &str, role: &str| {
            let node = ui.nodes.iter().find(|n| n.id == id).unwrap();
            assert_eq!(node.role, role, "{id}");
            node.label.clone()
        };
        let settings = |effects: &super::super::state::UiEffects, key: &str| {
            effects
                .commands
                .iter()
                .filter(|c| matches!(c, UiCommand::Settings { key: k, .. } if k == key))
                .count()
        };
        // Switch: toggles through the same Activate path.
        let label = label_of(&ui, "settings/vsync", "switch");
        let intent = ui
            .exact_button(&inventory, &content, Some(class), &title, &label, false)
            .unwrap();
        assert!(
            matches!(&intent, UiIntent::Activate { element_id, .. } if element_id == "settings/vsync")
        );
        let effects = ui.execute(intent, &mut inventory, &content).unwrap();
        assert_eq!(settings(&effects, "vsync"), 1);
        // Slider: one existing cycle step per activation.
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let label = label_of(&ui, "settings/fov", "slider");
        let intent = ui
            .exact_button(&inventory, &content, Some(class), &title, &label, false)
            .unwrap();
        let effects = ui.execute(intent, &mut inventory, &content).unwrap();
        assert_eq!(settings(&effects, "fov"), 1);
        // Tab: swaps the visible row set.
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let label = label_of(&ui, "options/tab/2", "tab");
        let intent = ui
            .exact_button(&inventory, &content, Some(class), &title, &label, false)
            .unwrap();
        assert!(
            matches!(&intent, UiIntent::Activate { element_id, .. } if element_id == "options/tab/2")
        );
        ui.execute(intent, &mut inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert!(ui.nodes.iter().any(|n| n.id == "settings/volume"));
        assert!(!ui.nodes.iter().any(|n| n.id == "settings/fov"));
    }
    #[test]
    fn p1_statistics_exact_button_opens_real_screen_and_requests_once_headlessly() {
        use super::super::state::UiCommand;
        let (content, mut inventory, mut ui) = statistics_fixture();
        ui.resize(854, 480, 1.6);
        ui.execute(UiIntent::OpenPause, &mut inventory, &content)
            .unwrap();
        assert!(!ui.text.can_render());
        let tree = ui.semantic_tree(&inventory, &content).unwrap();
        for node in tree["nodes"].as_array().unwrap() {
            if node["role"] == "button" {
                let bounds = node["bounds"].as_array().unwrap();
                assert!(
                    bounds[1].as_f64().unwrap() + bounds[3].as_f64().unwrap()
                        <= f64::from(ui.height)
                );
            }
        }
        for id in [
            "resume",
            "advancements",
            "statistics",
            "settings",
            "accessibility",
            "disconnect",
        ] {
            assert!(ui
                .nodes
                .iter()
                .any(|node| node.id == format!("pause/{id}") && node.enabled && node.visible));
        }
        let (_, title) = ui.source_screen_metadata(&inventory).unwrap().unwrap();
        assert!(ui
            .exact_button(
                &inventory,
                &content,
                Some("wrong.class"),
                &title,
                "Statistics",
                false
            )
            .is_err());
        assert!(ui
            .exact_button(
                &inventory,
                &content,
                None,
                "wrong title",
                "Statistics",
                false
            )
            .is_err());
        assert!(ui
            .exact_button(&inventory, &content, None, &title, "not a button", false)
            .is_err());
        let intent = ui
            .exact_button(
                &inventory,
                &content,
                Some("net.minecraft.client.gui.screens.PauseScreen"),
                &title,
                "Statistics",
                false,
            )
            .unwrap();
        assert!(
            matches!(&intent, UiIntent::Activate { element_id, .. } if element_id == "pause/statistics")
        );
        let effects = ui.execute(intent, &mut inventory, &content).unwrap();
        assert!(matches!(
            effects.commands.as_slice(),
            [UiCommand::RequestStatistics]
        ));
        assert_eq!(ui.screen(), Screen::Statistics);
        assert!(ui.dialog_screen_active());
        assert_eq!(
            ui.source_screen_metadata(&inventory).unwrap(),
            Some((
                "net.minecraft.client.gui.screens.achievement.StatsScreen",
                "Statistics".into()
            ))
        );
        assert!(!ui.statistics().received());
        for _ in 0..2 {
            ui.semantic_tree(&inventory, &content).unwrap();
        }
        ui.resize(1280, 720, 1.5);
        ui.semantic_tree(&inventory, &content).unwrap();
        assert!(ui
            .activate_for_test(
                NodeAction::Screen(Screen::Statistics),
                &mut inventory,
                &content
            )
            .unwrap()
            .commands
            .is_empty());
        assert!(
            click_statistics_button(&mut ui, &mut inventory, &content, "Items")
                .commands
                .is_empty()
        );
        assert!(ui
            .observed_nodes()
            .iter()
            .any(|node| node["id"] == "statistics/waiting"));
        assert!(statistics_row_labels(&ui).is_empty());
    }
    #[test]
    fn p1_statistics_received_row_count_labels_and_tab_switch_use_real_nodes() {
        use super::super::statistics::StatisticsGroup;
        let (content, mut inventory, mut ui) = statistics_fixture();
        ui.execute(UiIntent::OpenPause, &mut inventory, &content)
            .unwrap();
        click_statistics_button(&mut ui, &mut inventory, &content, "Statistics");
        award_statistics(
            &mut ui,
            &content,
            &[
                (0, 0, 125000),
                (0, 1, 72000),
                (0, 2, 37),
                (1, 0, 1234),
                (2, 0, 2),
            ],
        );
        ui.semantic_tree(&inventory, &content).unwrap();
        assert!(ui.statistics().received());
        assert!(!ui
            .observed_nodes()
            .iter()
            .any(|node| node["id"] == "statistics/waiting"));
        let labels = statistics_row_labels(&ui);
        for label in [
            "stat.minecraft.walk_one_cm: 1.25 km",
            "stat.minecraft.play_time: 1.00 h",
            "stat.minecraft.damage_dealt: 3.70",
            "stat.minecraft.jump: 0",
        ] {
            assert!(labels.iter().any(|actual| actual == label), "{labels:?}");
        }
        assert!(
            click_statistics_button(&mut ui, &mut inventory, &content, "Items")
                .commands
                .is_empty()
        );
        assert_eq!(ui.statistics_group, StatisticsGroup::Items);
        assert_eq!(
            statistics_row_labels(&ui),
            ["stat_type.minecraft.used: minecraft:stone: 1,234"]
        );
        ui.scroll = 10;
        assert!(
            click_statistics_button(&mut ui, &mut inventory, &content, "Mobs")
                .commands
                .is_empty()
        );
        assert_eq!(ui.scroll, 0);
        assert_eq!(
            statistics_row_labels(&ui),
            ["stat_type.minecraft.killed: minecraft:pig: 2"]
        );
        award_statistics(&mut ui, &content, &[(2, 0, 0)]);
        ui.semantic_tree(&inventory, &content).unwrap();
        assert_eq!(
            statistics_row_labels(&ui),
            ["stat_type.minecraft.killed: minecraft:pig: 0"]
        );
        click_statistics_button(&mut ui, &mut inventory, &content, "Items");
        assert_eq!(
            statistics_row_labels(&ui),
            ["stat_type.minecraft.used: minecraft:stone: 1,234"]
        );
    }
    #[test]
    fn p1_statistics_waiting_response_and_screen_session_lifecycle() {
        use super::super::{state::UiCommand, statistics::StatisticsGroup, Modifiers};
        let (content, mut inventory, mut ui) = statistics_fixture();
        ui.execute(UiIntent::OpenPause, &mut inventory, &content)
            .unwrap();
        click_statistics_button(&mut ui, &mut inventory, &content, "Statistics");
        let revision = ui.revision;
        assert!(ui
            .apply(3, &mut crate::protocol::Reader::new(&[1]), &content)
            .is_err());
        assert_eq!(ui.revision, revision);
        assert!(!ui.statistics().received());
        award_statistics(&mut ui, &content, &[(0, 2, 37)]);
        ui.semantic_tree(&inventory, &content).unwrap();
        assert!(
            click_statistics_button(&mut ui, &mut inventory, &content, "Done")
                .commands
                .is_empty()
        );
        assert_eq!(ui.screen(), Screen::Pause);
        let effects = click_statistics_button(&mut ui, &mut inventory, &content, "Statistics");
        assert!(matches!(
            effects.commands.as_slice(),
            [UiCommand::RequestStatistics]
        ));
        assert!(!ui.statistics().received());
        assert!(statistics_row_labels(&ui).is_empty());
        assert!(ui.statistics().observation(&content).unwrap()["counters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|counter| counter["value"] == 37));
        // Empty AwardStats is still a response and retains earlier absolute counters.
        award_statistics(&mut ui, &content, &[]);
        ui.semantic_tree(&inventory, &content).unwrap();
        assert!(statistics_row_labels(&ui)
            .iter()
            .any(|label| label == "stat.minecraft.damage_dealt: 3.70"));
        let effects = ui
            .execute(
                UiIntent::Key {
                    key: "Escape".into(),
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
                &mut inventory,
                &content,
            )
            .unwrap();
        assert!(effects.commands.is_empty());
        assert_eq!(ui.screen(), Screen::Pause);
        click_statistics_button(&mut ui, &mut inventory, &content, "Back to game");
        assert_eq!(ui.screen(), Screen::Game);
        assert!(ui.semantic_tree(&inventory, &content).unwrap()["nodes"]
            .as_array()
            .unwrap()
            .is_empty());
        ui.execute(UiIntent::OpenPause, &mut inventory, &content)
            .unwrap();
        click_statistics_button(&mut ui, &mut inventory, &content, "Statistics");
        click_statistics_button(&mut ui, &mut inventory, &content, "Mobs");
        ui.clear_session();
        assert_eq!(ui.screen(), Screen::Game);
        assert_eq!(ui.statistics_group, StatisticsGroup::General);
        assert!(!ui.statistics().received());
        assert!(ui.statistics().observation(&content).unwrap()["counters"]
            .as_array()
            .unwrap()
            .is_empty());
        assert!(ui.observed_nodes().is_empty());
    }
    #[test]
    fn exact_screen_input_refuses_ambiguous_disabled_hidden_and_noneditable_targets() {
        let fields = BTreeMap::from([(
            "field".into(),
            EditBuffer::new("old".into(), 8, false).unwrap(),
        )]);
        let mut n = node("field");
        assert!(exact_input_target(&[n.clone(), node("other")], &fields, "Name", "new").is_err());
        n.enabled = false;
        assert!(exact_input_target(&[n.clone()], &fields, "Name", "new").is_err());
        n.enabled = true;
        n.visible = false;
        assert!(exact_input_target(&[n.clone()], &fields, "Name", "new").is_err());
        n.visible = true;
        n.action = NodeAction::None;
        assert!(exact_input_target(&[n], &fields, "Name", "new").is_err());
    }
    #[test]
    fn exact_screen_input_bounds_utf8_without_truncation_and_preserves_rejected_value() {
        let nodes = [node("field")];
        let mut fields = BTreeMap::from([(
            "field".into(),
            EditBuffer::new("old".into(), 8, false).unwrap(),
        )]);
        assert!(exact_input_target(&nodes, &fields, "Name", "ééééé").is_err());
        assert!(exact_input_target(&nodes, &fields, "Name", "new\n").is_err());
        assert_eq!(fields["field"].text, "old");
        let (id, length) = exact_input_target(&nodes, &fields, "Name", "éééé").unwrap();
        let edit = fields.get_mut(id).unwrap();
        edit.select(0, length).unwrap();
        edit.insert("éééé").unwrap();
        assert_eq!(edit.text, "éééé");
        assert_eq!(edit.cursor, 8);
    }
}
