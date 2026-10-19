//! The launcher window: warm amber pixel theme matching the client UI.
use crate::{
    Args, bootstrap,
    config::{self, ClientSettings, Config, Profile, Server},
    launch,
    paths::{GAME_VERSION, Layout},
    progress::{Reporter, human_bytes},
};
use anyhow::Result;
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontId, Layout as UiLayout, Margin, Rect, RichText, Sense, Stroke,
    StrokeKind, Vec2,
};
use std::{
    process::Child,
    sync::{Arc, mpsc},
};

// Palette: the client's `ui/theme.rs` constants as sRGB bytes.
const BG: Color32 = Color32::from_rgb(14, 9, 6);
const PANEL: Color32 = Color32::from_rgb(18, 12, 8);
const CONTROL: Color32 = Color32::from_rgb(34, 23, 15);
const CONTROL_HOT: Color32 = Color32::from_rgb(64, 40, 19);
const CONTROL_DOWN: Color32 = Color32::from_rgb(24, 16, 10);
const RIM: Color32 = Color32::from_rgb(120, 74, 31);
const ACCENT: Color32 = Color32::from_rgb(255, 163, 46);
const ACCENT_HI: Color32 = Color32::from_rgb(255, 209, 120);
const TRACK: Color32 = Color32::from_rgb(10, 7, 5);
const TEXT: Color32 = Color32::from_rgb(251, 233, 207);
const TEXT_DIM: Color32 = Color32::from_rgb(162, 136, 102);
const TEXT_OFF: Color32 = Color32::from_rgb(122, 102, 80);
const HEADING: Color32 = Color32::from_rgb(255, 184, 74);
const ERROR: Color32 = Color32::from_rgb(255, 110, 90);
const OK: Color32 = Color32::from_rgb(150, 220, 110);

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Play,
    Settings,
    Logs,
}

enum Phase {
    Idle,
    Installing { play_after: bool, done: mpsc::Receiver<Result<(), String>> },
    Running(Child),
}

struct App {
    layout: Layout,
    config: Config,
    settings: ClientSettings,
    tab: Tab,
    rep: Reporter,
    phase: Phase,
    ready: bool,
    status: Option<(String, bool)>,
    manifest: String,
    args_server: Option<String>,
    args_username: Option<String>,
    force_next: bool,
}

pub fn run(args: Args, layout: Layout) -> Result<()> {
    let rep = Reporter::new(Some(&layout.log_file()), false);
    let mut config = Config::load(&layout.config_file());
    if let Some(url) = &args.manifest {
        config.manifest_url = url.clone();
    }
    let tab = match args.tab.as_deref() {
        Some("settings") => Tab::Settings,
        Some("logs") => Tab::Logs,
        _ => Tab::Play,
    };
    let mut app = App {
        settings: config::load_client_settings(&layout.client_settings()),
        ready: bootstrap::is_ready(&layout),
        manifest: config.manifest_url.clone(),
        layout,
        config,
        tab,
        rep,
        phase: Phase::Idle,
        status: None,
        args_server: args.server.clone(),
        args_username: args.username.clone(),
        force_next: args.force,
    };
    let autoplay = args.autoplay;
    let icon = load_icon();
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Solaris Launcher")
        .with_app_id("solaris-launcher")
        .with_inner_size([980.0, 620.0])
        .with_min_inner_size([820.0, 540.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "Solaris Launcher",
        options,
        Box::new(move |cc| {
            apply_theme(&cc.egui_ctx);
            if autoplay {
                app.start_play(&cc.egui_ctx);
            }
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| anyhow::anyhow!("window error: {e}"))
}

fn load_icon() -> Option<egui::IconData> {
    let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../assets/icon.png")));
    let mut reader = decoder.read_info().ok()?;
    let mut buffer = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buffer).ok()?;
    buffer.truncate(info.buffer_size());
    (info.color_type == png::ColorType::Rgba).then_some(egui::IconData {
        rgba: buffer,
        width: info.width,
        height: info.height,
    })
}

fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.override_text_color = Some(TEXT);
    visuals.panel_fill = BG;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = TRACK;
    visuals.faint_bg_color = CONTROL;
    visuals.hyperlink_color = ACCENT_HI;
    visuals.selection.bg_fill = Color32::from_rgba_unmultiplied(255, 163, 46, 90);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT_HI);
    let radius = CornerRadius::same(2);
    let w = &mut visuals.widgets;
    w.noninteractive.bg_fill = PANEL;
    w.noninteractive.bg_stroke = Stroke::new(1.0, RIM);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.bg_fill = Color32::from_rgb(78, 48, 22);
    w.inactive.weak_bg_fill = CONTROL;
    w.inactive.bg_stroke = Stroke::new(1.0, RIM);
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.hovered.bg_fill = CONTROL_HOT;
    w.hovered.weak_bg_fill = CONTROL_HOT;
    w.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
    w.hovered.fg_stroke = Stroke::new(1.5, ACCENT_HI);
    w.active.bg_fill = CONTROL_DOWN;
    w.active.weak_bg_fill = CONTROL_DOWN;
    w.active.bg_stroke = Stroke::new(1.0, ACCENT_HI);
    w.active.fg_stroke = Stroke::new(1.5, ACCENT_HI);
    w.open = w.hovered;
    for state in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        state.corner_radius = radius;
    }
    ctx.set_visuals(visuals);
    ctx.style_mut(|style| {
        use egui::TextStyle::*;
        style.text_styles.insert(Heading, FontId::proportional(24.0));
        style.text_styles.insert(Body, FontId::proportional(15.0));
        style.text_styles.insert(Button, FontId::proportional(15.0));
        style.text_styles.insert(Small, FontId::proportional(12.5));
        style.text_styles.insert(Monospace, FontId::monospace(12.5));
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 6.0);
        style.spacing.interact_size.y = 28.0;
        style.interaction.selectable_labels = false;
    });
}

// 5x7 pixel glyphs for the wordmark.
const GLYPHS: &[(char, [&str; 7])] = &[
    ('S', [".####", "#....", "#....", ".###.", "....#", "....#", "####."]),
    ('O', [".###.", "#...#", "#...#", "#...#", "#...#", "#...#", ".###."]),
    ('L', ["#....", "#....", "#....", "#....", "#....", "#....", "#####"]),
    ('A', [".###.", "#...#", "#...#", "#####", "#...#", "#...#", "#...#"]),
    ('R', ["####.", "#...#", "#...#", "####.", "#.#..", "#..#.", "#...#"]),
    ('I', ["#####", "..#..", "..#..", "..#..", "..#..", "..#..", "#####"]),
];

fn wordmark(ui: &mut egui::Ui, text: &str, cell: f32) {
    let width = text.chars().count() as f32 * 6.0 * cell;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 8.0 * cell), Sense::hover());
    let painter = ui.painter_at(rect.expand(cell));
    for (index, ch) in text.chars().enumerate() {
        let Some((_, rows)) = GLYPHS.iter().find(|(c, _)| *c == ch) else { continue };
        let origin = rect.min + Vec2::new(index as f32 * 6.0 * cell, 0.0);
        for (y, row) in rows.iter().enumerate() {
            for (x, bit) in row.bytes().enumerate() {
                if bit != b'#' {
                    continue;
                }
                let cell_rect = Rect::from_min_size(origin + Vec2::new(x as f32, y as f32) * cell, Vec2::splat(cell));
                painter.rect_filled(cell_rect.translate(Vec2::splat(cell * 0.6)), 0.0, Color32::from_rgb(70, 38, 10));
                let shade = if y < 3 { ACCENT_HI } else { ACCENT };
                painter.rect_filled(cell_rect, 0.0, shade);
            }
        }
    }
}

fn panel() -> egui::Frame {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, RIM))
        .corner_radius(CornerRadius::same(2))
        .inner_margin(Margin::same(14))
}

/// Wrapping, non-selectable status text (selectable labels would steal clicks
/// from overlapping buttons).
fn note(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) {
    ui.add(egui::Label::new(RichText::new(text).color(color)).wrap().selectable(false));
}

fn heading(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).color(HEADING).strong().size(13.0));
}

/// A selectable list row: title + dim subtitle.
fn row(ui: &mut egui::Ui, selected: bool, title: &str, subtitle: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 46.0), Sense::click());
    let fill = if response.is_pointer_button_down_on() {
        CONTROL_DOWN
    } else if response.hovered() {
        CONTROL_HOT
    } else {
        CONTROL
    };
    let painter = ui.painter();
    painter.rect(rect, CornerRadius::same(2), fill, Stroke::new(1.0, if selected { ACCENT } else { RIM }), StrokeKind::Inside);
    if selected {
        painter.rect_filled(Rect::from_min_size(rect.min + Vec2::new(0.0, 4.0), Vec2::new(3.0, rect.height() - 8.0)), 0.0, ACCENT);
    }
    painter.text(rect.min + Vec2::new(14.0, 8.0), Align2::LEFT_TOP, title, FontId::proportional(16.0), TEXT);
    painter.text(rect.min + Vec2::new(14.0, 27.0), Align2::LEFT_TOP, subtitle, FontId::proportional(12.5), TEXT_DIM);
    response
}

fn nav_button(ui: &mut egui::Ui, current: &mut Tab, tab: Tab, label: &str) {
    let selected = *current == tab;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
    let fill = if selected {
        CONTROL_HOT
    } else if response.hovered() {
        CONTROL
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, CornerRadius::same(2), fill);
    if selected {
        ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())), 0.0, ACCENT);
    }
    ui.painter().text(
        rect.min + Vec2::new(16.0, rect.height() / 2.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(16.0),
        if selected { ACCENT_HI } else { TEXT },
    );
    if response.clicked() {
        *current = tab;
    }
}

impl App {
    fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    fn selected_username_ok(&self) -> bool {
        config::valid_username(self.config.username())
    }

    fn address(&self) -> String {
        self.args_server.clone().unwrap_or_else(|| self.config.address().trim().to_owned())
    }

    fn username(&self) -> String {
        self.args_username.clone().unwrap_or_else(|| self.config.username().to_owned())
    }

    fn save_config(&self) {
        self.config.save(&self.layout.config_file());
    }

    fn save_settings(&self) {
        config::save_client_settings(&self.layout.client_settings(), &self.settings);
    }

    fn start_install(&mut self, ctx: &egui::Context, play_after: bool, force: bool) {
        self.status = None;
        self.rep.reset_cancel();
        let (tx, rx) = mpsc::channel();
        let (layout_root, rep) = (self.layout.root.clone(), self.rep.clone());
        let options = bootstrap::Options {
            manifest_url: self.manifest.clone(),
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let layout = Layout::new(layout_root);
            let result = bootstrap::run(&layout, &options, force, &rep).map_err(|error| {
                if error.downcast_ref::<crate::net::UserCancelled>().is_some() {
                    "Cancelled".to_owned()
                } else {
                    format!("{error:#}")
                }
            });
            if let Err(message) = &result {
                rep.log(format!("ERROR: {message}"));
            }
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.phase = Phase::Installing { play_after, done: rx };
    }

    fn start_client(&mut self) {
        let spec = launch::Spec {
            root: self.layout.root.clone(),
            server: self.address(),
            username: self.username(),
            fullscreen: config::fullscreen(&self.settings),
        };
        match launch::spawn(&spec, &self.rep) {
            Ok(child) => {
                self.status = None;
                self.phase = Phase::Running(child);
            }
            Err(error) => {
                self.rep.log(format!("ERROR: {error:#}"));
                self.status = Some((format!("{error:#}"), true));
                self.phase = Phase::Idle;
            }
        }
    }

    /// Play: launch directly when installed, otherwise install first.
    fn start_play(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        self.save_config();
        let force = std::mem::take(&mut self.force_next);
        if self.ready && !force {
            self.start_client();
        } else {
            self.start_install(ctx, true, force);
        }
    }

    fn poll(&mut self) {
        let mut finished = None;
        match &mut self.phase {
            Phase::Installing { play_after, done } => {
                if let Ok(result) = done.try_recv() {
                    finished = Some((*play_after, result));
                }
            }
            Phase::Running(child) => {
                if let Ok(Some(status)) = child.try_wait() {
                    self.rep.log(format!("Client exited: {status}"));
                    if !status.success() {
                        self.status = Some((format!("Client exited with {status} - see Logs"), true));
                    }
                    self.phase = Phase::Idle;
                }
            }
            Phase::Idle => {}
        }
        if let Some((play_after, result)) = finished {
            self.phase = Phase::Idle;
            self.ready = bootstrap::is_ready(&self.layout);
            match result {
                Ok(()) if play_after && self.ready => self.start_client(),
                Ok(()) => self.status = Some(("Game files are ready.".into(), false)),
                Err(message) => self.status = Some((message, true)),
            }
        }
    }

    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("bottom")
            .exact_height(120.0)
            .frame(egui::Frame::new().fill(PANEL).stroke(Stroke::new(1.0, RIM)).inner_margin(Margin::symmetric(20, 14)))
            .show(ctx, |ui| {
                ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                    self.play_button(ui, ctx);
                    ui.with_layout(UiLayout::left_to_right(Align::Center), |ui| self.status_area(ui));
                });
            });
    }

    fn play_button(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let valid = self.selected_username_ok() && config::valid_address(&self.address());
        let (label, enabled) = match &self.phase {
            Phase::Idle if self.ready => ("PLAY", valid),
            Phase::Idle => ("INSTALL & PLAY", valid),
            Phase::Installing { .. } => ("INSTALLING...", false),
            Phase::Running(_) => ("RUNNING", false),
        };
        let button = egui::Button::new(RichText::new(label).size(20.0).strong().color(if enabled { Color32::from_rgb(40, 22, 6) } else { TEXT_OFF }))
            .fill(if enabled { ACCENT } else { CONTROL })
            .stroke(Stroke::new(1.0, if enabled { ACCENT_HI } else { RIM }))
            .min_size(Vec2::new(210.0, 56.0));
        if ui.add_enabled(enabled, button).clicked() {
            self.start_play(ctx);
        }
        if let Phase::Running(child) = &mut self.phase
            && ui.button("Stop").clicked()
        {
            let _ = child.kill();
        }
        if matches!(self.phase, Phase::Installing { .. }) && ui.button("Cancel").clicked() {
            self.rep.cancel();
        }
    }

    fn status_area(&mut self, ui: &mut egui::Ui) {
        match &self.phase {
            Phase::Installing { .. } => {
                let snap = self.rep.snapshot();
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(
                        RichText::new(format!("Step {}/{}  -  {}", snap.stage_index, snap.stage_count, snap.title))
                            .color(HEADING)
                            .strong(),
                    );
                    // Unknown totals (unpacking, deriving) show a sweeping bar instead of a stuck zero.
                    let fraction = if snap.total > 0 {
                        (snap.done as f32 / snap.total as f32).clamp(0.0, 1.0)
                    } else {
                        (ui.input(|i| i.time) as f32 % 2.0) / 2.0
                    };
                    let detail = if snap.total > 0 {
                        format!(
                            "{} / {}   {}/s",
                            human_bytes(snap.done),
                            human_bytes(snap.total),
                            human_bytes(snap.bytes_per_second as u64)
                        )
                    } else {
                        "working...".to_owned()
                    };
                    let width = (ui.available_width() - 20.0).max(200.0);
                    ui.add(egui::ProgressBar::new(fraction).desired_width(width).desired_height(16.0).fill(ACCENT));
                    ui.horizontal(|ui| {
                        ui.set_width(width);
                        note(ui, detail, TEXT);
                    });
                    let stage_fraction = if snap.total > 0 { fraction } else { 0.0 };
                    let overall = ((snap.stage_index.saturating_sub(1)) as f32 + stage_fraction) / snap.stage_count.max(1) as f32;
                    ui.add(egui::ProgressBar::new(overall.clamp(0.0, 1.0)).desired_width(width).desired_height(5.0).fill(RIM));
                });
            }
            Phase::Running(_) => {
                note(ui, format!("Playing as {} on {}", self.username(), self.address()), OK);
            }
            Phase::Idle => {
                if let Some((message, error)) = &self.status {
                    note(ui, message.clone(), if *error { ERROR } else { OK });
                } else if self.ready {
                    note(ui, format!("Minecraft {GAME_VERSION} content ready"), TEXT_DIM);
                } else {
                    note(
                        ui,
                        "First run: downloads about 670 MB of game content from Mojang, Adoptium and the Solaris server.",
                        TEXT_DIM,
                    );
                }
            }
        }
    }

    fn sidebar(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("nav")
            .exact_width(210.0)
            .frame(egui::Frame::new().fill(PANEL).stroke(Stroke::new(1.0, RIM)).inner_margin(Margin::same(16)))
            .show(ctx, |ui| {
                ui.add_space(6.0);
                wordmark(ui, "SOLARIS", 4.0);
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Minecraft {GAME_VERSION}")).color(TEXT_DIM).size(12.5));
                ui.add_space(22.0);
                nav_button(ui, &mut self.tab, Tab::Play, "Play");
                nav_button(ui, &mut self.tab, Tab::Settings, "Settings");
                nav_button(ui, &mut self.tab, Tab::Logs, "Logs");
                ui.with_layout(UiLayout::bottom_up(Align::LEFT), |ui| {
                    ui.label(RichText::new(format!("launcher {}", env!("CARGO_PKG_VERSION"))).color(TEXT_OFF).size(11.5));
                });
            });
    }

    fn play_tab(&mut self, ui: &mut egui::Ui) {
        ui.columns(2, |cols| {
            let ui = &mut cols[0];
            panel().show(ui, |ui| {
                ui.set_min_height(380.0);
                heading(ui, "PROFILE");
                let mut clicked = None;
                for (index, profile) in self.config.profiles.iter().enumerate() {
                    if row(ui, index == self.config.selected_profile, &profile.username, "Offline account").clicked() {
                        clicked = Some(index);
                    }
                }
                if let Some(index) = clicked {
                    self.config.selected_profile = index;
                    self.save_config();
                }
                ui.add_space(4.0);
                ui.label(RichText::new("Username").color(TEXT_DIM).size(12.5));
                let selected = self.config.selected_profile;
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.config.profiles[selected].username)
                        .desired_width(f32::INFINITY)
                        .char_limit(16),
                );
                if response.changed() {
                    self.save_config();
                }
                if !self.selected_username_ok() {
                    ui.label(RichText::new("3-16 letters, digits or underscore").color(ERROR).size(12.5));
                }
                ui.horizontal(|ui| {
                    if ui.button("+ New profile").clicked() {
                        self.config.profiles.push(Profile {
                            username: format!("Player{}", self.config.profiles.len() + 1),
                        });
                        self.config.selected_profile = self.config.profiles.len() - 1;
                        self.save_config();
                    }
                    if ui.add_enabled(self.config.profiles.len() > 1, egui::Button::new("Remove")).clicked() {
                        self.config.profiles.remove(self.config.selected_profile);
                        self.config.normalize();
                        self.save_config();
                    }
                });
                ui.add_space(6.0);
                ui.add_enabled(
                    false,
                    egui::Button::new(RichText::new("Sign in with Microsoft").color(TEXT_OFF)).min_size(Vec2::new(ui.available_width(), 34.0)),
                )
                .on_disabled_hover_text("Microsoft login is not implemented yet; this preview uses offline accounts.");
                ui.label(RichText::new("Microsoft login: coming later").color(TEXT_OFF).size(12.0));
            });

            let ui = &mut cols[1];
            panel().show(ui, |ui| {
                ui.set_min_height(380.0);
                heading(ui, "SERVER");
                let mut clicked = None;
                for (index, server) in self.config.servers.iter().enumerate() {
                    if row(ui, index == self.config.selected_server, &server.name, &server.address).clicked() {
                        clicked = Some(index);
                    }
                }
                if let Some(index) = clicked {
                    self.config.selected_server = index;
                    self.save_config();
                }
                ui.add_space(4.0);
                let selected = self.config.selected_server;
                let mut changed = false;
                ui.label(RichText::new("Name").color(TEXT_DIM).size(12.5));
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut self.config.servers[selected].name).desired_width(f32::INFINITY))
                    .changed();
                ui.label(RichText::new("Address (host[:port])").color(TEXT_DIM).size(12.5));
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut self.config.servers[selected].address).desired_width(f32::INFINITY))
                    .changed();
                if changed {
                    self.save_config();
                }
                if !config::valid_address(&self.config.servers[selected].address) {
                    ui.label(RichText::new("Enter host or host:port").color(ERROR).size(12.5));
                }
                ui.horizontal(|ui| {
                    if ui.button("+ Add server").clicked() {
                        self.config.servers.push(Server {
                            name: "New server".into(),
                            address: "127.0.0.1:25565".into(),
                        });
                        self.config.selected_server = self.config.servers.len() - 1;
                        self.save_config();
                    }
                    if ui.add_enabled(self.config.servers.len() > 1, egui::Button::new("Remove")).clicked() {
                        self.config.servers.remove(self.config.selected_server);
                        self.config.normalize();
                        self.save_config();
                    }
                });
            });
        });
    }

    fn settings_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        panel().show(ui, |ui| {
            heading(ui, "GAME");
            let mut distance = config::render_distance(&self.settings);
            ui.horizontal(|ui| {
                ui.label("Render distance");
                ui.spacing_mut().slider_width = 240.0;
                if ui.add(egui::Slider::new(&mut distance, 2..=16).suffix(" chunks")).changed() {
                    self.settings.insert("render_distance".into(), serde_json::json!(distance));
                    self.save_settings();
                }
            });
            let mut fullscreen = config::fullscreen(&self.settings);
            if ui.checkbox(&mut fullscreen, "Start in fullscreen").changed() {
                self.settings.insert("fullscreen".into(), serde_json::json!(fullscreen));
                self.save_settings();
            }
            ui.label(RichText::new("More options (FOV, volume, GUI scale, VSync) are in the game's Esc > Options screen.").color(TEXT_DIM).size(12.5));
        });
        ui.add_space(10.0);
        panel().show(ui, |ui| {
            heading(ui, "CONTENT");
            ui.horizontal(|ui| {
                ui.label("Data folder");
                ui.label(RichText::new(self.layout.root.display().to_string()).monospace().color(ACCENT_HI));
                if ui.button("Open").clicked() {
                    let _ = std::fs::create_dir_all(&self.layout.root);
                    let opener = if cfg!(windows) { "explorer" } else { "xdg-open" };
                    let _ = std::process::Command::new(opener).arg(&self.layout.root).spawn();
                }
            });
            ui.horizontal(|ui| {
                ui.label("Content manifest");
                if ui.add(egui::TextEdit::singleline(&mut self.manifest).desired_width(f32::INFINITY)).changed() {
                    self.config.manifest_url = self.manifest.trim().to_owned();
                    self.save_config();
                }
            });
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.busy(), egui::Button::new("Verify / repair game files")).clicked() {
                    self.start_install(ctx, false, true);
                }
                ui.label(
                    RichText::new("Re-checks every download against its hash and re-derives local data.")
                        .color(TEXT_DIM)
                        .size(12.5),
                );
            });
        });
    }

    fn logs_tab(&mut self, ui: &mut egui::Ui) {
        let lines = self.rep.logs();
        ui.horizontal(|ui| {
            heading(ui, "LOG");
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(lines.join("\n"));
            }
            if ui.button("Clear").clicked() {
                self.rep.clear_logs();
            }
            ui.label(RichText::new(self.layout.log_file().display().to_string()).color(TEXT_OFF).size(12.0));
        });
        egui::Frame::new()
            .fill(TRACK)
            .stroke(Stroke::new(1.0, RIM))
            .corner_radius(CornerRadius::same(2))
            .inner_margin(Margin::same(10))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                    ui.style_mut().interaction.selectable_labels = true;
                    for line in &lines {
                        let color = if line.starts_with("ERROR") {
                            ERROR
                        } else if line.starts_with('[') && !line.starts_with("[client]") {
                            HEADING
                        } else {
                            TEXT
                        };
                        ui.label(RichText::new(line).monospace().color(color));
                    }
                    if lines.is_empty() {
                        ui.label(RichText::new("Nothing logged yet.").color(TEXT_OFF));
                    }
                });
            });
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.sidebar(ctx);
        self.bottom_bar(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::same(20)))
            .show(ctx, |ui| {
                match self.tab {
                    Tab::Play => self.play_tab(ui),
                    Tab::Settings => self.settings_tab(ui, ctx),
                    Tab::Logs => self.logs_tab(ui),
                }
            });
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_config();
        if let Phase::Running(child) = &mut self.phase {
            // Leave a running game alone; the launcher closing is not a quit request.
            let _ = child.try_wait();
        }
    }
}
