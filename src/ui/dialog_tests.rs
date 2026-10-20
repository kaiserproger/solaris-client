use super::*;
use crate::{
    inventory::InventoryStore,
    protocol::{Writer, content::Content},
    ui::UiIntent,
};

fn state() -> (Content, InventoryStore, UiState) {
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs": []}"#.as_slice());
    let inventory = InventoryStore::default();
    let ui = UiState::new_state(&content).expect("state-only UI must construct");
    (content, inventory, ui)
}

fn s(value: &str) -> Nbt {
    Nbt::String(value.into())
}

fn compound(entries: Vec<(&str, Nbt)>) -> Nbt {
    Nbt::Compound(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), v))
            .collect(),
    )
}

fn list(items: Vec<Nbt>) -> Nbt {
    Nbt::List(if items.is_empty() { 0 } else { 10 }, items)
}

fn custom(id: &str, additions: Vec<(&str, Nbt)>) -> Nbt {
    let mut entries = vec![("type", s("minecraft:dynamic/custom")), ("id", s(id))];
    if !additions.is_empty() {
        entries.push(("additions", compound(additions)));
    }
    compound(entries)
}

fn button(label: &str, action: Option<Nbt>) -> Nbt {
    let mut entries = vec![("label", s(label))];
    entries.extend(action.map(|action| ("action", action)));
    compound(entries)
}

/// A `multi_action` dialog with one input of each kind, as the server API builds it.
fn sample(extra: Vec<(&str, Nbt)>) -> Nbt {
    let mut entries = vec![
        ("type", s("minecraft:multi_action")),
        ("title", s("Found settlement")),
        ("pause", Nbt::Byte(0)),
        (
            "body",
            list(vec![compound(vec![
                ("type", s("minecraft:plain_message")),
                ("contents", s("Pick a name and radius")),
            ])]),
        ),
        (
            "inputs",
            list(vec![
                compound(vec![
                    ("type", s("minecraft:text")),
                    ("key", s("name")),
                    ("label", s("Name")),
                    ("initial", s("Ёлки")),
                    ("max_length", Nbt::Int(8)),
                ]),
                compound(vec![
                    ("type", s("minecraft:number_range")),
                    ("key", s("radius")),
                    ("label", s("Radius")),
                    ("start", Nbt::Float(1.)),
                    ("end", Nbt::Float(32.)),
                    ("initial", Nbt::Float(8.)),
                    ("step", Nbt::Float(1.)),
                ]),
                compound(vec![
                    ("type", s("minecraft:boolean")),
                    ("key", s("walls")),
                    ("label", s("Walls")),
                ]),
                compound(vec![
                    ("type", s("minecraft:single_option")),
                    ("key", s("kind")),
                    ("label", s("Kind")),
                    (
                        "options",
                        list(vec![
                            compound(vec![("id", s("village")), ("display", s("Village"))]),
                            compound(vec![
                                ("id", s("camp")),
                                ("display", s("Camp")),
                                ("initial", Nbt::Byte(1)),
                            ]),
                        ]),
                    ),
                ]),
            ]),
        ),
        (
            "actions",
            list(vec![
                button(
                    "Found",
                    Some(custom("solaris:settlement/found", vec![("v", Nbt::Int(2))])),
                ),
                button("Plain", None),
            ]),
        ),
        (
            "exit_action",
            button("Close", Some(custom("solaris:settlement/cancel", vec![]))),
        ),
    ];
    for (key, value) in extra {
        match entries.iter_mut().find(|(name, _)| *name == key) {
            Some(entry) => entry.1 = value,
            None => entries.push((key, value)),
        }
    }
    compound(entries)
}

fn packet(dialog: &Nbt) -> Vec<u8> {
    let mut writer = Writer::default();
    writer.var(0);
    dialog.write_root(&mut writer).unwrap();
    writer.0
}

fn show(ui: &mut UiState, dialog: &Nbt) -> Result<()> {
    let bytes = packet(dialog);
    ui.apply_dialog_packet(140, &mut Reader::new(&bytes))
}

fn rebuild(ui: &mut UiState, content: &Content, inventory: &InventoryStore) {
    // Absorb the first-sync revision bump (hotbar slot) before contexts are taken.
    ui.synchronize(inventory, content).unwrap();
    ui.resize(1280, 720, 3.);
    ui.rebuild_for_test(inventory, content).unwrap();
    ui.synchronize(inventory, content).unwrap();
}

fn click(
    ui: &mut UiState,
    content: &Content,
    inventory: &mut InventoryStore,
    id: &str,
) -> UiEffects {
    let context = ui.context(inventory);
    let effects = ui
        .execute(
            UiIntent::Activate {
                context,
                element_id: id.into(),
            },
            inventory,
            content,
        )
        .unwrap();
    if ui.screen() == Screen::ServerDialog {
        rebuild(ui, content, inventory);
    }
    effects
}

fn sent(effects: &UiEffects) -> (&str, &Nbt) {
    match effects.commands.as_slice() {
        [
            UiCommand::DialogClick {
                id,
                payload: Some(payload),
            },
        ] => (id, payload),
        other => panic!("expected one dialog click, got {other:?}"),
    }
}

#[test]
fn show_dialog_opens_screen_with_title_body_inputs_and_buttons() {
    let (content, inventory, mut ui) = state();
    show(&mut ui, &sample(vec![])).unwrap();
    assert_eq!(ui.screen(), Screen::ServerDialog);
    assert!(ui.dialog_screen_active());
    rebuild(&mut ui, &content, &inventory);
    let ids: Vec<_> = ui.nodes.iter().map(|n| n.id.as_str()).collect();
    for expected in [
        "dialog/title",
        "dialog/body/0",
        "dialog/input/name",
        "dialog/input/radius",
        "dialog/input/walls",
        "dialog/input/kind",
        "dialog/action/0",
        "dialog/action/1",
        "dialog/action/2",
    ] {
        assert!(ids.contains(&expected), "{expected} missing from {ids:?}");
    }
    let node = |id: &str| ui.nodes.iter().find(|n| n.id == id).unwrap();
    assert_eq!(node("dialog/input/radius").label, "Radius: 8");
    assert_eq!(node("dialog/input/kind").label, "Kind: Camp");
    assert_eq!(node("dialog/action/0").label, "Found");
    assert!(
        ui.nodes
            .iter()
            .filter(|n| n.id.starts_with("dialog/"))
            .all(|n| n.visible)
    );
    // The exit button sits in the footer, below the body controls.
    assert!(node("dialog/action/2").bounds[1] > node("dialog/action/0").bounds[1]);
}

#[test]
fn button_click_sends_typed_inputs_merged_over_additions_and_closes() {
    let (content, mut inventory, mut ui) = state();
    show(&mut ui, &sample(vec![])).unwrap();
    rebuild(&mut ui, &content, &inventory);
    // Edit the text field.
    click(&mut ui, &content, &mut inventory, "dialog/input/name");
    ui.execute(
        UiIntent::Text { text: "!".into() },
        &mut inventory,
        &content,
    )
    .unwrap();
    rebuild(&mut ui, &content, &inventory);
    // Flip the checkbox and cycle the option (camp -> village).
    click(&mut ui, &content, &mut inventory, "dialog/input/walls");
    click(&mut ui, &content, &mut inventory, "dialog/input/kind");
    // Keyboard on the slider: focus it, then ArrowRight (+1% of the track = 8.31, snapped to 8).
    let context = ui.context(&inventory);
    ui.execute(
        UiIntent::Focus {
            context,
            element_id: "dialog/input/radius".into(),
        },
        &mut inventory,
        &content,
    )
    .unwrap();
    for _ in 0..1 {
        ui.execute(
            UiIntent::Key {
                key: "ArrowRight".into(),
                pressed: true,
                modifiers: Default::default(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
    }
    rebuild(&mut ui, &content, &inventory);
    let effects = click(&mut ui, &content, &mut inventory, "dialog/action/0");
    let (id, payload) = sent(&effects);
    assert_eq!(id, "solaris:settlement/found");
    assert_eq!(
        payload.get("v"),
        Some(&Nbt::Int(2)),
        "static additions survive"
    );
    assert_eq!(payload.get("name"), Some(&s("Ёлки!")));
    assert_eq!(payload.get("walls"), Some(&Nbt::Byte(1)));
    assert_eq!(payload.get("kind"), Some(&s("village")));
    assert_eq!(payload.get("radius"), Some(&Nbt::Float(8.)));
    assert_eq!(
        ui.screen(),
        Screen::Game,
        "after_action close returns to the previous screen"
    );
    assert!(ui.fields.keys().all(|id| !id.starts_with("dialog/")));
}

#[test]
fn text_limit_counts_utf16_units_and_plain_buttons_send_nothing() {
    let (content, mut inventory, mut ui) = state();
    show(&mut ui, &sample(vec![])).unwrap();
    rebuild(&mut ui, &content, &inventory);
    click(&mut ui, &content, &mut inventory, "dialog/input/name");
    // "Ёлки" is 4 units; max_length 8 leaves room for exactly four more.
    ui.execute(
        UiIntent::Text {
            text: "abcd".into(),
        },
        &mut inventory,
        &content,
    )
    .unwrap();
    assert!(
        ui.execute(
            UiIntent::Text { text: "e".into() },
            &mut inventory,
            &content
        )
        .is_err()
    );
    rebuild(&mut ui, &content, &inventory);
    let effects = click(&mut ui, &content, &mut inventory, "dialog/action/1");
    assert!(effects.commands.is_empty());
    assert_eq!(ui.screen(), Screen::Game);
}

#[test]
fn slider_drag_snaps_to_step_and_label_follows() {
    let (content, mut inventory, mut ui) = state();
    show(&mut ui, &sample(vec![])).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let bounds = ui
        .nodes
        .iter()
        .find(|n| n.id == "dialog/input/radius")
        .unwrap()
        .bounds;
    let (track_x, track_w) = super::super::options::track_span(bounds);
    // Pointer at ~75% of the track: 1 + 0.75 * 31 = 24.25 -> step 1 around initial 8 -> 24.
    ui.pointer = [track_x + 3. + (track_w - 6.) * 0.75, bounds[1] + 5.];
    assert!(ui.dialog_slider_pointer("dialog/input/radius"));
    rebuild(&mut ui, &content, &inventory);
    assert_eq!(
        ui.nodes
            .iter()
            .find(|n| n.id == "dialog/input/radius")
            .unwrap()
            .label,
        "Radius: 24"
    );
    let effects = click(&mut ui, &content, &mut inventory, "dialog/action/0");
    assert_eq!(sent(&effects).1.get("radius"), Some(&Nbt::Float(24.)));
}

#[test]
fn escape_runs_exit_action_unless_the_dialog_cannot_be_closed() {
    let (content, mut inventory, mut ui) = state();
    show(&mut ui, &sample(vec![])).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let effects = ui
        .execute(
            UiIntent::Key {
                key: "Escape".into(),
                pressed: true,
                modifiers: Default::default(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
    assert_eq!(sent(&effects).0, "solaris:settlement/cancel");
    assert_eq!(ui.screen(), Screen::Game);

    let (content, mut inventory, mut ui) = state();
    show(
        &mut ui,
        &sample(vec![("can_close_with_escape", Nbt::Byte(0))]),
    )
    .unwrap();
    rebuild(&mut ui, &content, &inventory);
    let effects = ui
        .execute(
            UiIntent::Key {
                key: "Escape".into(),
                pressed: true,
                modifiers: Default::default(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
    assert!(effects.commands.is_empty());
    assert_eq!(ui.screen(), Screen::ServerDialog);
}

#[test]
fn notice_and_confirmation_use_footer_buttons_and_escape_picks_the_cancel_side() {
    let (content, mut inventory, mut ui) = state();
    let confirm = compound(vec![
        ("type", s("minecraft:confirmation")),
        ("title", s("Sure?")),
        ("pause", Nbt::Byte(0)),
        ("yes", button("Yes", Some(custom("solaris:x/yes", vec![])))),
        ("no", button("No", Some(custom("solaris:x/no", vec![])))),
    ]);
    show(&mut ui, &confirm).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let yes = ui
        .nodes
        .iter()
        .find(|n| n.id == "dialog/action/0")
        .unwrap()
        .bounds;
    let no = ui
        .nodes
        .iter()
        .find(|n| n.id == "dialog/action/1")
        .unwrap()
        .bounds;
    assert_eq!(yes[1], no[1], "same footer row");
    assert!(yes[0] < no[0]);
    let effects = ui
        .execute(
            UiIntent::Key {
                key: "Escape".into(),
                pressed: true,
                modifiers: Default::default(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
    assert_eq!(sent(&effects).0, "solaris:x/no");

    // Notice without an action shows a default OK that just closes.
    let notice = compound(vec![
        ("type", s("minecraft:notice")),
        ("title", s("Hi")),
        ("pause", Nbt::Byte(0)),
    ]);
    show(&mut ui, &notice).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let effects = click(&mut ui, &content, &mut inventory, "dialog/action/0");
    assert!(effects.commands.is_empty());
    assert_eq!(ui.screen(), Screen::Game);
}

#[test]
fn after_action_none_keeps_the_dialog_and_wait_for_response_blocks_until_cleared() {
    let (content, mut inventory, mut ui) = state();
    show(&mut ui, &sample(vec![("after_action", s("none"))])).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let effects = click(&mut ui, &content, &mut inventory, "dialog/action/0");
    assert_eq!(sent(&effects).0, "solaris:settlement/found");
    assert_eq!(ui.screen(), Screen::ServerDialog);

    let (content, mut inventory, mut ui) = state();
    show(
        &mut ui,
        &sample(vec![("after_action", s("wait_for_response"))]),
    )
    .unwrap();
    rebuild(&mut ui, &content, &inventory);
    click(&mut ui, &content, &mut inventory, "dialog/action/0");
    rebuild(&mut ui, &content, &inventory);
    assert_eq!(ui.screen(), Screen::ServerDialog);
    assert!(ui.dialogs.shown.as_ref().unwrap().waiting_since.is_some());
    assert!(
        ui.nodes.iter().all(|n| n.id != "dialog/action/0"),
        "controls are gone while waiting"
    );
    // clear_dialog answers the wait.
    ui.apply_dialog_packet(139, &mut Reader::new(&[])).unwrap();
    assert_eq!(ui.screen(), Screen::Game);
}

#[test]
fn a_new_dialog_replaces_the_shown_one_and_keeps_the_original_return_screen() {
    let (content, inventory, mut ui) = state();
    ui.transition(Screen::Pause);
    show(&mut ui, &sample(vec![])).unwrap();
    let second = compound(vec![
        ("type", s("minecraft:notice")),
        ("title", s("Second")),
        ("pause", Nbt::Byte(0)),
    ]);
    show(&mut ui, &second).unwrap();
    rebuild(&mut ui, &content, &inventory);
    assert!(
        ui.nodes
            .iter()
            .any(|n| n.id == "dialog/title" && n.label == "Second")
    );
    assert!(ui.nodes.iter().all(|n| n.id != "dialog/input/name"));
    ui.apply_dialog_packet(139, &mut Reader::new(&[])).unwrap();
    assert_eq!(ui.screen(), Screen::Pause);
}

#[test]
fn dialog_received_while_loading_opens_when_loading_ends() {
    use crate::ui::{HudState, UiPresentation};
    let (_content, _inventory, mut ui) = state();
    let presentation = |loading: Option<&str>| UiPresentation {
        hud: HudState::default(),
        loading: loading.map(str::to_owned),
        death: None,
        can_respawn: false,
        editor: None,
        can_edit_operator_blocks: false,
    };
    ui.set_presentation(presentation(Some("Loading terrain")))
        .unwrap();
    show(&mut ui, &sample(vec![])).unwrap();
    assert_eq!(ui.screen(), Screen::Loading);
    ui.set_presentation(presentation(None)).unwrap();
    assert_eq!(ui.screen(), Screen::ServerDialog);
    ui.apply_dialog_packet(139, &mut Reader::new(&[])).unwrap();
    assert_eq!(ui.screen(), Screen::Game);
}

#[test]
fn run_command_actions_substitute_inputs_like_vanilla_templates() {
    let (content, mut inventory, mut ui) = state();
    let dialog = compound(vec![
        ("type", s("minecraft:notice")),
        ("title", s("Say")),
        ("pause", Nbt::Byte(0)),
        (
            "inputs",
            list(vec![compound(vec![
                ("type", s("minecraft:text")),
                ("key", s("msg")),
                ("label", s("Message")),
                ("initial", s("it's \"x\"")),
                ("max_length", Nbt::Int(32)),
            ])]),
        ),
        (
            "action",
            button(
                "Run",
                Some(compound(vec![
                    ("type", s("minecraft:dynamic/run_command")),
                    ("template", s("/say hi $(msg) $(missing)!")),
                ])),
            ),
        ),
    ]);
    show(&mut ui, &dialog).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let effects = click(&mut ui, &content, &mut inventory, "dialog/action/0");
    match effects.commands.as_slice() {
        [
            UiCommand::Chat {
                message,
                command: true,
            },
        ] => {
            assert_eq!(message, "say hi it\\'s \\\"x\\\" !");
        }
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn malformed_dialogs_fail_the_packet_and_unsupported_types_are_reported_not_fatal() {
    let (_content, _inventory, mut ui) = state();
    // pause=true (the default) with after_action none is invalid in vanilla.
    assert!(
        show(
            &mut ui,
            &sample(vec![("pause", Nbt::Byte(1)), ("after_action", s("none"))])
        )
        .is_err()
    );
    assert!(show(&mut ui, &compound(vec![("type", s("minecraft:notice"))])).is_err());
    let bad_key = sample(vec![]);
    let Nbt::Compound(mut entries) = bad_key else {
        unreachable!()
    };
    entries.retain(|(k, _)| k != "inputs");
    entries.push((
        "inputs".into(),
        list(vec![compound(vec![
            ("type", s("minecraft:boolean")),
            ("key", s("bad key")),
            ("label", s("x")),
        ])]),
    ));
    assert!(show(&mut ui, &Nbt::Compound(entries)).is_err());
    assert_eq!(ui.screen(), Screen::Game);
    // A syntactically fine but unsupported dialog type leaves the UI alone.
    show(
        &mut ui,
        &compound(vec![
            ("type", s("minecraft:server_links")),
            ("title", s("Links")),
        ]),
    )
    .unwrap();
    assert_eq!(ui.screen(), Screen::Game);
    assert_eq!(ui.dialogs.rejected.len(), 1);
    // Registry references need the dialog registry; they are ignored, not fatal.
    ui.apply_dialog_packet(140, &mut Reader::new(&[2])).unwrap();
    assert_eq!(ui.screen(), Screen::Game);
}

#[test]
fn scaled_value_matches_vanilla_compute_scaled_value() {
    // start 1, end 32, initial 8, step 1 (the sample slider).
    let f = |slider| compute_scaled_value(slider, 1., 32., Some(8.), Some(1.));
    assert_eq!(f(initial_slider(1., 32., Some(8.))), 8.);
    assert_eq!(f(0.), 1.);
    assert_eq!(f(1.), 32.);
    // No step: plain lerp. No initial: midpoint anchors the step grid.
    assert_eq!(compute_scaled_value(0.5, 0., 10., None, None), 5.);
    assert_eq!(compute_scaled_value(1., 0., 10., None, Some(3.)), 8.);
    // Degenerate range pins the slider to the middle.
    assert_eq!(initial_slider(5., 5., None), 0.5);
    assert_eq!(number_text(12.), "12");
    assert_eq!(number_text(12.5), "12.5");
}

#[test]
fn slider_activation_steps_by_the_declared_step_and_wraps_at_the_end() {
    let (content, mut inventory, mut ui) = state();
    show(&mut ui, &sample(vec![])).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let label = |ui: &UiState| {
        ui.nodes
            .iter()
            .find(|n| n.id == "dialog/input/radius")
            .unwrap()
            .label
            .clone()
    };
    click(&mut ui, &content, &mut inventory, "dialog/input/radius");
    assert_eq!(label(&ui), "Radius: 9");
    for _ in 0..23 {
        click(&mut ui, &content, &mut inventory, "dialog/input/radius");
    }
    assert_eq!(label(&ui), "Radius: 32");
    click(&mut ui, &content, &mut inventory, "dialog/input/radius");
    assert_eq!(label(&ui), "Radius: 1");
}

fn long_notice(paragraphs: usize) -> Nbt {
    compound(vec![
        ("type", s("minecraft:notice")),
        ("title", s("Long")),
        ("pause", Nbt::Byte(0)),
        (
            "body",
            list(
                (0..paragraphs)
                    .map(|i| {
                        compound(vec![
                            ("type", s("minecraft:plain_message")),
                            ("contents", s(&format!("Paragraph {i}"))),
                        ])
                    })
                    .collect(),
            ),
        ),
    ])
}

#[test]
fn body_text_crossing_the_viewport_edge_is_clipped_not_hidden() {
    let (content, inventory, mut ui) = state();
    show(&mut ui, &long_notice(30)).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let (top, bottom) = (HEADER, ui.height - FOOTER);
    let max = ui.dialogs.max_scroll;
    assert!(max > 3, "the dialog must overflow its viewport");
    let mut partial = 0;
    for scroll in 0..=max {
        ui.scroll = scroll;
        ui.invalidate();
        rebuild(&mut ui, &content, &inventory);
        for node in ui.nodes.iter().filter(|n| n.id.starts_with("dialog/body/")) {
            let [_, y, _, h] = node.bounds;
            let overlaps = y < bottom && y + h > top;
            assert_eq!(node.visible, overlaps, "{} at scroll {scroll}", node.id);
            if overlaps && (y < top || y + h > bottom) {
                partial += 1;
                let clip = node
                    .clip
                    .expect("a straddling text node carries a clip rect");
                assert!(clip[1] >= top && clip[1] + clip[3] <= bottom, "{clip:?}");
            }
        }
    }
    assert!(
        partial > 0,
        "no scroll offset put text across a viewport edge"
    );
}

#[test]
fn clip_rect_of_a_paragraph_taller_than_the_viewport_is_the_viewport() {
    // Taller than the viewport on both sides: only the viewport slice may paint.
    assert_eq!(
        dialog_clip([10., -500., 100., 2000.], 33., 207.),
        Some([10., 33., 100., 174.])
    );
    assert_eq!(
        dialog_clip([10., 40., 100., 9.], 33., 207.),
        Some([10., 40., 100., 9.])
    );
    assert_eq!(dialog_clip([10., 300., 100., 9.], 33., 207.), None);
    assert_eq!(dialog_clip([10., 0., 100., 33.], 33., 207.), None);
}

#[test]
fn mcp_button_click_scrolls_an_off_screen_dialog_button_into_view() {
    let (content, mut inventory, mut ui) = state();
    let actions = (0..20)
        .map(|i| {
            button(
                &format!("Go {i}"),
                Some(custom(&format!("solaris:t/{i}"), vec![])),
            )
        })
        .collect();
    let dialog = sample(vec![
        ("body", list(vec![])),
        ("inputs", list(vec![])),
        ("actions", list(actions)),
        ("columns", Nbt::Int(1)),
    ]);
    show(&mut ui, &dialog).unwrap();
    rebuild(&mut ui, &content, &inventory);
    let node = |ui: &UiState, id: &str| ui.nodes.iter().find(|n| n.id == id).unwrap().clone();
    assert!(
        !node(&ui, "dialog/action/15").visible,
        "starts below the viewport"
    );
    let (class, title) = ui.dialog_screen_metadata().unwrap();
    let intent = ui
        .exact_button(&inventory, &content, Some(class), &title, "Go 15", false)
        .unwrap();
    assert!(
        matches!(&intent, UiIntent::Activate { element_id, .. } if element_id == "dialog/action/15")
    );
    assert!(ui.scroll > 0);
    assert!(node(&ui, "dialog/action/15").visible);
    let effects = ui.execute(intent, &mut inventory, &content).unwrap();
    assert_eq!(sent(&effects).0, "solaris:t/15");

    // And back up again for a button scrolled above the viewport.
    show(&mut ui, &dialog).unwrap();
    rebuild(&mut ui, &content, &inventory);
    ui.scroll = ui.dialogs.max_scroll;
    ui.invalidate();
    rebuild(&mut ui, &content, &inventory);
    assert!(
        !node(&ui, "dialog/action/0").visible,
        "starts above the viewport"
    );
    let intent = ui
        .exact_button(&inventory, &content, Some(class), &title, "Go 0", false)
        .unwrap();
    assert!(
        matches!(&intent, UiIntent::Activate { element_id, .. } if element_id == "dialog/action/0")
    );
    assert!(node(&ui, "dialog/action/0").visible);
}
