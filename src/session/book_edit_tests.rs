//! Authored fixtures follow local 26.3 BookEditScreen.saveChanges and edit-book codec facts.
use super::*;
use crate::protocol::{Reader, packet_ids::play::{clientbound as cb, serverbound as sb}};

fn dispatch(session: &mut Session, intent: UiIntent) {
    let effects = session.ui.as_mut().unwrap().execute(
        intent, &mut session.inventory, session.content.as_ref().unwrap(),
    ).unwrap();
    for command in effects.commands {
        session.execute_ui_command(command).unwrap();
    }
}

fn assert_book_packet(hand: u8, selected: u8, title: Option<&str>) {
    let mut session = Session::new_mode("rc1-book-edit".into(), false);
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    content.item_names.insert(1, "minecraft:writable_book".into());
    content.insert_test_item_components(1, BTreeMap::from([
        ("minecraft:writable_book_content".into(), json!({"pages":[]})),
    ]));
    session.ui = Some(UiState::new_state(&content).unwrap());
    session.content = Some(content);
    session.phase = "play".into();
    session.inventory.selected_slot = selected;
    let expected_slot = if hand == 1 { 40 } else { i32::from(selected) };
    let mut received = Writer::default();
    received.var(expected_slot);
    received.0.extend([1, 1, 0, 0]); // one default writable book, no component patch
    session.apply(Phase::Play, cb::SET_PLAYER_INVENTORY, &received.0).unwrap();
    session.open_trigger_local(UiTrigger::Book { hand }).unwrap();
    session.ui.as_mut().unwrap().rebuild_for_test(
        &session.inventory, session.content.as_ref().unwrap(),
    ).unwrap();
    session.commands.clear();
    dispatch(&mut session, UiIntent::Text { text: "line one\nliteral @a 🦀".into() });
    let context = session.ui.as_ref().unwrap().context(&session.inventory);
    dispatch(&mut session, UiIntent::EditBookPage {
        context, page: 1, text: "second page".into(),
    });
    let context = session.ui.as_ref().unwrap().context(&session.inventory);
    let submit = match title {
        Some(title) => UiIntent::SubmitBook { context, title: Some(title.into()) },
        None => UiIntent::Activate { context, element_id: "editor/submit".into() },
    };
    dispatch(&mut session, submit);
    assert_eq!(session.ui.as_ref().unwrap().screen(), crate::ui::Screen::Game);
    assert_eq!(session.commands.len(), 1);
    let packet = &session.commands[0];
    assert_eq!(packet.id, sb::EDIT_BOOK);
    let mut reader = Reader::new(&packet.payload.0);
    assert_eq!(reader.var().unwrap(), expected_slot, "edit-book uses inventory, not menu slot");
    assert_eq!(reader.var().unwrap(), 2);
    assert_eq!(reader.string(1024).unwrap(), "line one\nliteral @a 🦀");
    assert_eq!(reader.string(1024).unwrap(), "second page");
    assert_eq!(reader.bool().unwrap(), title.is_some());
    if let Some(title) = title {
        assert_eq!(reader.string(32).unwrap(), title);
    }
    reader.done().unwrap();
}

#[test]
fn rc1_book_done_encodes_selected_hotbar_inventory_slot() {
    for selected in [0, 4, 8] {
        assert_book_packet(0, selected, None);
    }
}

#[test]
fn rc1_book_done_encodes_offhand_inventory_slot() {
    for selected in [0, 8] {
        assert_book_packet(1, selected, None);
    }
}

#[test]
fn rc1_book_sign_encodes_inventory_slot_pages_and_title() {
    for hand in [0, 1] {
        assert_book_packet(hand, 6, Some("Field notes"));
    }
}

#[test]
#[ignore = "requires a controlled loopback server with one writable book in BookFix's first hotbar slot"]
fn rc1_book_live_write_reopen_sign() {
    fn wait(session: &mut Session, predicate: impl Fn(&Session) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            session.process_events();
            assert!(session.error.is_none(), "{:?}", session.error);
            if predicate(session) {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "book live state deadline: {}", session.phase);
            // The network/bootstrap wake retains an unpark token; never sleep or poll.
            thread::park_timeout(remaining);
        }
    }
    fn current_context(session: &mut Session) -> crate::inventory::UiContext {
        let ui = session.ui.as_mut().unwrap();
        ui.synchronize(&session.inventory, session.content.as_ref().unwrap()).unwrap();
        ui.context(&session.inventory)
    }
    let address = std::env::var("SOLARIS_BOOK_SMOKE_SERVER").expect("controlled book server");
    assert!(address.parse::<std::net::SocketAddr>().unwrap().ip().is_loopback());
    let mut session = Session::new_mode("rc1-book-live".into(), false);
    let owner = thread::current();
    session.set_wake(Arc::new(move || owner.unpark()));
    session.connect(&address, "BookFix").unwrap();
    wait(&mut session, |s| s.phase == "play" && s.play_world_presentation_ready()
        && s.inventory.received_player_slot(36).unwrap().is_some_and(|stack|
            stack.components.contains_key("minecraft:writable_book_content")));
    session.input(InputIntent::Hotbar { slot: 0 }).unwrap();
    session.interact(InteractionIntent::Use { hand: 0 }).unwrap();
    assert!(matches!(session.ui.as_ref().unwrap().editor,
        Some(EditorState::Book { writable: true, .. })));
    let text = "rc1 book persists\nliteral @a 🦀";
    let context = current_context(&mut session);
    session.execute_ui(UiIntent::EditBookPage { context, page: 0, text: text.into() }).unwrap();
    let context = current_context(&mut session);
    session.execute_ui(UiIntent::SubmitBook { context, title: None }).unwrap();
    wait(&mut session, |s| s.inventory.received_player_slot(36).unwrap()
        .is_some_and(|stack| stack.components["minecraft:writable_book_content"]["pages"][0]["raw"]
            .as_str() == Some(text)));
    let received = session.inventory.received_player_slot(36).unwrap().unwrap();
    eprintln!("book_live_unsigned_received={}", received.components["minecraft:writable_book_content"]);
    session.interact(InteractionIntent::Use { hand: 0 }).unwrap();
    assert!(matches!(&session.ui.as_ref().unwrap().editor,
        Some(EditorState::Book { pages, writable: true, .. })
            if pages.len() == 1 && pages[0] == text));
    // Sign the reopened server-observed pages without rewriting them.
    let context = current_context(&mut session);
    session.execute_ui(UiIntent::SubmitBook { context, title: Some("Field notes".into()) }).unwrap();
    wait(&mut session, |s| s.inventory.received_player_slot(36).unwrap()
        .is_some_and(|stack| stack.components.contains_key("minecraft:written_book_content")));
    let received = session.inventory.received_player_slot(36).unwrap().unwrap();
    assert_eq!(session.content.as_ref().unwrap().item_names[&received.id], "minecraft:written_book");
    assert!(!received.components.contains_key("minecraft:writable_book_content"));
    eprintln!("book_live_signed_received={}", received.components["minecraft:written_book_content"]);
    session.interact(InteractionIntent::Use { hand: 0 }).unwrap();
    wait(&mut session, |s| matches!(&s.ui.as_ref().unwrap().editor,
        Some(EditorState::Book { writable: false, .. })));
    assert!(matches!(&session.ui.as_ref().unwrap().editor,
        Some(EditorState::Book { pages, title, author, writable: false, .. })
            if pages.len() == 1 && pages[0] == text
                && title.as_deref() == Some("Field notes") && author.as_deref() == Some("BookFix")));
    eprintln!("book_live_signed_reopened_read_only=true");
    session.command(Command::Disconnect).unwrap();
}
