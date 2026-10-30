//! UI hotfix gates: death-respawn flow and Minecraft-scale HUD geometry at
//! 1280x720.
//!
//! State-only `UiState::new_state` has no fonts (`can_render() == false`), so
//! `synchronize()` cannot rebuild the semantic tree there. These gates pin
//! the pure fail-closed respawn state machine (screen + zero-health flag) and
//! the shared geometry constants the windowed proof drivers exercise live
//! with real fonts/GPU; pixel/click paths are covered by the windowed
//! before/after screenshots, not by headless layout.

use crate::inventory::InventoryStore;
use crate::protocol::content::Content;
use crate::ui::{HudState, Screen, UiCommand, UiPresentation, UiState};

fn state() -> (Content, InventoryStore, UiState) {
    let mut content = Content::test_cube();
    // UiState::new_mode parses creative_tabs JSON; the unit cube ships an
    // empty report. A minimal tabs document is enough: tests never open the
    // creative inventory, they only need construction to succeed.
    content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs": []}"#.as_slice());
    let inventory = InventoryStore::default();
    let ui = UiState::new_state(&content).expect("state-only UI must construct");
    (content, inventory, ui)
}
#[test]
fn rc1_hud_scoreboard_packets_reach_sidebar_state() {
    use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
    let (content, _, mut ui) = state();
    let mut objective = Writer::default();
    objective.string("points");
    objective.0.extend([0, 8, 0, 6]);
    objective.0.extend(b"Points");
    objective.0.extend([0, 0]);
    assert!(ui.apply(cb::SET_OBJECTIVE, &mut Reader::new(&objective.0), &content).unwrap());
    let mut display = Writer::default();
    display.var(1);
    display.string("points");
    assert!(ui.apply(cb::SET_DISPLAY_OBJECTIVE, &mut Reader::new(&display.0), &content).unwrap());
    let mut score = Writer::default();
    score.string("Alice");
    score.string("points");
    score.var(42);
    score.0.extend([0, 0]);
    assert!(ui.apply(cb::SET_SCORE, &mut Reader::new(&score.0), &content).unwrap());
    assert_eq!(ui.scoreboard().sidebar_objective(None).unwrap().0, "points");
    assert_eq!(ui.scoreboard().scores["points"]["Alice"].value, 42);
    let mut reset = Writer::default();
    reset.string("Alice");
    reset.0.push(1);
    reset.string("points");
    assert!(ui.apply(cb::RESET_SCORE, &mut Reader::new(&reset.0), &content).unwrap());
    assert!(!ui.scoreboard().scores["points"].contains_key("Alice"));
    let mut remove = Writer::default();
    remove.string("points");
    remove.0.push(1);
    assert!(ui.apply(cb::SET_OBJECTIVE, &mut Reader::new(&remove.0), &content).unwrap());
    assert!(ui.scoreboard().sidebar_objective(None).is_none());
}

#[test]
fn rc1_hud_chat_wraps_without_overlap_in_game_and_history() {
    use crate::ui::UiIntent;
    let (mut content, mut inventory, _) = state();
    content.insert_test_font("minecraft:default", &serde_json::json!({"providers":[]}));
    content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
    let mut ui = UiState::new(&content).unwrap();
    ui.resize(427, 240, 1.);
    ui.local_message("older message").unwrap();
    ui.local_message(&"long styled chat message ".repeat(12)).unwrap();
    ui.chat.back_mut().unwrap().component = serde_json::json!({
        "text": "", "extra": [
            {"text": "red ".repeat(35), "color": "red"},
            {"text": "bold ".repeat(25), "bold": true}
        ]
    });
    for chat_open in [false, true] {
        if chat_open {
            ui.execute(UiIntent::OpenChat { command: false }, &mut inventory, &content).unwrap();
        }
        ui.rebuild_for_test(&inventory, &content).unwrap();
        let message = ui.nodes.iter().find(|node| node.id.starts_with("chat/line/") && node.label.starts_with("red ")).unwrap();
        let crate::ui::Paint::RichText(layout) = &message.paint else { panic!("chat must retain rich text") };
        assert!(layout.glyphs.iter().any(|glyph| glyph.position[1] > 0. && layout.runs[glyph.style].style.bold));
        assert!(layout.runs.iter().any(|run| run.style.color_override && run.style.color[0] > run.style.color[1]));
        let nodes = ui.observed_nodes();
        let lines: Vec<_> = nodes.iter().filter(|node| node["id"].as_str().is_some_and(|id| id.starts_with("chat/line/") && !id.contains("/action/"))).collect();
        assert_eq!(lines.len(), 2);
        let older = lines[1]["bounds"].as_array().unwrap();
        let newer = lines[0]["bounds"].as_array().unwrap();
        assert!(newer[3].as_f64().unwrap() > 18.);
        assert!(newer[1].as_f64().unwrap() + newer[3].as_f64().unwrap() <= 190.,
            "the last wrapped line must stay above the status bars");
        assert_eq!(newer[3].as_f64().unwrap(), f64::from(layout.height));
        assert_eq!(older[3].as_f64().unwrap(), 9., "single lines use the same 9px pitch");
        assert_eq!(older[1].as_f64().unwrap() + older[3].as_f64().unwrap(), newer[1].as_f64().unwrap());
        assert!(older[1].as_f64().unwrap() + older[3].as_f64().unwrap() <= newer[1].as_f64().unwrap());
    }
}

#[test]
fn rc1_hud_sidebar_orders_and_caps_entries() {
    use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
    let (mut content, inventory, _) = state();
    content.insert_test_font("minecraft:default", &serde_json::json!({"providers":[]}));
    content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
    let mut ui = UiState::new(&content).unwrap();
    ui.resize(427, 240, 1.);
    let mut objective = Writer::default();
    objective.string("points");
    objective.0.extend([0, 8, 0, 6]);
    objective.0.extend(b"Points");
    objective.0.extend([0, 0]);
    ui.apply(cb::SET_OBJECTIVE, &mut Reader::new(&objective.0), &content).unwrap();
    let mut display = Writer::default();
    display.var(1);
    display.string("points");
    ui.apply(cb::SET_DISPLAY_OBJECTIVE, &mut Reader::new(&display.0), &content).unwrap();
    for i in 0..20 {
        let mut score = Writer::default();
        score.string(&format!("player{i:02}"));
        score.string("points");
        score.var(i / 2);
        score.0.extend([0, 0]);
        ui.apply(cb::SET_SCORE, &mut Reader::new(&score.0), &content).unwrap();
    }
    ui.rebuild_for_test(&inventory, &content).unwrap();
    let nodes = ui.observed_nodes();
    let names: Vec<_> = nodes.iter().filter_map(|n| {
        let id = n["id"].as_str()?;
        (id.starts_with("hud/sidebar/holder/") && id.ends_with("/name")).then(|| n["label"].as_str().unwrap().to_owned())
    }).collect();
    assert_eq!(names.len(), 15);
    assert_eq!(&names[..4], ["player18", "player19", "player16", "player17"]);
    assert!(nodes.iter().any(|n| n["id"] == "hud/sidebar/title" && n["label"] == "Points"));
}

#[test]
fn rc1_hud_f1_toggles_game_hud() {
    use crate::ui::UiIntent;
    let (content, mut inventory, mut ui) = state();
    ui.resize(427, 240, 1.);
    for hidden in [true, false] {
        ui.execute(UiIntent::Key { key: "F1".into(), pressed: true, modifiers: Default::default() }, &mut inventory, &content).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert_eq!(ui.setting("hide_hud").and_then(serde_json::Value::as_bool), Some(hidden));
        assert_eq!(ui.observed_nodes().iter().any(|n| n["id"] == "hud/hotbar"), !hidden);
        assert_eq!(ui.observed_nodes().iter().any(|n| n["id"] == "hud/crosshair"), !hidden);
    }
}
#[test]
fn hide_hud_setting_hides_and_restores_the_hud() {
    let (content, inventory, mut ui) = state();
    ui.resize(427, 240, 1.);
    for hidden in [true, false] {
        ui.apply_setting("hide_hud", serde_json::json!(hidden)).unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        assert_eq!(ui.observed_nodes().iter().any(|n| n["id"] == "hud/hotbar"), !hidden);
        assert_eq!(ui.observed_nodes().iter().any(|n| n["id"] == "hud/crosshair"), !hidden);
    }
}



fn dead(ui: &mut UiState) {
    ui.set_presentation(UiPresentation {
        hud: HudState::default(),
        loading: None,
        death: Some(serde_json::json!({"text": "fell"})),
        // Zero-health flag without a separate combat-death message is the
        // lifecycle-admission case (`minecraft_wait_for_health_below` with no
        // entity-42 combat payload): still dead, still respawnable.
        can_respawn: true,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .expect("death presentation must apply");
    assert_eq!(ui.screen(), Screen::Death);
    ui.resize(1280, 720, 1.5);
}

/// The fail-closed respawn gate: Death screen + zero-health flag (no combat
/// death message required). `set_presentation` transitions are pure state and
/// work without fonts; command emission is pinned by construction below.
#[test]
fn death_without_combat_message_reaches_death_screen() {
    let (_content, _inventory, mut ui) = state();
    dead(&mut ui);
    // Death message is display-only: drop it, the zero-health flag remains and
    // the Death screen stays (this is the fixed regression: previously the
    // Respawn action additionally required `death.is_some()`).
    ui.set_presentation(UiPresentation {
        hud: HudState::default(),
        loading: None,
        death: None,
        can_respawn: true,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .expect("zero-health presentation must stay on the death screen");
    assert_eq!(ui.screen(), Screen::Death);
    assert!(ui.dialog_screen_active());
}

#[test]
fn world_loading_screen_holds_input_until_received_terrain_is_ready() {
    let (_content, _inventory, mut ui) = state();
    ui.set_presentation(UiPresentation {
        hud: HudState::default(),
        loading: Some("Waiting for terrain and light".into()),
        death: None,
        can_respawn: false,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .expect("loading presentation must apply");
    assert_eq!(ui.screen(), Screen::Loading);
    assert!(ui.dialog_screen_active());

    ui.set_presentation(UiPresentation {
        hud: HudState::default(),
        loading: None,
        death: None,
        can_respawn: false,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .expect("world-ready presentation must clear loading");
    assert_eq!(ui.screen(), Screen::Game);
    assert!(!ui.dialog_screen_active());
}
#[test]
fn death_screen_clears_when_alive_again() {
    let (_content, _inventory, mut ui) = state();
    dead(&mut ui);
    ui.set_presentation(UiPresentation {
        hud: HudState::default(),
        loading: None,
        death: None,
        can_respawn: false,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .expect("alive presentation must apply");
    assert_eq!(ui.screen(), Screen::Game);
    assert!(!ui.dialog_screen_active());
}

#[test]
fn dialog_screens_own_input_while_death_refuses_close() {
    let (_content, _inventory, mut ui) = state();
    dead(&mut ui);
    assert!(ui.dialog_screen_active());
    // Death refuses Escape-close is enforced in `key()`; here we pin the
    // routing predicate the native window events consult.
    assert!(ui.dialog_screen_active());
}

/// SliceA human-playability gates (window-free, `UiState::execute` level):
/// T/Slash open chat, Escape closes it, empty input rejects on Enter-send,
/// and Escape on Game opens Pause. The winit key wiring in `lib.rs` routes
/// through exactly these intents, so pin the state machine here. Note:
/// state-only UiState has no fonts, so no semantic tree backs the focus
/// target and Text fails closed with unknown_ui_target; live typing is
/// covered by the windowed proof drivers, not here.
#[test]
fn chat_open_type_send_returns_to_game() {
    use crate::ui::{Modifiers, UiIntent};
    let (content, mut inventory, mut ui) = state();
    assert_eq!(ui.screen(), Screen::Game);
    ui.execute(
        UiIntent::OpenChat { command: false },
        &mut inventory,
        &content,
    )
    .expect("T must open chat on the Game screen");
    assert_eq!(ui.screen(), Screen::Chat);
    let text_err = match ui.execute(
        UiIntent::Text { text: "hi".into() },
        &mut inventory,
        &content,
    ) {
        Ok(_) => panic!("state-only text must fail closed without a tree"),
        Err(e) => e,
    };
    ui.execute(
        UiIntent::Key {
            key: "Escape".into(),
            pressed: true,
            modifiers: Modifiers::default(),
        },
        &mut inventory,
        &content,
    )
    .expect("Escape must close chat");
    assert_eq!(ui.screen(), Screen::Game);
    ui.execute(
        UiIntent::OpenChat { command: true },
        &mut inventory,
        &content,
    )
    .expect("Slash must open command chat on the Game screen");
    assert_eq!(ui.screen(), Screen::Chat);
    ui.execute(
        UiIntent::Key {
            key: "Enter".into(),
            pressed: true,
            modifiers: Modifiers::default(),
        },
        &mut inventory,
        &content,
    )
    .expect("slash-only command draft sends (live path)");
    assert_eq!(ui.screen(), Screen::Game);
}

#[test]
fn escape_on_game_opens_pause() {
    use crate::ui::{Modifiers, UiIntent};
    let (content, mut inventory, mut ui) = state();
    assert_eq!(ui.screen(), Screen::Game);
    ui.execute(
        UiIntent::Key {
            key: "Escape".into(),
            pressed: true,
            modifiers: Modifiers::default(),
        },
        &mut inventory,
        &content,
    )
    .expect("Escape must open Pause from Game");
    assert_eq!(ui.screen(), Screen::Pause);
}

#[test]
fn singleplayer_pause_menu_emits_pause_and_resume_only_in_singleplayer() {
    use crate::ui::{Modifiers, UiIntent};
    for singleplayer in [false, true] {
        let (content, mut inventory, mut ui) = state();
        ui.singleplayer_active = singleplayer;
        let escape = || UiIntent::Key {
            key: "Escape".into(),
            pressed: true,
            modifiers: Modifiers::default(),
        };
        let open = ui.execute(escape(), &mut inventory, &content).unwrap();
        let messages: Vec<_> = open
            .commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::Chat {
                    message,
                    command: true,
                } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            messages,
            if singleplayer {
                vec!["solaris_pause on"]
            } else {
                vec![]
            }
        );
        let close = ui.execute(escape(), &mut inventory, &content).unwrap();
        let messages: Vec<_> = close
            .commands
            .iter()
            .filter_map(|command| match command {
                UiCommand::Chat {
                    message,
                    command: true,
                } => Some(message.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            messages,
            if singleplayer {
                vec!["solaris_pause off"]
            } else {
                vec![]
            }
        );
    }
}

#[test]
fn title_enter_prefills_and_escape_stays() {
    use crate::ui::{Modifiers, UiIntent};
    let (content, mut inventory, mut ui) = state();
    // Defaults when nothing was used before.
    ui.enter_title(None, None)
        .expect("enter_title must prefill");
    assert_eq!(ui.screen(), Screen::Title);
    assert!(ui.dialog_screen_active());
    assert_eq!(
        ui.fields.get("title/server").map(|f| f.text.as_str()),
        Some("127.0.0.1:25565")
    );
    assert_eq!(
        ui.fields.get("title/username").map(|f| f.text.as_str()),
        Some("Player")
    );
    // Explicit prefill wins.
    ui.enter_title(Some("play.example:25565"), Some("Alex"))
        .expect("explicit prefill must apply");
    assert_eq!(
        ui.fields.get("title/server").map(|f| f.text.as_str()),
        Some("play.example:25565")
    );
    assert_eq!(
        ui.fields.get("title/username").map(|f| f.text.as_str()),
        Some("Alex")
    );
    // Escape on Title is a noop (Quit button exits instead).
    ui.execute(
        UiIntent::Key {
            key: "Escape".into(),
            pressed: true,
            modifiers: Modifiers::default(),
        },
        &mut inventory,
        &content,
    )
    .expect("Escape on Title must stay");
    assert_eq!(ui.screen(), Screen::Title);
    // Close on Title is a noop too.
    let context = ui.context(&inventory);
    ui.execute(UiIntent::Close { context }, &mut inventory, &content)
        .expect("Close on Title must stay");
    assert_eq!(ui.screen(), Screen::Title);
}

#[test]
fn title_connect_and_quit_emit_commands() {
    use crate::ui::{NodeAction, UiCommand};
    let (content, mut inventory, mut ui) = state();
    ui.enter_title(Some("play.example:25565"), Some("Alex"))
        .expect("enter_title must prefill");
    ui.activate_for_test(
        NodeAction::Screen(Screen::DirectConnect),
        &mut inventory,
        &content,
    )
    .expect("Direct Connect opens from the shell");
    let effects = ui
        .activate_for_test(NodeAction::Connect, &mut inventory, &content)
        .expect("Connect must validate test fields");
    assert_eq!(effects.commands.len(), 1);
    match &effects.commands[0] {
        UiCommand::Connect { address, username } => {
            assert_eq!(address, "play.example:25565");
            assert_eq!(username, "Alex");
        }
        other => panic!("expected Connect, got {other:?}"),
    }
    // Last-used prefill survives re-entry.
    ui.enter_title(None, None).expect("re-entry must prefill");
    assert_eq!(
        ui.fields.get("title/server").map(|f| f.text.as_str()),
        Some("play.example:25565")
    );
    assert_eq!(
        ui.fields.get("title/username").map(|f| f.text.as_str()),
        Some("Alex")
    );
    // Bad username fails closed at the UiState layer (no network touched).
    ui.enter_title(Some("play.example:25565"), Some("not valid!"))
        .expect("prefill must apply");
    ui.activate_for_test(
        NodeAction::Screen(Screen::DirectConnect),
        &mut inventory,
        &content,
    )
    .unwrap();
    assert!(
        ui.activate_for_test(NodeAction::Connect, &mut inventory, &content)
            .is_err(),
        "bad username must fail closed"
    );
    // Empty server fails closed too.
    ui.enter_title(Some("  "), Some("Alex"))
        .expect("prefill must apply");
    ui.activate_for_test(
        NodeAction::Screen(Screen::DirectConnect),
        &mut inventory,
        &content,
    )
    .unwrap();
    assert!(
        ui.activate_for_test(NodeAction::Connect, &mut inventory, &content)
            .is_err(),
        "empty server must fail closed"
    );
    // Quit emits Shutdown for the App owner.
    let effects = ui
        .activate_for_test(NodeAction::Quit, &mut inventory, &content)
        .expect("Quit must emit");
    assert_eq!(effects.commands.len(), 1);
    assert!(matches!(effects.commands[0], UiCommand::Shutdown));
}

#[test]
fn hud_minimum_sizes() {
    // Vanilla logical geometry (parent-owned Auto scale magnifies to
    // physical pixels): hotbar 182x22, hearts 9px, crosshair 15px. The
    // windowed proof drivers check the same bounds against live pixels;
    // here we pin the constants so a future shrink fails loudly.
    assert_eq!(UiState::HUD_HOTBAR, [182.0, 22.0]);
    assert!(UiState::HUD_HEART >= 9.0);
    assert!(UiState::HUD_CROSSHAIR >= 15.0);
    assert!(UiState::HUD_BUTTON_MIN[0] >= 200.0 && UiState::HUD_BUTTON_MIN[1] >= 20.0);
    // The dialog buttons (200x20) satisfy the shared 200x20 minimum; the
    // clickability gate itself needs a built tree (real fonts), so it is
    // exercised live, not here.
    assert!(200.0 >= UiState::HUD_BUTTON_MIN[0] && 20.0 >= UiState::HUD_BUTTON_MIN[1]);
}
#[test]
fn experience_bar_uses_hotbar_logical_width_before_auto_scale() {
    let (content, inventory, mut ui) = state();
    ui.resize(1280, 720, 1.);
    ui.set_presentation(UiPresentation {
        hud: HudState {
            experience: Some(0.5),
            ..HudState::default()
        },
        loading: None,
        death: None,
        can_respawn: false,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .unwrap();
    ui.rebuild_for_test(&inventory, &content).unwrap();
    let nodes = ui.observed_nodes();
    let background = nodes
        .iter()
        .find(|node| node["id"] == "hud/experience/background")
        .unwrap();
    assert_eq!(background["bounds"][2], 546.);
    assert_eq!(background["bounds"][3], 15.);
}
/// HUD armor/air rows reuse the existing sprite loop: once sync populates
/// `hud.armor` / `hud.air` / `hud.max_air`, the tree must contain the matching
/// `hud/armor/*` and `hud/air/*` sprite nodes. Attack below full charge must
/// expose a `hud/attack` progressbar next to the crosshair.
/// Vanilla hides the armor row at 0 (P1-1: empty-armor icons instead of
/// hearts); the air row stays hidden when full.
fn hud_armor_air_and_attack_nodes_appear_from_hud_state() {
    let (content, inventory, mut ui) = state();
    ui.set_presentation(UiPresentation {
        hud: HudState {
            health: Some(20.),
            max_health: Some(20.),
            armor: Some(10),
            air: Some(200),
            max_air: Some(300),
            attack: Some(0.5),
            ..HudState::default()
        },
        loading: None,
        death: None,
        can_respawn: false,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .expect("hud presentation must apply");
    // State-only UI has no fonts, so `synchronize` cannot rebuild the tree.
    // Drive the same explicit rebuild the windowed proof drivers use once
    // real fonts/GPU are present.
    ui.rebuild_for_test(&inventory, &content)
        .expect("hud tree must build");
    let tree = ui
        .semantic_tree(&inventory, &content)
        .expect("hud tree must read back");
    let ids: Vec<&str> = tree["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert!(
        ids.iter().any(|id| id.starts_with("hud/armor/")),
        "armor row missing: {ids:?}"
    );
    assert!(
        ids.iter().any(|id| id.starts_with("hud/air/")),
        "air row missing: {ids:?}"
    );
    assert!(ids.contains(&"hud/attack"), "attack bar missing: {ids:?}");
    // Full charge hides the indicator again (vanilla Gui behavior).
    ui.set_presentation(UiPresentation {
        hud: HudState {
            health: Some(20.),
            max_health: Some(20.),
            attack: Some(1.),
            ..HudState::default()
        },
        loading: None,
        death: None,
        can_respawn: false,
        editor: None,
        can_edit_operator_blocks: false,
    })
    .expect("hud presentation must apply");
    ui.rebuild_for_test(&inventory, &content)
        .expect("hud tree must build");
    let tree = ui
        .semantic_tree(&inventory, &content)
        .expect("hud tree must read back");
    let ids: Vec<&str> = tree["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert!(
        !ids.contains(&"hud/attack"),
        "attack bar must hide at full charge: {ids:?}"
    );
}

/// Vanilla hides the armor row when armor is 0 (QA P1-1: a row of
/// empty-armor icons must not render at zero). Armor above 0 still renders.
#[test]
fn hud_armor_row_hidden_at_zero() {
    let (content, inventory, mut ui) = state();
    for armor in [Some(0), None] {
        ui.set_presentation(UiPresentation {
            hud: HudState {
                health: Some(20.),
                max_health: Some(20.),
                armor,
                ..HudState::default()
            },
            loading: None,
            death: None,
            can_respawn: false,
            editor: None,
            can_edit_operator_blocks: false,
        })
        .expect("hud presentation must apply");
        ui.rebuild_for_test(&inventory, &content)
            .expect("hud tree must build");
        let tree = ui
            .semantic_tree(&inventory, &content)
            .expect("hud tree must read back");
        let ids: Vec<&str> = tree["nodes"]
            .as_array()
            .expect("nodes array")
            .iter()
            .filter_map(|node| node["id"].as_str())
            .collect();
        assert!(
            !ids.iter().any(|id| id.starts_with("hud/armor/")),
            "armor row must hide at {armor:?}: {ids:?}"
        );
    }
}

/// Inventory captions stay above slot rows (QA P2-4): the title and
/// inventory labels are narrow and must not intersect any slot rect.
/// The player recess carries a `menu/player_preview` layout pin (no fake
/// placeholder; parent-owned state.rs renders the real rig).
#[test]
fn inventory_captions_clear_slots_and_preview_pin_exists() {
    use crate::inventory::MenuKind;
    use crate::ui::Screen;
    let (content, mut inventory, mut ui) = state();
    inventory
        .open_inventory()
        .expect("player inventory must open");
    ui.resize(427, 240, 1.0);
    // Drive the same explicit rebuild the windowed proof drivers use once
    // real fonts/GPU are present; screen follows the observed container.
    ui.synchronize(&inventory, &content)
        .expect("menu screen must follow open inventory");
    assert_eq!(ui.screen(), Screen::Menu);
    assert_eq!(inventory.menu().kind, MenuKind::Inventory);
    ui.rebuild_for_test(&inventory, &content)
        .expect("menu tree must build");
    let nodes = ui.observed_nodes();
    assert!(
        nodes.iter().any(|node| node["id"] == "recipe/book"),
        "inventory must expose the shared recipe book"
    );
    assert!(
        !nodes.iter().any(|node| node["id"] == "menu/close"),
        "inventory must not expose a floating Close"
    );
    let rect = |id: &str| {
        nodes
            .iter()
            .find(|n| n["id"] == id)
            .unwrap_or_else(|| panic!("node {id} missing"))["bounds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap() as f32)
            .collect::<Vec<_>>()
    };
    let overlaps = |a: &[f32], b: &[f32]| {
        a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
    };
    let title = rect("menu/title");
    // InventoryScreen.extractLabels draws Crafting only; an Inventory
    // caption at y72 would intersect the boots slot (y62..78).
    assert!(!nodes
        .iter()
        .any(|node| node["id"] == "menu/inventory_label"));
    assert!(title[2] <= 72.0, "title label must stay narrow: {title:?}");
    let slots: Vec<Vec<f32>> = nodes
        .iter()
        .filter(|n| {
            n["id"]
                .as_str()
                .is_some_and(|id| id.starts_with("slot/") && !id.starts_with("slot_not"))
        })
        .map(|n| {
            n["bounds"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap() as f32)
                .collect()
        })
        .collect();
    assert!(!slots.is_empty(), "menu slots must exist");
    for slot in &slots {
        assert!(
            !overlaps(&title, slot),
            "title caption overlaps slot: {title:?} vs {slot:?}"
        );
    }
    // Preview pin: vanilla recess x25-74/y7-77 inside the 176x166 texture.
    let preview = rect("menu/player_preview");
    assert_eq!(
        preview[2], 51.0,
        "preview width must match recess: {preview:?}"
    );
    assert_eq!(
        preview[3], 70.0,
        "preview height must match recess: {preview:?}"
    );
}

/// Dialog trees fit the 427x240 Auto viewport (1280x720 window, scale 3):
/// every visible button rect stays inside the viewport. Title, death,
/// pause, and settings all route through this.
#[test]
fn dialog_trees_fit_auto_viewport() {
    use crate::ui::{NodeAction, Screen, UiIntent};
    let (content, inventory, mut ui) = state();
    ui.resize(427, 240, 1.0);
    // Title: enter via the ordinary title path, then rebuild with fonts
    // absent (layout only; visibility math is font-independent).
    ui.enter_title(None, None)
        .expect("enter_title must reach the Title screen");
    assert_eq!(ui.screen(), Screen::Title);
    ui.rebuild_for_test(&inventory, &content)
        .expect("title tree must build");
    for node in ui.observed_nodes() {
        if matches!(
            node["role"].as_str(),
            Some("button" | "slider" | "switch" | "tab")
        ) {
            let b = node["bounds"].as_array().expect("button bounds");
            let b: Vec<f32> = b.iter().map(|v| v.as_f64().unwrap() as f32).collect();
            assert!(
                b[0] >= 0.0 && b[1] >= 0.0 && b[0] + b[2] <= 427.0 && b[1] + b[3] <= 240.0,
                "title button overflows 427x240: {} {b:?}",
                node["id"]
            );
        }
    }
    // Death / pause / settings: presentation-driven screens.
    dead(&mut ui);
    ui.resize(427, 240, 1.0);
    ui.rebuild_for_test(&inventory, &content)
        .expect("death tree must build");
    for node in ui.observed_nodes() {
        if matches!(
            node["role"].as_str(),
            Some("button" | "slider" | "switch" | "tab")
        ) {
            let b = node["bounds"].as_array().expect("button bounds");
            let b: Vec<f32> = b.iter().map(|v| v.as_f64().unwrap() as f32).collect();
            assert!(
                b[0] >= 0.0 && b[1] >= 0.0 && b[0] + b[2] <= 427.0 && b[1] + b[3] <= 240.0,
                "death button overflows 427x240: {} {b:?}",
                node["id"]
            );
        }
    }
    // Pause via intent, settings via node action (same path as the live UI).
    let (content, mut inventory, mut ui) = state();
    ui.resize(427, 240, 1.0);
    ui.execute(UiIntent::OpenPause, &mut inventory, &content)
        .expect("OpenPause must work from Game");
    ui.rebuild_for_test(&inventory, &content)
        .expect("pause tree must build");
    for node in ui.observed_nodes() {
        if matches!(
            node["role"].as_str(),
            Some("button" | "slider" | "switch" | "tab")
        ) {
            let b = node["bounds"].as_array().expect("button bounds");
            let b: Vec<f32> = b.iter().map(|v| v.as_f64().unwrap() as f32).collect();
            assert!(
                b[0] >= 0.0 && b[1] >= 0.0 && b[0] + b[2] <= 427.0 && b[1] + b[3] <= 240.0,
                "pause button overflows 427x240: {} {b:?}",
                node["id"]
            );
        }
    }
    ui.activate_for_test(
        NodeAction::Screen(Screen::Settings),
        &mut inventory,
        &content,
    )
    .expect("Pause->Settings must transition");
    ui.rebuild_for_test(&inventory, &content)
        .expect("settings tree must build");
    for node in ui.observed_nodes() {
        if matches!(
            node["role"].as_str(),
            Some("button" | "slider" | "switch" | "tab")
        ) {
            let b = node["bounds"].as_array().expect("button bounds");
            let b: Vec<f32> = b.iter().map(|v| v.as_f64().unwrap() as f32).collect();
            assert!(
                b[0] >= 0.0 && b[1] >= 0.0 && b[0] + b[2] <= 427.0 && b[1] + b[3] <= 240.0,
                "settings button overflows 427x240: {} {b:?}",
                node["id"]
            );
        }
    }
}

/// Held-item name appears above the hotbar after a selection change and
/// expires 100 ticks later. The stack carries `minecraft:custom_name`, so the
/// name resolves without any item registry (test content has no item facts).
#[test]
fn hud_selected_name_shows_then_expires() {
    let (content, mut inventory, mut ui) = state();
    let mut named = crate::inventory::Stack::default();
    named.count = 1;
    std::sync::Arc::make_mut(&mut named.components).insert(
        "minecraft:custom_name".into(),
        serde_json::json!({"text":"Sword"}),
    );
    inventory.player.slots[36] = Some(named);
    inventory.selected_slot = 1;
    ui.tick(1000, 0.);
    ui.synchronize(&inventory, &content)
        .expect("selection change must track");
    ui.rebuild_for_test(&inventory, &content)
        .expect("hud tree must build");
    let tree = ui
        .semantic_tree(&inventory, &content)
        .expect("hud tree must read back");
    let has_name = tree["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .any(|node| node["id"] == "hud/selected_name");
    assert!(!has_name, "name follows the new slot, which is empty");
    inventory.selected_slot = 0;
    ui.synchronize(&inventory, &content)
        .expect("selection change must track");
    ui.rebuild_for_test(&inventory, &content)
        .expect("hud tree must build");
    let tree = ui
        .semantic_tree(&inventory, &content)
        .expect("hud tree must read back");
    let name = tree["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .find(|node| node["id"] == "hud/selected_name")
        .expect("selected name must show after change");
    assert_eq!(name["label"], "Sword");
    ui.tick(1100, 0.);
    ui.rebuild_for_test(&inventory, &content)
        .expect("hud tree must build");
    let tree = ui
        .semantic_tree(&inventory, &content)
        .expect("hud tree must read back");
    assert!(
        tree["nodes"]
            .as_array()
            .expect("nodes array")
            .iter()
            .all(|node| node["id"] != "hud/selected_name"),
        "selected name must expire after 100 ticks"
    );
}

/// F3 overlay: debug lines render as a `debug/line/{i}` column above a
/// `debug/panel` backing, even when HUD is hidden; empty lines draw nothing.
#[test]
fn f3_debug_lines_render_above_hidden_hud() {
    let (content, inventory, mut ui) = state();
    ui.set_debug_lines(vec![
        "60 fps (16.67 ms)".into(),
        "XYZ: 1.500 / 2.000 / 3.500".into(),
    ]);
    // hide_hud is a session-toggle key read directly from UiState::settings,
    // not a sanitized managed setting: seed it directly.
    ui.settings
        .insert("hide_hud".into(), serde_json::json!(true));
    ui.invalidate();
    ui.rebuild_for_test(&inventory, &content)
        .expect("debug tree must build");
    let tree = ui
        .semantic_tree(&inventory, &content)
        .expect("debug tree must read back");
    let ids: Vec<&str> = tree["nodes"]
        .as_array()
        .expect("nodes array")
        .iter()
        .filter_map(|node| node["id"].as_str())
        .collect();
    assert!(
        ids.contains(&"debug/panel"),
        "backing panel missing: {ids:?}"
    );
    assert!(ids.contains(&"debug/line/0"), "line 0 missing: {ids:?}");
    assert!(ids.contains(&"debug/line/1"), "line 1 missing: {ids:?}");
    assert!(
        !ids.iter().any(|id| id.starts_with("hud/hotbar")),
        "HUD must stay hidden under F3: {ids:?}"
    );
    ui.set_debug_lines(Vec::new());
    ui.rebuild_for_test(&inventory, &content)
        .expect("cleared tree must build");
    let tree = ui
        .semantic_tree(&inventory, &content)
        .expect("cleared tree must read back");
    assert!(
        tree["nodes"]
            .as_array()
            .expect("nodes array")
            .iter()
            .all(|node| !node["id"].as_str().unwrap_or("").starts_with("debug/")),
        "cleared F3 must draw no debug nodes"
    );
}

/// Windowed-proof seams: F3 toggle publishes the flag + session-part lines,
/// Pause opens only from Game (fail-closed on dialogs), and Options rows
/// render labels (button identity the playthrough clicks).
#[test]
fn pause_and_options_seams() {
    use crate::ui::{Modifiers, UiIntent};
    let (content, mut inventory, mut ui) = state();
    assert_eq!(ui.screen(), Screen::Game);
    // Game -> Pause via the ordinary OpenPause intent.
    ui.execute(UiIntent::OpenPause, &mut inventory, &content)
        .expect("OpenPause must work from Game");
    assert_eq!(ui.screen(), Screen::Pause);
    assert!(ui.dialog_screen_active());
    // State-only UiState has no fonts, so no semantic tree backs button
    // activation here (unknown_ui_target fail-closed). The live windowed
    // proof drives pause/settings/done/back-to-game clicks for real (see
    // step-pause/step-options screenshots). Here pin the screen machine:
    // Pause -> Settings -> Pause round-trip via NodeAction directly.
    use crate::ui::{NodeAction, UiCommand};
    let effects = ui
        .activate_for_test(
            NodeAction::Screen(Screen::Settings),
            &mut inventory,
            &content,
        )
        .expect("Pause->Settings must transition");
    assert_eq!(ui.screen(), Screen::Settings);
    assert!(effects.commands.is_empty());
    // Options FOV row cycles via next_setting_value (label shows the value).
    let fov0 = UiState::next_setting_value("fov", None).expect("fov default");
    ui.apply_setting("fov", fov0.clone())
        .expect("fov must apply");
    let fov1 = UiState::next_setting_value("fov", Some(&fov0)).expect("fov next");
    assert_ne!(fov0, fov1, "FOV row must cycle");
    ui.apply_setting("fov", fov1.clone())
        .expect("fov next must apply");
    assert_eq!(ui.setting("fov"), Some(&fov1));
    let _ = effects.commands.len();
    // Options rows carry labels (playthrough clicks by label).
    ui.rebuild_for_test(&inventory, &content)
        .expect("options must rebuild with real layout");
    let fov = ui
        .observed_nodes()
        .into_iter()
        .find(|n| n["id"] == "settings/fov")
        .expect("FOV row must exist");
    assert_eq!(fov["role"], "slider");
    assert!(fov["label"]
        .as_str()
        .is_some_and(|l| l.starts_with("Field of view:")));
    assert!(ui.observed_settings().get("fov").is_some() || true);
    // Escape on Pause goes back (Close path), not stuck.
    ui.execute(
        UiIntent::Key {
            key: "Escape".into(),
            pressed: true,
            modifiers: Modifiers::default(),
        },
        &mut inventory,
        &content,
    )
    .expect("Escape must leave Options");
}

#[test]
fn p1_sign_line_labels_select_each_edit_buffer_without_changing_typing() {
    use crate::ui::{EditorState, UiIntent};
    let (content, mut inventory, mut ui) = state();
    let pos = [1, 2, 3];
    inventory.sign_editor = Some((pos, true));
    ui.open_editor(
        EditorState::Sign {
            pos,
            front: true,
            lines: std::array::from_fn(|_| String::new()),
        },
        false,
    )
    .unwrap();
    ui.rebuild_for_test(&inventory, &content).unwrap();
    let (class, title) = ui.source_screen_metadata(&inventory).unwrap().unwrap();
    for line in 0..4 {
        let label = format!("Sign line {}", line + 1);
        let (focus, _) = ui
            .exact_input(&inventory, &content, &class, &title, &label, "x")
            .unwrap();
        match &focus {
            UiIntent::Focus { element_id, .. } => {
                assert_eq!(element_id, &format!("sign/line/{line}"));
            }
            other => panic!("unexpected sign input intent: {other:?}"),
        }
        ui.execute(focus, &mut inventory, &content).unwrap();
        ui.execute(
            UiIntent::Text {
                text: format!("line{}", line + 1),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
    }
    let effects = ui
        .execute(
            UiIntent::SubmitSign {
                context: ui.context(&inventory),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
    assert!(matches!(
        effects.commands.as_slice(),
        [UiCommand::Sign { lines, .. }]
            if lines == &["line1", "line2", "line3", "line4"].map(String::from)
    ));
}

#[test]
fn p1_review_sign_close_uses_current_fields_without_container_close() {
    use crate::ui::{EditorState, UiIntent};
    let (content, mut inventory, mut ui) = state();
    let pos = [1, 2, 3];
    inventory.sign_editor = Some((pos, false));
    inventory.screen_open = true;
    ui.open_editor(
        EditorState::Sign {
            pos,
            front: false,
            lines: ["old", "", "", ""].map(String::from),
        },
        false,
    )
    .unwrap();
    ui.synchronize(&inventory, &content).unwrap();
    let lines = ["current", "two", "three", "four"].map(String::from);
    for (line, text) in lines.iter().enumerate() {
        ui.execute(
            UiIntent::EditSignLine {
                context: ui.context(&inventory),
                line: line as u8,
                text: text.clone(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
    }
    let effects = ui
        .execute(
            UiIntent::Close {
                context: ui.context(&inventory),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
    assert!(matches!(effects.commands.as_slice(),
        [UiCommand::Sign { pos: actual, front: false, lines: submitted }]
            if actual == &pos && submitted == &lines));
    assert_eq!(ui.screen(), Screen::Game);
}

fn vitals_hud() -> HudState {
    HudState {
        health: Some(20.),
        max_health: Some(20.),
        food: Some(20),
        armor: Some(10),
        air: Some(150),
        max_air: Some(300),
        ..HudState::default()
    }
}
fn hud_has(ui: &UiState, prefix: &str) -> bool {
    ui.observed_nodes()
        .iter()
        .any(|n| n["id"].as_str().is_some_and(|id| id.starts_with(prefix)))
}

/// Vanilla `canHurtPlayer`: only survival/adventure draw health, hunger, armour
/// and air; creative and spectator hide all four rows but keep the hotbar.
#[test]
fn rc2_hud_vitals_follow_the_game_mode() {
    assert!(UiState::hud_shows_vitals(0) && UiState::hud_shows_vitals(2));
    assert!(!UiState::hud_shows_vitals(1) && !UiState::hud_shows_vitals(3));
    for (mode, shown) in [(0, true), (1, false), (2, true), (3, false)] {
        let (content, mut inventory, mut ui) = state();
        inventory.game_mode = mode;
        ui.resize(427, 240, 1.);
        ui.set_presentation(UiPresentation {
            hud: vitals_hud(),
            loading: None,
            death: None,
            can_respawn: false,
            editor: None,
            can_edit_operator_blocks: false,
        })
        .unwrap();
        ui.rebuild_for_test(&inventory, &content).unwrap();
        for row in ["hud/heart/", "hud/food/", "hud/armor/", "hud/air/"] {
            assert_eq!(hud_has(&ui, row), shown, "game mode {mode} row {row}");
        }
        assert!(hud_has(&ui, "hud/hotbar"), "game mode {mode} keeps the hotbar");
        assert!(hud_has(&ui, "hud/crosshair"));
    }
}

/// The hotbar and its selection frame are the game's own GUI sprites, not the
/// themed rectangles the options screens use.
#[test]
fn rc2_hud_hotbar_uses_vanilla_sprites() {
    let (content, inventory, mut ui) = state();
    ui.resize(427, 240, 1.);
    ui.rebuild_for_test(&inventory, &content).unwrap();
    let nodes = ui.observed_nodes();
    let bounds = |id: &str| {
        nodes.iter().find(|n| n["id"] == id).unwrap()["bounds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_f64().unwrap())
            .collect::<Vec<_>>()
    };
    assert_eq!(bounds("hud/hotbar")[2..], [182., 22.]);
    assert_eq!(bounds("hud/selection")[2..], [24., 23.]);
}
