//! Behavioral shell tests. Native-window screenshots are a separate acceptance gate.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

fn test_app() -> App {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let scratch = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let root = scratch.join(format!(
        "solaris-redesign-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    App::new(Layout::new(root))
}
fn paint(
    app: &mut App,
    ctx: &egui::Context,
    size: Vec2,
    events: Vec<egui::Event>,
) -> egui::FullOutput {
    typography(ctx);
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        },
        |ctx| app.draw(ctx),
    )
}
fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Pos2)> {
    fn visit(shape: &egui::Shape, out: &mut Vec<(String, egui::Pos2)>) {
        match shape {
            egui::Shape::Text(t) => {
                out.push((t.galley.job.text.clone(), t.pos + t.galley.size() * 0.5))
            }
            egui::Shape::Vec(shapes) => {
                for s in shapes {
                    visit(s, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for s in shapes {
        visit(&s.shape, &mut out);
    }
    out
}
fn click(app: &mut App, ctx: &egui::Context, label: &str) {
    let size = Vec2::new(820.0, 530.0);
    let _ = paint(app, ctx, size, vec![]);
    let output = paint(app, ctx, size, vec![]);
    // A search field can contain the exact theme name too. Target the last
    // matching painted label (the gallery control), not the search editor.
    let pos = texts(&output.shapes)
        .into_iter()
        .rfind(|(text, _)| text == label)
        .unwrap_or_else(|| {
            panic!(
                "Missing visible label {label:?}; visible: {:?}",
                texts(&output.shapes)
            )
        })
        .1;
    let _ = paint(
        app,
        ctx,
        size,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    let _ = paint(
        app,
        ctx,
        size,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    );
}

#[test]
fn main_screen_is_not_an_inline_administration_form() {
    let ctx = egui::Context::default();
    let mut app = test_app();
    let _ = paint(&mut app, &ctx, Vec2::new(820.0, 530.0), vec![]);
    let output = paint(&mut app, &ctx, Vec2::new(820.0, 530.0), vec![]);
    let labels = texts(&output.shapes);
    assert!(
        !labels.iter().any(|(t, _)| t == "Username"),
        "Username editing belongs in a dialog"
    );
    assert!(
        !labels.iter().any(|(t, _)| t == "Address (host[:port])"),
        "Address editing belongs in a dialog"
    );
    for expected in [
        "Solaris test",
        "Profiles",
        "Servers",
        "Install & Play",
        "Offline profile",
    ] {
        assert!(
            labels.iter().any(|(t, _)| t == expected),
            "Missing home control {expected}"
        );
    }
}

#[test]
fn nested_navigation_starts_reveal_before_painting_new_content() {
    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        time: f64,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(820.0, 530.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            },
            |ctx| app.draw(ctx),
        )
    }
    fn opacity(output: &egui::FullOutput, label: &str) -> Option<f32> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.job.text == label => {
                // Ui opacity tints paint colors; TextShape's separate factor
                // remains 1.0. Inspect the color actually sent for rendering.
                Some(f32::from(text.fallback_color.a()) / 255.0 * text.opacity_factor)
            }
            _ => None,
        })
    }
    for (initial, control, title) in [
        (SettingsTab::General, "Appearance", "Solaris Themes"),
        (
            SettingsTab::Appearance,
            "Background & motion",
            "A little atmosphere",
        ),
    ] {
        let mut app = test_app();
        app.tab = Tab::Settings;
        app.settings_tab = initial;
        let ctx = egui::Context::default();
        typography(&ctx);
        let _ = frame(&mut app, &ctx, 0.0, vec![]);
        let settled = frame(&mut app, &ctx, 0.4, vec![]);
        let pos = texts(&settled.shapes)
            .into_iter()
            .find(|(label, _)| label == control)
            .unwrap()
            .1;
        let _ = frame(
            &mut app,
            &ctx,
            1.0,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let clicked = frame(
            &mut app,
            &ctx,
            1.01,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let first = opacity(&clicked, title).expect("New page should appear on its click frame");
        assert!(
            first < 1.0,
            "{title} appeared fully opaque before its reveal started"
        );
        let next = frame(&mut app, &ctx, 1.03, vec![]);
        let next = opacity(&next, title).unwrap();
        assert!(next >= first, "{title} dimmed after first appearing");
        let settled = frame(&mut app, &ctx, 1.3, vec![]);
        assert_eq!(opacity(&settled, title), Some(1.0));
    }
}

#[test]
fn header_uses_the_real_solaris_logo_and_uploads_it_only_once() {
    let ctx = egui::Context::default();
    let mut app = test_app();
    let icon = load_icon().expect("Bundled Solaris logo must decode");
    let size = Vec2::new(820.0, 530.0);
    let first = paint(&mut app, &ctx, size, vec![]);
    let (texture_id, _) = first
        .textures_delta
        .set
        .iter()
        .find(|(_, delta)| delta.image.size() == [icon.width as usize, icon.height as usize])
        .expect("Header must upload the bundled Solaris artwork, not draw a generic pill");
    let second = paint(&mut app, &ctx, size, vec![]);
    assert!(
        !second
            .textures_delta
            .set
            .iter()
            .any(|(id, _)| id == texture_id),
        "An idle header must reuse its GPU texture"
    );
    let primitives = ctx.tessellate(second.shapes, second.pixels_per_point);
    let mesh = primitives
        .iter()
        .find_map(|primitive| match &primitive.primitive {
            egui::epaint::Primitive::Mesh(mesh) if &mesh.texture_id == texture_id => Some(mesh),
            _ => None,
        })
        .expect("The logo texture must actually be painted");
    assert!(
        Rect::from_min_max(egui::pos2(20.0, 0.0), egui::pos2(61.0, 58.0))
            .contains_rect(mesh.calc_bounds())
    );
}

#[test]
fn animated_primary_hover_preserves_text_contrast_in_every_theme() {
    for theme in theme_presets::all() {
        let mut app = test_app();
        app.palette = theme.clone();
        let ctx = egui::Context::default();
        let size = Vec2::new(820.0, 530.0);
        let _ = paint(&mut app, &ctx, size, vec![]);
        let output = paint(&mut app, &ctx, size, vec![]);
        let action_rect = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Rect(shape)
                        if shape.rect.min.y > size.y - 94.0
                            && shape.rect.width() >= 174.0
                            && shape.rect.width() < 180.0
                            && shape.fill.a() > 0 =>
                    {
                        Some((shape.rect, shape.fill))
                    }
                    _ => None,
                })
                .expect("Actual primary action background")
        };
        let (rect, _) = action_rect(&output);
        let _ = paint(
            &mut app,
            &ctx,
            size,
            vec![egui::Event::PointerMoved(rect.center())],
        );
        for _ in 0..24 {
            let output = paint(&mut app, &ctx, size, vec![]);
            let (_, fill) = action_rect(&output);
            assert!(
                theme::contrast(fill, theme.on_accent) >= 4.5,
                "{} primary action loses contrast during hover: {:?}",
                theme.id,
                fill
            );
        }
    }
}

#[test]
fn all_forty_themes_apply_by_gallery_click_and_survive_reloading() {
    let ctx = egui::Context::default();
    let mut app = test_app();
    app.tab = Tab::Settings;
    app.settings_tab = SettingsTab::Appearance;
    let presets = theme_presets::all();
    assert_eq!(presets.len(), 40);
    let mut ids = std::collections::BTreeSet::new();
    for theme in presets {
        app.search = theme.name.clone();
        click(&mut app, &ctx, &theme.name);
        assert_eq!(app.palette.id, theme.id);
        assert_eq!(ctx.style().visuals.panel_fill, theme.background);
        assert_eq!(
            Config::load(&app.layout.config_file()).appearance.theme_id,
            theme.id
        );
        ids.insert(theme.id);
        // These surfaces share the current semantic palette, not stale UI constants.
        for tab in [Tab::Play, Tab::Logs, Tab::Settings] {
            app.tab = tab;
            let _ = paint(&mut app, &ctx, Vec2::new(640.0, 460.0), vec![]);
            assert_eq!(ctx.style().visuals.window_fill, app.palette.surface);
        }
    }
    assert_eq!(ids.len(), 40);
    println!(
        "Gallery click + config readback verified: {}",
        ids.into_iter().collect::<Vec<_>>().join(", ")
    );
}

#[test]
fn favorites_search_and_categories_use_real_catalogue() {
    let ctx = egui::Context::default();
    let mut app = test_app();
    app.tab = Tab::Settings;
    app.settings_tab = SettingsTab::Appearance;
    app.search = "Forest".into();
    click(&mut app, &ctx, "☆");
    assert_eq!(app.config.appearance.favorites, ["forest"]);
    app.search.clear();
    app.filter = Filter::Favorites;
    assert_eq!(app.filtered_themes(false).len(), 1);
    app.filter = Filter::Light;
    assert_eq!(app.filtered_themes(false).len(), 10);
    app.filter = Filter::Dark;
    assert_eq!(app.filtered_themes(false).len(), 10);
    app.filter = Filter::Colorful;
    assert_eq!(app.filtered_themes(false).len(), 20);
    app.search = "LaVeNDer".into();
    assert_eq!(app.filtered_themes(false).len(), 1);
    assert_eq!(
        Config::load(&app.layout.config_file()).appearance.favorites,
        ["forest"]
    );
}

#[test]
fn failed_favorite_save_does_not_change_the_active_catalogue() {
    let mut app = test_app();
    std::fs::create_dir_all(app.layout.config_file()).unwrap();
    app.toggle_favorite("forest");
    assert!(app.config.appearance.favorites.is_empty());
    assert!(app.status.as_ref().is_some_and(|s| s.1));
}

#[test]
fn profiles_and_servers_are_managed_without_corrupting_selections() {
    let ctx = egui::Context::default();
    let mut app = test_app();
    click(&mut app, &ctx, "Profiles");
    assert!(app.dialog == Some(Dialog::Profiles));
    click(&mut app, &ctx, "+ New profile");
    app.draft_name = "SecondPlayer".into();
    click(&mut app, &ctx, "Save & use");
    assert_eq!(app.config.username(), "SecondPlayer");
    assert_eq!(app.config.profiles.len(), 2);
    click(&mut app, &ctx, "Profiles");
    click(&mut app, &ctx, "Player   ·   Offline");
    assert_eq!(app.config.username(), "Player");
    click(&mut app, &ctx, "Done");
    click(&mut app, &ctx, "Servers");
    click(&mut app, &ctx, "+ Add server");
    app.draft_name = "Local fixture".into();
    app.draft_address = "[::1]:25565".into();
    click(&mut app, &ctx, "Save & use");
    assert_eq!(app.config.address(), "[::1]:25565");
    click(&mut app, &ctx, "Servers");
    click(&mut app, &ctx, "Remove");
    assert_eq!(app.config.servers.len(), 1);
    assert_eq!(app.config.address(), "135.106.208.217:25565");
    let saved = Config::load(&app.layout.config_file());
    assert_eq!(saved.profiles.len(), 2);
    assert_eq!(saved.servers.len(), 1);
}

#[test]
fn failed_manager_selection_and_removal_keep_the_saved_profiles() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.config.profiles.push(Profile {
        username: "SecondPlayer".into(),
        skin_path: None,
    });
    app.save_config();
    app.open_manager(Dialog::Profiles);
    std::fs::remove_file(app.layout.config_file()).unwrap();
    std::fs::create_dir(app.layout.config_file()).unwrap();
    click(&mut app, &ctx, "SecondPlayer   ·   Offline");
    assert_eq!(app.config.username(), "Player");
    assert_eq!(app.draft_name, "Player");
    click(&mut app, &ctx, "Remove");
    assert_eq!(app.config.profiles.len(), 2);
    assert_eq!(app.config.username(), "Player");
}

#[test]
fn invalid_drafts_do_not_replace_valid_profile_or_server() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.open_manager(Dialog::Profiles);
    app.draft_name = "bad name".into();
    assert!(!app.save_draft(Dialog::Profiles, &ctx));
    assert_eq!(app.config.username(), "Player");
    app.open_manager(Dialog::Servers);
    app.draft_address = "host:0".into();
    assert!(!app.save_draft(Dialog::Servers, &ctx));
    assert_eq!(app.config.servers.len(), 1);
    app.open_manager(Dialog::Profiles);
    app.draft_skin = "missing-local-skin.png".into();
    assert!(!app.save_draft(Dialog::Profiles, &ctx));
    assert!(app.dialog_error.is_some());
}

#[test]
fn failed_profile_save_keeps_the_dialog_and_current_selection() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.open_manager(Dialog::Profiles);
    app.draft_name = "UnsavedPlayer".into();
    // A directory at the exact target makes atomic replacement fail on all OSes.
    std::fs::create_dir_all(app.layout.config_file()).unwrap();
    click(&mut app, &ctx, "Save & use");
    assert!(
        app.dialog == Some(Dialog::Profiles),
        "A failed save must not dismiss the user's draft"
    );
    assert_eq!(app.config.username(), "Player");
    assert_eq!(app.draft_name, "UnsavedPlayer");
    assert!(
        app.dialog_error
            .as_ref()
            .is_some_and(|s| s.contains("save"))
    );
}

#[test]
fn failed_custom_theme_save_does_not_apply_an_unsaved_palette() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.open_custom(None);
    let original = app.palette.clone();
    std::fs::create_dir_all(app.layout.config_file()).unwrap();
    app.save_custom(&ctx);
    assert!(app.dialog == Some(Dialog::CustomTheme));
    assert_eq!(app.palette, original);
    assert!(app.config.appearance.custom_themes.is_empty());
    assert!(
        app.dialog_error
            .as_ref()
            .is_some_and(|s| s.contains("save"))
    );
}

#[test]
fn custom_theme_create_edit_import_export_and_delete_are_real() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.open_custom(None);
    app.custom_draft.name = "Personal warm".into();
    app.save_custom(&ctx);
    assert_eq!(app.config.appearance.custom_themes.len(), 1);
    assert_eq!(app.palette.name, "Personal warm");
    let custom = app.config.appearance.custom_themes[0].clone();
    app.open_custom(Some(&custom));
    app.custom_draft.name = "Renamed warm".into();
    app.save_custom(&ctx);
    assert_eq!(app.config.appearance.custom_themes.len(), 1);
    let json = theme::export_json(&app.config.appearance.custom_themes[0]).unwrap();
    let path = app.layout.join("export.json");
    export_theme_file(path.to_str().unwrap(), &json).unwrap();
    assert!(export_theme_file(path.to_str().unwrap(), "do not overwrite").is_err());
    assert_eq!(
        read_theme_file(path.to_str().unwrap()).unwrap().name,
        "Renamed warm"
    );
    app.tab = Tab::Settings;
    app.settings_tab = SettingsTab::Appearance;
    app.dialog = None;
    app.search = "not-found".into();
    click(&mut app, &ctx, "Delete");
    assert!(app.config.appearance.custom_themes.is_empty());
    assert_eq!(app.palette.id, "graphite");
}

#[test]
fn failed_theme_deletion_keeps_the_saved_palette_and_favorite() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.open_custom(None);
    app.save_custom(&ctx);
    let id = app.palette.id.clone();
    app.toggle_favorite(&id);
    std::fs::remove_file(app.layout.config_file()).unwrap();
    std::fs::create_dir(app.layout.config_file()).unwrap();
    app.tab = Tab::Settings;
    app.settings_tab = SettingsTab::Appearance;
    app.search = "not-found".into();
    click(&mut app, &ctx, "Delete");
    assert_eq!(app.config.appearance.custom_themes.len(), 1);
    assert_eq!(app.palette.id, id);
    assert_eq!(app.config.appearance.favorites, [id]);
}

#[test]
fn changing_a_custom_theme_id_preserves_its_favorite_after_reload() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.open_custom(None);
    app.save_custom(&ctx);
    let original = app.config.appearance.custom_themes[0].clone();
    app.toggle_favorite(&original.id);
    app.open_custom(Some(&original));
    app.custom_draft.id = "custom-renamed".into();
    app.save_custom(&ctx);
    let reloaded = Config::load(&app.layout.config_file());
    assert_eq!(reloaded.appearance.theme_id, "custom-renamed");
    assert_eq!(reloaded.appearance.favorites, ["custom-renamed"]);
}

#[test]
fn long_custom_names_keep_edit_and_delete_inside_the_minimum_window() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.open_custom(None);
    app.custom_draft.name = "W".repeat(64);
    app.save_custom(&ctx);
    app.tab = Tab::Settings;
    app.settings_tab = SettingsTab::Appearance;
    app.search = "not-found".into();
    let size = Vec2::new(640.0, 460.0);
    let _ = paint(&mut app, &ctx, size, vec![]);
    let output = paint(&mut app, &ctx, size, vec![]);
    for label in ["Edit / export", "Delete"] {
        let (clip, bounds) = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == label => {
                    Some((s.clip_rect, Rect::from_min_size(t.pos, t.galley.size())))
                }
                _ => None,
            })
            .expect("Custom theme action should be painted");
        assert!(
            clip.contains_rect(bounds),
            "{label} is clipped: {bounds:?}, clip={clip:?}"
        );
    }
}

#[test]
fn all_shell_surfaces_render_at_default_minimum_and_simulated_dpi() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    for ppp in [1.0, 1.25, 1.5] {
        ctx.set_pixels_per_point(ppp);
        for size in [Vec2::new(820.0, 530.0), Vec2::new(640.0, 460.0)] {
            for tab in [Tab::Play, Tab::Settings, Tab::Logs] {
                app.tab = tab;
                for settings_tab in [
                    SettingsTab::General,
                    SettingsTab::Appearance,
                    SettingsTab::Minecraft,
                    SettingsTab::Advanced,
                ] {
                    app.settings_tab = settings_tab;
                    for background in [false, true] {
                        app.config.appearance.background_enabled = background;
                        let output = paint(&mut app, &ctx, size, vec![]);
                        assert!(!output.shapes.is_empty());
                        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
                        assert!(!primitives.is_empty());
                    }
                }
            }
            for dialog in [Dialog::Profiles, Dialog::Servers, Dialog::CustomTheme] {
                if dialog == Dialog::CustomTheme {
                    app.open_custom(None);
                } else {
                    app.open_manager(dialog);
                }
                let output = paint(&mut app, &ctx, size, vec![]);
                assert!(!output.shapes.is_empty());
                app.dialog = None;
            }
        }
    }
}

#[test]
fn client_settings_merge_does_not_erase_game_options() {
    let mut app = test_app();
    let settings = [
        ("fov".into(), serde_json::json!(92)),
        ("fullscreen".into(), serde_json::json!(false)),
    ]
    .into_iter()
    .collect();
    config::save_client_settings(&app.layout.client_settings(), &settings).unwrap();
    app.save_setting("render_distance", serde_json::json!(32));
    let saved = config::load_client_settings(&app.layout.client_settings());
    assert_eq!(saved["fov"], 92);
    assert_eq!(saved["render_distance"], 32);
    assert_eq!(config::render_distance(&saved), 32);
}

#[test]
fn subsequent_launch_uses_the_games_current_fullscreen_setting() {
    let mut app = test_app();
    app.save_setting("fullscreen", serde_json::json!(true));
    assert!(app.launch_spec().fullscreen);
    let mut game_settings = app.settings.clone();
    game_settings.insert("fullscreen".into(), serde_json::json!(false));
    config::save_client_settings(&app.layout.client_settings(), &game_settings).unwrap();
    assert!(
        !app.launch_spec().fullscreen,
        "Play must not reapply a stale fullscreen override"
    );
}

#[test]
fn active_work_prevents_competing_launcher_game_settings_writes() {
    let mut app = test_app();
    app.save_setting("render_distance", serde_json::json!(8));
    let before = std::fs::read(app.layout.client_settings()).unwrap();
    let (_tx, rx) = mpsc::channel();
    app.phase = Phase::Installing {
        play_after: false,
        done: rx,
    };
    app.save_setting("render_distance", serde_json::json!(12));
    assert_eq!(std::fs::read(app.layout.client_settings()).unwrap(), before);
    assert_eq!(config::render_distance(&app.settings), 8);
}

#[test]
fn unreadable_or_corrupt_game_settings_are_not_replaced_with_defaults() {
    let mut app = test_app();
    let path = app.layout.client_settings();
    let damaged = b"{\"fov\":92,\"fullscreen\":";
    std::fs::write(&path, damaged).unwrap();
    let before = app.settings.clone();
    app.save_setting("render_distance", serde_json::json!(12));
    assert_eq!(std::fs::read(&path).unwrap(), damaged);
    assert_eq!(app.settings, before);
    assert!(app.status.as_ref().is_some_and(|s| s.1));
}

#[test]
fn cancel_worker_failure_and_unknown_progress_remain_honest() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    let (tx, rx) = mpsc::channel();
    app.phase = Phase::Installing {
        play_after: false,
        done: rx,
    };
    app.rep.stage(1, 5, "Checking files");
    click(&mut app, &ctx, "Cancel");
    assert!(app.rep.check_cancel().is_err());
    let output = paint(&mut app, &ctx, Vec2::new(820.0, 530.0), vec![]);
    assert!(
        texts(&output.shapes)
            .iter()
            .any(|(s, _)| s == "Working · total not yet known")
    );
    let before = app.rep.logs().len();
    app.start_play(&ctx);
    assert_eq!(
        app.rep.logs().len(),
        before,
        "Repeated Play must not start another worker"
    );
    tx.send(Err("Cancelled".into())).unwrap();
    app.poll();
    assert!(!app.busy());
    assert!(!app.status.as_ref().unwrap().1);
    // Match start_install's precondition for an independent second worker.
    app.rep.reset_cancel();
    let (tx, rx) = mpsc::channel();
    app.phase = Phase::Installing {
        play_after: false,
        done: rx,
    };
    drop(tx);
    app.poll();
    assert!(!app.busy());
    assert!(app.status.as_ref().unwrap().1);
}

#[test]
fn play_preserves_a_settings_save_error_instead_of_starting_work() {
    let mut app = test_app();
    let blocked = app.layout.join("blocked-data-root");
    std::fs::write(&blocked, b"not a directory").unwrap();
    // Bootstrap itself also refuses this root before HTTP; no network is used.
    app.layout = Layout::new(blocked);
    app.start_play(&egui::Context::default());
    assert!(!app.busy(), "A failed prerequisite save must stop Play");
    assert!(
        app.status
            .as_ref()
            .is_some_and(|s| s.1 && s.0.contains("Could not save"))
    );
}

#[test]
fn invalid_command_line_identity_never_launches_or_installs() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.args_username = Some("bad name".into());
    app.start_play(&ctx);
    assert!(!app.busy());
    assert!(app.status.unwrap().1);
}

#[test]
fn minimum_window_keeps_the_entire_theme_filter_row_visible() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.tab = Tab::Settings;
    app.settings_tab = SettingsTab::Appearance;
    let size = Vec2::new(640.0, 460.0);
    let _ = paint(&mut app, &ctx, size, vec![]);
    let output = paint(&mut app, &ctx, size, vec![]);
    for label in ["All", "Dark", "Light", "Colorful", "Favorites"] {
        let visible = output.shapes.iter().any(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text == label => s
                .clip_rect
                .contains_rect(Rect::from_min_size(t.pos, t.galley.size())),
            _ => false,
        });
        assert!(
            visible,
            "Theme filter {label} is outside the minimum viewport"
        );
    }
}

#[test]
fn minimum_profile_dialog_keeps_save_visible_with_a_full_list_and_error() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    for i in 0..5 {
        app.config.profiles.push(Profile {
            username: format!("Player{i}"),
            skin_path: None,
        });
    }
    app.open_manager(Dialog::Profiles);
    app.dialog_error = Some("Could not save launcher settings: Creating settings directory: Access is denied. Check data folder permissions.".into());
    let size = Vec2::new(640.0, 460.0);
    let _ = paint(&mut app, &ctx, size, vec![]);
    let output = paint(&mut app, &ctx, size, vec![]);
    let (_, pos) = texts(&output.shapes)
        .into_iter()
        .find(|(label, _)| label == "Save & use")
        .expect("Save action must be painted");
    assert!(
        pos.y - 14.0 >= 0.0 && pos.y + 14.0 <= size.y,
        "Save is outside the viewport: {pos:?}"
    );
}

#[test]
fn minimum_window_keeps_dialog_actions_inside_the_viewport() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    let size = Vec2::new(640.0, 460.0);
    for dialog in [Dialog::Profiles, Dialog::Servers, Dialog::CustomTheme] {
        if dialog == Dialog::CustomTheme {
            app.open_custom(None);
        } else {
            app.open_manager(dialog);
        }
        let _ = paint(&mut app, &ctx, size, vec![]);
        let output = paint(&mut app, &ctx, size, vec![]);
        let labels = texts(&output.shapes);
        let action = if dialog == Dialog::CustomTheme {
            "Save & apply"
        } else {
            "Save & use"
        };
        let (_, pos) = labels
            .iter()
            .find(|(label, _)| label == action)
            .expect("Dialog action exists");
        assert!(
            pos.y + 14.0 <= size.y && pos.x <= size.x,
            "{action} outside minimum window: {pos:?}"
        );
        app.dialog = None;
    }
}

#[test]
fn config_damage_unknown_theme_and_save_failure_are_handled() {
    let mut app = test_app();
    let raw = r#"{"profiles":[{"username":"KeepMe"}],"appearance":{"theme_id":42},"window":null}"#;
    let mut config: Config = serde_json::from_str(raw).unwrap();
    config.normalize();
    assert_eq!(config.username(), "KeepMe");
    assert_eq!(config.appearance.theme_id, "graphite");
    config.appearance.theme_id = "unknown-id".into();
    config.window.width = 1.0;
    config.window.height = 99.0;
    config.normalize();
    assert_eq!(config.appearance.theme_id, "graphite");
    assert_eq!((config.window.width, config.window.height), (640.0, 460.0));
    // A file cannot be the parent directory for a settings file.
    let invalid_root = app.layout.join("file-not-directory");
    std::fs::write(&invalid_root, "fixture").unwrap();
    app.layout = Layout::new(invalid_root);
    app.save_config();
    assert!(app.status.as_ref().unwrap().1);
    assert!(app.status.as_ref().unwrap().0.contains("Could not save"));
}

#[test]
fn a_late_cancel_wins_over_completed_installation_and_autoplay() {
    let mut app = test_app();
    let (tx, rx) = mpsc::channel();
    app.phase = Phase::Installing {
        play_after: true,
        done: rx,
    };
    app.rep.cancel();
    tx.send(Ok(())).unwrap();
    app.poll();
    assert!(!app.busy());
    let (status, error) = app.status.as_ref().expect("Cancellation status");
    assert!(
        !error,
        "Cancellation must not become a missing-file or launch error"
    );
    assert!(status.contains("cancelled"));
}

#[test]
fn landscape_cover_fills_the_banner_instead_of_shrinking_to_source_aspect() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.config.appearance.background_enabled = true;
    let texture = app.media.background(&ctx, None).unwrap();
    let size = Vec2::new(820.0, 530.0);
    for settings in [false, true] {
        app.tab = if settings { Tab::Settings } else { Tab::Play };
        app.settings_tab = SettingsTab::Appearance;
        app.appearance_tab = AppearanceTab::Background;
        let _ = paint(&mut app, &ctx, size, vec![]);
        let output = paint(&mut app, &ctx, size, vec![]);
        let primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
        let rect = primitives
            .iter()
            .find_map(|s| match &s.primitive {
                egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture.id() => {
                    Some(mesh.calc_bounds())
                }
                _ => None,
            })
            .expect("Actual landscape image mesh is submitted");
        assert!(
            rect.width() >= size.x - 64.0,
            "Banner must fill the content width, got {rect:?}"
        );
        // Rounded-image tessellation adds a half-point AA fringe per edge.
        assert!(rect.height() <= 101.1, "Banner must stay compact: {rect:?}");
    }
}

#[test]
fn standard_home_with_banner_keeps_offline_notice_visible_without_scrolling() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.config.appearance.background_enabled = true;
    let size = Vec2::new(820.0, 530.0);
    let _ = paint(&mut app, &ctx, size, vec![]);
    let output = paint(&mut app, &ctx, size, vec![]);
    let notice = output
        .shapes
        .iter()
        .find_map(|s| match &s.shape {
            egui::Shape::Text(t) if t.galley.job.text.starts_with("Offline identity only.") => {
                Some((s.clip_rect, Rect::from_min_size(t.pos, t.galley.size())))
            }
            _ => None,
        })
        .expect("Offline notice should be painted at the standard window size");
    assert!(
        notice.0.contains_rect(notice.1),
        "Offline notice is clipped: {notice:?}"
    );
}

#[test]
fn compact_home_prioritizes_game_identity_over_optional_artwork() {
    let mut app = test_app();
    let ctx = egui::Context::default();
    app.config.appearance.background_enabled = true;
    let size = Vec2::new(640.0, 460.0);
    let _ = paint(&mut app, &ctx, size, vec![]);
    let output = paint(&mut app, &ctx, size, vec![]);
    for label in [
        format!("Minecraft {GAME_VERSION}"),
        "Offline identity only. Connect to servers that allow offline profiles.".into(),
    ] {
        let (_, center) = texts(&output.shapes)
            .into_iter()
            .find(|(text, _)| text == &label)
            .expect("Game identity must remain visible");
        assert!(
            center.y + 10.0 < size.y - 94.0,
            "Identity hidden behind action area: {center:?}"
        );
    }
}
