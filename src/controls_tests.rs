use crate::controls::{Binding, Bindings};
use winit::{event::MouseButton, keyboard::KeyCode};

#[test]
fn controls_rebind_conflict_and_resets() {
    let mut bindings = Bindings::default();
    bindings.set("forward", Binding::Key(KeyCode::KeyA));
    assert!(bindings.conflict("forward"));
    assert!(bindings.conflict("left"));
    bindings.set("forward", Binding::Mouse(MouseButton::Middle));
    assert!(!bindings.conflict("forward"));
    assert_eq!(bindings.action(Binding::Mouse(MouseButton::Middle)), Some("forward"));
    bindings.reset("forward");
    assert_eq!(bindings.action(Binding::Key(KeyCode::KeyW)), Some("forward"));
}

#[test]
fn controls_settings_round_trip() {
    let mut bindings = Bindings::default();
    bindings.set("jump", Binding::Mouse(MouseButton::Middle));
    let path = std::env::temp_dir().join(format!("solaris-controls-{}.json", std::process::id()));
    let mut settings = std::collections::BTreeMap::new();
    settings.insert("key_bindings".into(), bindings.value());
    crate::local_config::save_client_settings_to(&settings, &path);
    let saved = crate::local_config::load_client_settings_from(&path);
    std::fs::remove_file(path).unwrap();
    let restored = Bindings::from_value(&saved["key_bindings"]).unwrap();
    assert_eq!(restored.action(Binding::Mouse(MouseButton::Middle)), Some("jump"));
}

#[test]
fn controls_item_mapping_uses_rebound_key() {
    let mut bindings = Bindings::default();
    bindings.set("hotbar_2", Binding::Mouse(MouseButton::Middle));
    assert!(matches!(crate::native_item_key(&bindings, Binding::Mouse(MouseButton::Middle), true, crate::ui::Modifiers::default(), false), Some(crate::api::InputIntent::Hotbar { slot: 1 })));
    assert!(crate::native_item_key(&bindings, Binding::Key(KeyCode::Digit2), true, crate::ui::Modifiers::default(), false).is_none());
}

#[test]
fn controls_native_dispatch_uses_rebound_mouse_button() {
    let options = crate::Options {
        automation: false, bind: "127.0.0.1:0".parse().unwrap(),
        artifacts: std::env::temp_dir(), headless: Some(true),
        runtime_dir: std::env::temp_dir(), profile: "fixture".into(),
        server: None, direct_join: false, username: None,
        width: 1280, height: 720, fullscreen: false,
    };
    let mut app = crate::App::new(options, std::sync::Arc::new(|| {})).unwrap();
    let mut content = crate::protocol::content::Content::test_cube();
    content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
    let mut ui = crate::ui::UiState::new_state(&content).unwrap();
    ui.rebind = Some("hotbar_2");
    app.session.ui = Some(ui);
    assert!(app.capture_binding(Binding::Mouse(MouseButton::Middle)));
    assert_eq!(app.session.bindings.action(Binding::Mouse(MouseButton::Middle)), Some("hotbar_2"));
    app.session.ui = None;
    app.dispatch_binding(Binding::Key(KeyCode::Digit2), true);
    assert_eq!(app.session.inventory.selected_slot, 0);
    app.dispatch_binding(Binding::Mouse(MouseButton::Middle), true);
    assert_eq!(app.session.inventory.selected_slot, 1);
}
