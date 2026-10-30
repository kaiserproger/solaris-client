//! Native Soft Minimal launcher. Game installation/launch remain on the existing paths.
use crate::{
    Args, bootstrap,
    config::{self, ClientSettings, Config, Profile, Server},
    launch,
    media::{self, MediaCache},
    motion,
    paths::{GAME_VERSION, Layout},
    progress::{Reporter, human_bytes},
    theme::{self, CustomTheme, Theme, ThemeCategory},
    theme_presets,
};
use anyhow::Result;
use eframe::egui::{
    self, Align, Align2, Color32, FontId, Layout as UiLayout, Margin, Rect, RichText, Sense,
    Stroke, StrokeKind, Vec2,
};
use std::{
    process::Child,
    sync::{Arc, mpsc},
};

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Play,
    Settings,
    Logs,
}
#[derive(Clone, Copy, PartialEq)]
enum SettingsTab {
    General,
    Appearance,
    Minecraft,
    Advanced,
}
#[derive(Clone, Copy, PartialEq)]
enum AppearanceTab {
    Themes,
    Background,
}
#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Dark,
    Light,
    Colorful,
    Favorites,
}
#[derive(Clone, Copy, PartialEq)]
enum Dialog {
    Profiles,
    Servers,
    CustomTheme,
}

enum Phase {
    Idle,
    Installing {
        play_after: bool,
        done: mpsc::Receiver<Result<(), String>>,
    },
    Running(Child),
}

struct App {
    layout: Layout,
    config: Config,
    settings: ClientSettings,
    tab: Tab,
    settings_tab: SettingsTab,
    appearance_tab: AppearanceTab,
    rep: Reporter,
    phase: Phase,
    ready: bool,
    status: Option<(String, bool)>,
    manifest: String,
    args_server: Option<String>,
    args_username: Option<String>,
    force_next: bool,
    palette: Theme,
    themes: Vec<Theme>,
    media: MediaCache,
    logo: Option<egui::TextureHandle>,
    page_reveal: motion::Reveal,
    dialog_reveal: motion::Reveal,
    dialog_opacity: f32,
    filter: Filter,
    search: String,
    dialog: Option<Dialog>,
    editing: Option<usize>,
    draft_name: String,
    draft_address: String,
    draft_skin: String,
    custom_draft: CustomTheme,
    custom_original: Option<String>,
    custom_json: String,
    transfer_path: String,
    dialog_error: Option<String>,
}

pub fn run(args: Args, layout: Layout) -> Result<()> {
    let mut app = App::new(layout);
    if let Some(url) = args.manifest {
        app.manifest = url;
    }
    app.args_server = args.server;
    app.args_username = args.username;
    app.force_next = args.force;
    app.tab = match args.tab.as_deref() {
        Some("settings") => Tab::Settings,
        Some("logs") => Tab::Logs,
        _ => Tab::Play,
    };
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Solaris Launcher")
        .with_app_id("solaris-launcher")
        .with_inner_size([app.config.window.width, app.config.window.height])
        .with_min_inner_size([640.0, 460.0])
        .with_resizable(true);
    if let Some(icon) = load_icon() {
        viewport = viewport.with_icon(Arc::new(icon));
    }
    eframe::run_native(
        "Solaris Launcher",
        eframe::NativeOptions {
            viewport,
            ..Default::default()
        },
        Box::new(move |cc| {
            typography(&cc.egui_ctx);
            theme::apply(
                &cc.egui_ctx,
                &app.palette,
                app.config.appearance.reduced_motion,
            );
            if args.autoplay {
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
    let mut rgba = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut rgba).ok()?;
    rgba.truncate(info.buffer_size());
    (info.color_type == png::ColorType::Rgba).then_some(egui::IconData {
        rgba,
        width: info.width,
        height: info.height,
    })
}

fn typography(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        use egui::TextStyle::*;
        style
            .text_styles
            .insert(Heading, FontId::proportional(26.0));
        style.text_styles.insert(Body, FontId::proportional(14.0));
        style.text_styles.insert(Button, FontId::proportional(14.0));
        style.text_styles.insert(Small, FontId::proportional(12.5));
        style.text_styles.insert(Monospace, FontId::monospace(12.5));
        style.spacing.item_spacing = Vec2::new(10.0, 8.0);
        style.spacing.button_padding = Vec2::new(12.0, 7.0);
        style.spacing.interact_size.y = 30.0;
        style.interaction.selectable_labels = false;
    });
}

fn note(ui: &mut egui::Ui, text: impl Into<String>, color: Color32) -> egui::Response {
    ui.add(
        egui::Label::new(RichText::new(text).color(color))
            .wrap()
            .selectable(false),
    )
}

fn section(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).size(21.0).strong());
    ui.weak(subtitle);
    ui.add_space(12.0);
}

fn text_field(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize) -> egui::Response {
    ui.label(label);
    ui.add(
        egui::TextEdit::singleline(value)
            .desired_width(f32::INFINITY)
            .char_limit(limit),
    )
}

impl App {
    fn new(layout: Layout) -> Self {
        let config = Config::load(&layout.config_file());
        let palette = theme::resolve(
            &config.appearance.theme_id,
            &config.appearance.custom_themes,
        );
        let mut themes = theme_presets::all();
        themes.extend(
            config
                .appearance
                .custom_themes
                .iter()
                .filter_map(|t| t.validate().ok()),
        );
        Self {
            settings: config::load_client_settings(&layout.client_settings()),
            ready: bootstrap::is_ready(&layout),
            manifest: config.manifest_url.clone(),
            rep: Reporter::new(Some(&layout.log_file()), false),
            layout,
            config,
            palette,
            themes,
            tab: Tab::Play,
            settings_tab: SettingsTab::General,
            appearance_tab: AppearanceTab::Themes,
            phase: Phase::Idle,
            status: None,
            args_server: None,
            args_username: None,
            force_next: false,
            media: MediaCache::default(),
            logo: None,
            page_reveal: motion::Reveal::default(),
            dialog_reveal: motion::Reveal::default(),
            dialog_opacity: 1.0,
            filter: Filter::All,
            search: String::new(),
            dialog: None,
            editing: None,
            draft_name: String::new(),
            draft_address: String::new(),
            draft_skin: String::new(),
            custom_draft: CustomTheme::default(),
            custom_original: None,
            custom_json: String::new(),
            transfer_path: String::new(),
            dialog_error: None,
        }
    }
    fn busy(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }
    fn address(&self) -> String {
        self.args_server
            .clone()
            .unwrap_or_else(|| self.config.address().trim().to_owned())
    }
    fn username(&self) -> String {
        self.args_username
            .clone()
            .unwrap_or_else(|| self.config.username().to_owned())
    }
    fn valid_selection(&self) -> bool {
        config::valid_username(&self.username()) && config::valid_address(&self.address())
    }
    fn error(&mut self, message: impl Into<String>) {
        let message = message.into();
        self.rep.log(format!("ERROR: {message}"));
        self.status = Some((message, true));
    }
    fn save_config(&mut self) -> bool {
        self.commit_config(self.config.clone())
    }
    /// Publish a dialog's candidate only after durable settings replacement.
    /// Failed saves retain both the active selection and the editable draft.
    fn commit_config(&mut self, candidate: Config) -> bool {
        match candidate.save(&self.layout.config_file()) {
            Ok(()) => {
                self.config = candidate;
                self.dialog_error = None;
                true
            }
            Err(error) => {
                let message = format!("Could not save launcher settings: {error:#}");
                self.dialog_error = Some(message.clone());
                self.error(message);
                false
            }
        }
    }
    fn save_setting(&mut self, key: &str, value: serde_json::Value) {
        // The running Session owns its snapshot and can save it at any time.
        // Do not compete with that writer through the launcher's stale UI.
        if self.busy() {
            self.error(
                "Finish installation and close Minecraft before changing game settings here.",
            );
            return;
        }
        match config::update_client_setting(&self.layout.client_settings(), key, value) {
            Ok(settings) => self.settings = settings,
            Err(error) => self.error(format!("Could not save Minecraft settings: {error:#}")),
        }
    }
    fn select_theme(&mut self, ctx: &egui::Context, id: &str) {
        let palette = theme::resolve(id, &self.config.appearance.custom_themes);
        let mut candidate = self.config.clone();
        candidate.appearance.theme_id = palette.id.clone();
        if self.commit_config(candidate) {
            self.palette = palette;
            theme::apply(ctx, &self.palette, self.config.appearance.reduced_motion);
            ctx.request_repaint();
        }
    }
    fn toggle_favorite(&mut self, id: &str) {
        let mut candidate = self.config.clone();
        let favorites = &mut candidate.appearance.favorites;
        if let Some(index) = favorites.iter().position(|s| s == id) {
            favorites.remove(index);
        } else {
            favorites.push(id.to_owned());
        }
        self.commit_config(candidate);
    }
    fn start_install(&mut self, ctx: &egui::Context, play_after: bool, force: bool) {
        if self.busy() {
            return;
        }
        self.status = None;
        self.rep.reset_cancel();
        self.rep.stage(0, 5, "Checking files");
        let (tx, rx) = mpsc::channel();
        let (root, rep) = (self.layout.root.clone(), self.rep.clone());
        let options = bootstrap::Options {
            manifest_url: self.manifest.clone(),
        };
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = bootstrap::run(&Layout::new(root), &options, force, &rep).map_err(|e| {
                if e.downcast_ref::<crate::net::UserCancelled>().is_some() {
                    "Cancelled".to_owned()
                } else {
                    format!("{e:#}")
                }
            });
            if let Err(message) = &result {
                rep.log(format!("ERROR: {message}"));
            }
            let _ = tx.send(result);
            ctx.request_repaint();
        });
        self.phase = Phase::Installing {
            play_after,
            done: rx,
        };
    }
    fn launch_spec(&self) -> launch::Spec {
        launch::Spec {
            root: self.layout.root.clone(),
            server: self.address(),
            username: self.username(),
            fullscreen: config::fullscreen(&config::load_client_settings(
                &self.layout.client_settings(),
            )),
        }
    }
    fn start_client(&mut self) {
        let spec = self.launch_spec();
        match launch::spawn(&spec, &self.rep) {
            Ok(child) => {
                self.status = None;
                self.phase = Phase::Running(child);
            }
            Err(error) => {
                self.phase = Phase::Idle;
                self.error(format!("Cannot launch Minecraft: {error:#}"));
            }
        }
    }
    fn start_play(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        if !self.valid_selection() {
            self.error("Choose a valid offline name and server address before playing.");
            return;
        }
        if !self.save_config() {
            return;
        }
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
            Phase::Installing { play_after, done } => match done.try_recv() {
                Ok(result) => finished = Some((*play_after, result)),
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished = Some((
                        false,
                        Err("Installation worker stopped unexpectedly. See Logs and retry.".into()),
                    ))
                }
                Err(mpsc::TryRecvError::Empty) => {}
            },
            Phase::Running(child) => match child.try_wait() {
                Ok(Some(status)) => {
                    self.rep.log(format!("Client exited: {status}"));
                    self.phase = Phase::Idle;
                    if !status.success() {
                        self.error(format!(
                            "Minecraft exited with {status}. Open Logs for details."
                        ));
                    }
                }
                Err(error) => {
                    self.phase = Phase::Idle;
                    self.error(format!("Cannot observe Minecraft process: {error}"));
                }
                Ok(None) => {}
            },
            Phase::Idle => {}
        }
        if let Some((play_after, result)) = finished {
            self.phase = Phase::Idle;
            self.ready = bootstrap::is_ready(&self.layout);
            // A completion can race the user's Cancel. Never autoplay a
            // successfully completed worker after cancellation was requested.
            if self.rep.check_cancel().is_err() {
                self.status = Some((
                    "Installation cancelled. Your completed downloads are kept.".into(),
                    false,
                ));
                return;
            }
            match result {
                Ok(()) if play_after && self.ready => self.start_client(),
                Ok(()) if self.ready => self.status = Some(("Game files are ready.".into(), false)),
                Ok(()) => self.error("Installation finished but required files are missing. Open Logs and verify game files."),
                Err(message) if message == "Cancelled" => self.status = Some(("Installation cancelled. Your completed downloads are kept.".into(), false)),
                Err(message) => self.error(message),
            }
        }
    }
    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header")
            .exact_height(58.0)
            .frame(
                egui::Frame::new()
                    .fill(self.palette.background)
                    .inner_margin(Margin::symmetric(24, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    if self.logo.is_none()
                        && let Some(icon) = load_icon()
                    {
                        self.logo = Some(ctx.load_texture(
                            "solaris-brand",
                            egui::ColorImage::from_rgba_unmultiplied(
                                [icon.width as usize, icon.height as usize],
                                &icon.rgba,
                            ),
                            egui::TextureOptions::NEAREST,
                        ));
                    }
                    if let Some(logo) = &self.logo {
                        ui.add(
                            egui::Image::new(logo)
                                .fit_to_exact_size(Vec2::splat(32.0))
                                .corner_radius(6),
                        );
                    }
                    ui.label(RichText::new("Solaris").size(20.0).strong());
                    ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                        for (tab, name) in [
                            (Tab::Logs, "Logs"),
                            (Tab::Settings, "Settings"),
                            (Tab::Play, "Home"),
                        ] {
                            if motion::tab(ui, name, self.tab == tab).clicked() {
                                self.tab = tab;
                                if tab == Tab::Settings {
                                    self.settings = config::load_client_settings(
                                        &self.layout.client_settings(),
                                    );
                                }
                            }
                        }
                    });
                });
            });
    }
    fn bottom_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("action-area").exact_height(94.0)
            .frame(egui::Frame::new().fill(self.palette.surface).inner_margin(Margin::symmetric(24, 16)))
            .show(ctx, |ui| {
                ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                    let valid = self.valid_selection();
                    let (label, enabled) = match &self.phase {
                        Phase::Idle if self.ready => ("Play", valid),
                        Phase::Idle => ("Install & Play", valid),
                        Phase::Installing { .. } => ("Installing…", false),
                        Phase::Running(_) => ("Running", false),
                    };
                    let button = egui::Button::new(RichText::new(label).strong().size(18.0).color(if enabled { self.palette.on_accent } else { self.palette.disabled }))
                        .fill(if enabled { self.palette.accent } else { self.palette.elevated })
                        .stroke(Stroke::NONE).min_size(Vec2::new(174.0, 52.0));
                    let fill = if enabled { self.palette.accent } else { self.palette.elevated };
                    // Extrapolate away from the black/white label token so
                    // hover never reduces its contrast (notably in Matcha).
                    let hover = motion::mix(fill, self.palette.on_accent, -0.08);
                    if ui.add_enabled_ui(enabled, |ui| motion::button(ui, button, fill, hover)).inner.clicked() { self.start_play(ctx); }
                    if matches!(self.phase, Phase::Installing { .. }) && ui.button("Cancel").clicked() { self.rep.cancel(); }
                    if let Phase::Running(child) = &mut self.phase
                        && ui.button("Stop").clicked()
                        && let Err(error) = child.kill()
                    {
                        self.error(format!("Cannot stop Minecraft: {error}"));
                    }
                    let width = ui.available_width();
                    ui.allocate_ui_with_layout(Vec2::new(width, 60.0), UiLayout::top_down(Align::LEFT), |ui| {
                        match &self.phase {
                            Phase::Installing { .. } => {
                                let s = self.rep.snapshot();
                                let title = if s.stage_index == 0 { "Checking files".into() } else { format!("Step {}/{} · {}", s.stage_index, s.stage_count, s.title) };
                                ui.add(egui::Label::new(RichText::new(title).strong()).truncate());
                                if s.total > 0 {
                                    ui.add(egui::ProgressBar::new((s.done as f32 / s.total as f32).clamp(0.0, 1.0)).fill(self.palette.accent).desired_width(width).desired_height(5.0));
                                    ui.weak(format!("{} / {} · {}/s", human_bytes(s.done), human_bytes(s.total), human_bytes(s.bytes_per_second as u64)));
                                } else {
                                    ui.horizontal(|ui| {
                                        if !self.config.appearance.reduced_motion { ui.add(egui::Spinner::new().size(14.0).color(self.palette.accent)); }
                                        ui.weak("Working · total not yet known");
                                    });
                                }
                            }
                            Phase::Running(_) => { ui.label(RichText::new("Minecraft process running").strong()); ui.weak("Startup and connection details are in Logs."); }
                            Phase::Idle => {
                                let (title, detail, color) = if !valid {
                                    ("Check your selection", "Use a 3–16 character Latin offline name and a valid server address.".to_owned(), self.palette.error)
                                } else if let Some((message, error)) = &self.status {
                                    (if *error { "Something went wrong · see Logs" } else { "Status" }, message.clone(), if *error { self.palette.error } else { self.palette.muted })
                                } else if self.ready { ("Ready to play", "Your game files are installed.".into(), self.palette.muted) }
                                else { ("Game files needed", "First run downloads Minecraft content from its original sources.".into(), self.palette.muted) };
                                ui.label(RichText::new(title).strong().color(color));
                                ui.add(egui::Label::new(RichText::new(&detail).color(self.palette.muted)).truncate()).on_hover_text(detail);
                            }
                        }
                    });
                });
            });
    }
    fn skin_source(&self) -> (Option<String>, &'static str) {
        if let Some(path) = self.config.profiles[self.config.selected_profile]
            .skin_path
            .as_ref()
            .filter(|s| !s.trim().is_empty())
        {
            return (Some(path.clone()), "Local skin preview");
        }
        // Read the player's installed original resources; never bundle/copy these textures.
        for file in [
            "resources/assets/minecraft/textures/entity/player/wide/steve.png",
            "resources/assets/minecraft/textures/entity/steve.png",
        ] {
            let path = self.layout.join(file);
            if path.is_file() {
                return (Some(path.display().to_string()), "Default Steve preview");
            }
        }
        (None, media::FALLBACK_HEAD_LABEL)
    }
    fn play_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.weak("MULTIPLAYER");
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if motion::secondary(ui, "Servers", !self.busy()).clicked() {
                    self.open_manager(Dialog::Servers);
                }
            });
        });
        let server_name = self.config.servers[self.config.selected_server]
            .name
            .clone();
        ui.add(egui::Label::new(RichText::new(server_name).size(30.0).strong()).truncate());
        ui.add(
            egui::Label::new(RichText::new(self.address()).color(self.palette.muted)).truncate(),
        );
        ui.add_space(12.0);
        // In compact windows artwork yields to the profile and game identity.
        if self.config.appearance.background_enabled && ui.available_height() >= 210.0 {
            let path = self.config.appearance.background_path.trim();
            match self
                .media
                .background(ui.ctx(), if path.is_empty() { None } else { Some(path) })
            {
                Ok(texture) => {
                    let height = if ui.available_height() < 230.0 {
                        64.0
                    } else {
                        92.0
                    };
                    let size = Vec2::new(ui.available_width(), height);
                    let uv = media::cover_uv(texture.size_vec2(), size);
                    ui.add(
                        egui::Image::new(&texture)
                            .fit_to_exact_size(size)
                            .maintain_aspect_ratio(false)
                            .uv(uv)
                            .corner_radius(8),
                    )
                    .on_hover_text(if path.is_empty() {
                        media::DEFAULT_BACKGROUND_LABEL
                    } else {
                        "Your local PNG image"
                    });
                    ui.add_space(10.0);
                }
                Err(error) => {
                    note(
                        ui,
                        format!("Background unavailable: {error}"),
                        self.palette.error,
                    );
                }
            }
        }
        egui::Frame::new()
            .fill(self.palette.surface)
            .corner_radius(10)
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (source, label) = self.skin_source();
                    let image = self.media.head(ui.ctx(), source.as_deref());
                    let (image, notice) = match image {
                        Ok(image) => (Some(image), label.to_owned()),
                        Err(error) => (
                            self.media.head(ui.ctx(), None).ok(),
                            format!("Skin unavailable: {error}. {}", media::FALLBACK_HEAD_LABEL),
                        ),
                    };
                    if let Some(texture) = image {
                        ui.add(
                            egui::Image::new(&texture)
                                .fit_to_exact_size(Vec2::splat(42.0))
                                .corner_radius(6),
                        )
                        .on_hover_text(notice);
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(self.username()).strong().size(16.0));
                        ui.weak("Offline profile");
                    });
                    ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                        if motion::secondary(ui, "Profiles", !self.busy()).clicked() {
                            self.open_manager(Dialog::Profiles);
                        }
                    });
                });
            });
        ui.add_space(10.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(format!("Minecraft {GAME_VERSION}"));
            ui.weak("·  Native client preview");
        });
        ui.add_space(6.0);
        ui.weak("Offline identity only. Connect to servers that allow offline profiles.");
        if self.args_server.is_some() || self.args_username.is_some() {
            ui.weak("Command-line server / username override is active for this session.");
        }
    }
    fn settings_navigation(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            for (tab, name) in [
                (SettingsTab::General, "General"),
                (SettingsTab::Appearance, "Appearance"),
                (SettingsTab::Minecraft, "Minecraft"),
                (SettingsTab::Advanced, "Advanced"),
            ] {
                if motion::tab(ui, name, self.settings_tab == tab).clicked() {
                    self.settings_tab = tab;
                }
            }
        });
        ui.add_space(12.0);
        if self.settings_tab == SettingsTab::Appearance {
            ui.horizontal(|ui| {
                for (tab, label) in [
                    (AppearanceTab::Themes, "Themes"),
                    (AppearanceTab::Background, "Background & motion"),
                ] {
                    if motion::tab(ui, label, self.appearance_tab == tab).clicked() {
                        self.appearance_tab = tab;
                    }
                }
            });
            ui.add_space(8.0);
        }
    }
    fn settings_tab(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.settings_tab {
            SettingsTab::Appearance => {
                if self.appearance_tab == AppearanceTab::Themes {
                    self.theme_gallery(ui, ctx);
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| self.background_settings(ui));
                }
            }
            _ => {
                egui::ScrollArea::vertical().show(ui, |ui| match self.settings_tab {
                SettingsTab::General => {
                    section(ui, "Make yourself at home", "Your launcher settings stay on this device.");
                    ui.label(format!("Solaris Launcher {}", env!("CARGO_PKG_VERSION")));
                    ui.weak("Independent native Minecraft client · offline/test-identity preview");
                    ui.add_space(14.0);
                    ui.label("Data folder");
                    ui.add(egui::Label::new(RichText::new(self.layout.root.display().to_string()).monospace()).wrap());
                    if ui.button("Open data folder").clicked() {
                        let result = std::fs::create_dir_all(&self.layout.root).and_then(|_| std::process::Command::new(if cfg!(windows) { "explorer" } else { "xdg-open" }).arg(&self.layout.root).spawn().map(|_| ()));
                        if let Err(error) = result { self.error(format!("Cannot open data folder: {error}")); }
                    }
                    ui.add_space(12.0);
                    ui.weak("Window size is remembered when you close the launcher. Native window buttons handle minimize, maximize and close.");
                }
                SettingsTab::Minecraft => {
                    section(ui, "Minecraft", "Shared with the in-game Options screen.");
                    if self.busy() { ui.weak("Game settings are read-only here while work is active. Use the in-game Options screen while playing."); }
                    ui.add_enabled_ui(!self.busy(), |ui| {
                    let mut distance = config::render_distance(&self.settings);
                    ui.label("Render distance");
                    ui.spacing_mut().slider_width = (ui.available_width() - 130.0).clamp(160.0, 300.0);
                    if ui.add(egui::Slider::new(&mut distance, 2..=32).suffix(" chunks")).changed() { self.save_setting("render_distance", serde_json::json!(distance)); }
                    let mut fullscreen = config::fullscreen(&self.settings);
                    if ui.checkbox(&mut fullscreen, "Start Minecraft in fullscreen").changed() { self.save_setting("fullscreen", serde_json::json!(fullscreen)); }
                    });
                    ui.add_space(12.0);
                    ui.weak("FOV, volume, GUI scale and VSync are available in Minecraft → Esc → Options.");
                }
                SettingsTab::Advanced => {
                    section(ui, "Game content", "Downloads are verified against their original hashes.");
                    if ui.add_enabled(!self.busy(), egui::Button::new("Verify / repair game files")).clicked() { self.start_install(ctx, false, true); self.tab = Tab::Play; }
                    ui.add_space(14.0);
                    if text_field(ui, "Solaris content manifest", &mut self.manifest, 2048).changed() {
                        self.config.manifest_url = self.manifest.trim().to_owned(); self.save_config();
                    }
                    ui.weak("Advanced: change only if you know the source. Bundle trust is still pinned by this launcher.");
                    ui.add_space(14.0);
                    if ui.button("Open logs").clicked() { self.tab = Tab::Logs; }
                }
                SettingsTab::Appearance => {}
            });
            }
        }
        if let Some((message, true)) = &self.status {
            note(ui, message.clone(), self.palette.error);
        }
    }
    fn background_settings(&mut self, ui: &mut egui::Ui) {
        section(
            ui,
            "A little atmosphere",
            "Optional imagery. All text stays on solid, readable surfaces.",
        );
        if ui
            .checkbox(
                &mut self.config.appearance.background_enabled,
                "Show landscape banner on Home",
            )
            .changed()
        {
            self.save_config();
        }
        ui.add_space(8.0);
        if text_field(
            ui,
            "Local PNG path · leave empty for the Solaris illustration",
            &mut self.config.appearance.background_path,
            4096,
        )
        .changed()
        {
            self.save_config();
        }
        ui.weak("PNG only · up to 16 MiB / 4096×4096. Images stay on your computer.");
        if ui.button("Use default illustration").clicked() {
            self.config.appearance.background_path.clear();
            self.save_config();
        }
        let path = self.config.appearance.background_path.trim();
        match self
            .media
            .background(ui.ctx(), if path.is_empty() { None } else { Some(path) })
        {
            Ok(texture) => {
                let size = Vec2::new(ui.available_width(), 100.0);
                ui.add(
                    egui::Image::new(&texture)
                        .uv(media::cover_uv(texture.size_vec2(), size))
                        .fit_to_exact_size(size)
                        .maintain_aspect_ratio(false)
                        .corner_radius(8),
                );
                ui.weak(if path.is_empty() {
                    media::DEFAULT_BACKGROUND_LABEL
                } else {
                    "Local PNG preview"
                });
            }
            Err(error) => {
                note(ui, error, self.palette.error);
            }
        }
        ui.add_space(12.0);
        if ui
            .checkbox(&mut self.config.appearance.reduced_motion, "Reduced motion")
            .changed()
        {
            self.save_config();
        }
        ui.weak("Disables animated interactions, scrolling and indeterminate spinners. Progress and status still update.");
    }
    fn filtered_themes(&self, favorites_only: bool) -> Vec<Theme> {
        let query = self.search.trim().to_lowercase();
        self.themes
            .iter()
            .filter(|t| {
                let favorite = self.config.appearance.favorites.contains(&t.id);
                let category = match self.filter {
                    Filter::All => true,
                    Filter::Dark => t.category == ThemeCategory::Dark,
                    Filter::Light => t.category == ThemeCategory::Light,
                    Filter::Colorful => t.category == ThemeCategory::Colorful,
                    Filter::Favorites => favorite,
                };
                category && (!favorites_only || favorite) && t.name.to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }
    fn theme_gallery(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Solaris Themes").size(21.0).strong());
            ui.with_layout(UiLayout::right_to_left(Align::Center), |ui| {
                if ui.button("Create theme").clicked() {
                    self.open_custom(None);
                }
                ui.weak(format!("{} palettes", self.themes.len()));
            });
        });
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .hint_text("Search themes…")
                    .desired_width(180.0)
                    .char_limit(64),
            );
            for (filter, label) in [
                (Filter::All, "All"),
                (Filter::Dark, "Dark"),
                (Filter::Light, "Light"),
                (Filter::Colorful, "Colorful"),
                (Filter::Favorites, "Favorites"),
            ] {
                if motion::tab(ui, label, self.filter == filter).clicked() {
                    self.filter = filter;
                }
            }
        });
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .id_salt("theme-gallery")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.filter == Filter::All
                    && self.search.is_empty()
                    && !self.config.appearance.favorites.is_empty()
                {
                    ui.label(RichText::new("Favorites").strong());
                    self.theme_grid(ui, ctx, self.filtered_themes(true), "favorites");
                    ui.add_space(8.0);
                    ui.label(RichText::new("All themes").strong());
                }
                let themes = self.filtered_themes(false);
                if themes.is_empty() {
                    ui.weak(if self.filter == Filter::Favorites {
                        "No favorites yet. Star a theme to keep it here."
                    } else {
                        "No themes match your search."
                    });
                }
                self.theme_grid(ui, ctx, themes, "all");
                if !self.config.appearance.custom_themes.is_empty() {
                    ui.add_space(8.0);
                    ui.label(RichText::new("Your themes").strong());
                    let customs = self.config.appearance.custom_themes.clone();
                    for custom in customs {
                        ui.horizontal(|ui| {
                            // Reserve action space before allocating the bounded name.
                            if ui.small_button("Edit / export").clicked() {
                                self.open_custom(Some(&custom));
                            }
                            if ui.small_button("Delete").clicked() {
                                let mut candidate = self.config.clone();
                                candidate
                                    .appearance
                                    .custom_themes
                                    .retain(|t| t.id != custom.id);
                                candidate.appearance.favorites.retain(|s| s != &custom.id);
                                if candidate.appearance.theme_id == custom.id {
                                    candidate.appearance.theme_id = "graphite".into();
                                }
                                if self.commit_config(candidate) {
                                    self.rebuild_themes();
                                    self.palette = theme::resolve(
                                        &self.config.appearance.theme_id,
                                        &self.config.appearance.custom_themes,
                                    );
                                    theme::apply(
                                        ctx,
                                        &self.palette,
                                        self.config.appearance.reduced_motion,
                                    );
                                }
                            }
                            ui.add(egui::Label::new(&custom.name).truncate())
                                .on_hover_text(&custom.name);
                        });
                    }
                }
            });
    }
    fn theme_grid(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        themes: Vec<Theme>,
        salt: &str,
    ) {
        let cols = ((ui.available_width() + 10.0) / 170.0).floor().max(2.0) as usize;
        let width = (ui.available_width() - (cols - 1) as f32 * 10.0) / cols as f32;
        egui::Grid::new(("theme-grid", salt))
            .num_columns(cols)
            .spacing(Vec2::new(10.0, 10.0))
            .show(ui, |ui| {
                for (index, t) in themes.iter().enumerate() {
                    ui.push_id(&t.id, |ui| {
                        let active = self.palette.id == t.id;
                        let favorite = self.config.appearance.favorites.contains(&t.id);
                        let (select, star) =
                            theme_card(ui, t, width, active, favorite, &self.palette);
                        if select {
                            self.select_theme(ctx, &t.id);
                        }
                        if star {
                            self.toggle_favorite(&t.id);
                        }
                    });
                    if index % cols == cols - 1 {
                        ui.end_row();
                    }
                }
            });
    }
    fn rebuild_themes(&mut self) {
        self.themes = theme_presets::all();
        self.themes.extend(
            self.config
                .appearance
                .custom_themes
                .iter()
                .filter_map(|t| t.validate().ok()),
        );
    }
    fn open_manager(&mut self, dialog: Dialog) {
        self.dialog = Some(dialog);
        self.dialog_error = None;
        let index = if dialog == Dialog::Profiles {
            self.config.selected_profile
        } else {
            self.config.selected_server
        };
        self.load_draft(dialog, index);
    }
    fn load_draft(&mut self, dialog: Dialog, index: usize) {
        self.editing = Some(index);
        if dialog == Dialog::Profiles {
            self.draft_name = self.config.profiles[index].username.clone();
            self.draft_skin = self.config.profiles[index]
                .skin_path
                .clone()
                .unwrap_or_default();
        } else {
            self.draft_name = self.config.servers[index].name.clone();
            self.draft_address = self.config.servers[index].address.clone();
        }
    }
    fn save_draft(&mut self, kind: Dialog, ctx: &egui::Context) -> bool {
        if self.busy() {
            return false;
        }
        let mut candidate = self.config.clone();
        if kind == Dialog::Profiles {
            let name = self.draft_name.trim();
            if !config::valid_username(name) {
                self.dialog_error = Some("Use 3–16 Latin letters, digits or underscore.".into());
                return false;
            }
            let skin = self.draft_skin.trim();
            if !skin.is_empty()
                && let Err(error) = self.media.head(ctx, Some(skin))
            {
                self.dialog_error = Some(error);
                return false;
            }
            let profile = Profile {
                username: name.to_owned(),
                skin_path: (!skin.is_empty()).then(|| skin.to_owned()),
            };
            if let Some(index) = self.editing {
                candidate.profiles[index] = profile;
                candidate.selected_profile = index;
            } else {
                candidate.profiles.push(profile);
                candidate.selected_profile = candidate.profiles.len() - 1;
            }
        } else {
            if self.draft_name.trim().is_empty() || !config::valid_address(&self.draft_address) {
                self.dialog_error =
                    Some("Enter a server name and host, host:port or [IPv6]:port.".into());
                return false;
            }
            let server = Server {
                name: self.draft_name.trim().into(),
                address: self.draft_address.trim().into(),
            };
            if let Some(index) = self.editing {
                candidate.servers[index] = server;
                candidate.selected_server = index;
            } else {
                candidate.servers.push(server);
                candidate.selected_server = candidate.servers.len() - 1;
            }
        }
        self.commit_config(candidate)
    }
    fn manager_dialog(&mut self, ctx: &egui::Context, kind: Dialog) {
        let width = (ctx.content_rect().width() - 80.0).clamp(300.0, 440.0);
        let response = egui::Modal::new(egui::Id::new("manage-selection"))
            .backdrop_color(self.palette.background.gamma_multiply(0.8)).show(ctx, |ui| {
                ui.multiply_opacity(self.dialog_opacity);
                ui.set_width(width);
                ui.label(RichText::new(if kind == Dialog::Profiles { "Offline profiles" } else { "Servers" }).size(21.0).strong());
                let rows: Vec<(String, String)> = if kind == Dialog::Profiles {
                    self.config.profiles.iter().map(|p| (p.username.clone(), "Offline".into())).collect()
                } else { self.config.servers.iter().map(|s| (s.name.clone(), s.address.clone())).collect() };
                egui::ScrollArea::vertical().id_salt("manager-list").max_height(100.0).show(ui, |ui| {
                    for (index, (title, subtitle)) in rows.iter().enumerate() {
                        let selected = self.editing == Some(index);
                        if ui.add(egui::Button::selectable(selected, format!("{title}   ·   {subtitle}")).min_size(Vec2::new(width, 30.0))).clicked() {
                            let mut candidate = self.config.clone();
                            if kind == Dialog::Profiles { candidate.selected_profile = index; }
                            else { candidate.selected_server = index; }
                            if self.commit_config(candidate) { self.load_draft(kind, index); }
                        }
                    }
                });
                ui.horizontal(|ui| {
                    if ui.button(if kind == Dialog::Profiles { "+ New profile" } else { "+ Add server" }).clicked() {
                        self.editing = None; self.draft_name.clear(); self.draft_address.clear(); self.draft_skin.clear(); self.dialog_error = None;
                    }
                    if ui.add_enabled(rows.len() > 1 && self.editing.is_some(), egui::Button::new("Remove")).clicked() {
                        let index = self.editing.unwrap();
                        let mut candidate = self.config.clone();
                        if kind == Dialog::Profiles { candidate.profiles.remove(index); }
                        else { candidate.servers.remove(index); }
                        candidate.normalize();
                        let selected = if kind == Dialog::Profiles { candidate.selected_profile } else { candidate.selected_server };
                        if self.commit_config(candidate) { self.load_draft(kind, selected); }
                    }
                });
                ui.separator();
                // Editing the draft retires the previous save error.
                let mut edited = text_field(ui, if kind == Dialog::Profiles { "Username" } else { "Server name" }, &mut self.draft_name, if kind == Dialog::Profiles { 16 } else { 80 }).changed();
                if kind == Dialog::Servers { edited |= text_field(ui, "Address · host[:port]", &mut self.draft_address, 512).changed(); }
                else {
                    edited |= text_field(ui, "Local skin PNG · optional path", &mut self.draft_skin, 4096).changed();
                    ui.weak(media::LOCAL_SKIN_NOTICE);
                    ui.weak("No skin selected: installed Steve texture, or the original Solaris avatar.");
                }
                if edited { self.dialog_error = None; }
                if let Some(error) = &self.dialog_error { note(ui, error.clone(), self.palette.error); }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Save & use").clicked() && self.save_draft(kind, ctx) { self.dialog = None; }
                    if ui.button("Done").clicked() { self.dialog = None; }
                });
            });
        if response.should_close() {
            self.dialog = None;
        }
    }
    fn open_custom(&mut self, existing: Option<&CustomTheme>) {
        self.dialog = Some(Dialog::CustomTheme);
        self.dialog_error = None;
        self.custom_original = existing.map(|t| t.id.clone());
        self.custom_draft = existing.cloned().unwrap_or_else(|| {
            let mut number = 1;
            while self
                .themes
                .iter()
                .any(|t| t.id == format!("custom-{number}"))
            {
                number += 1;
            }
            CustomTheme {
                id: format!("custom-{number}"),
                name: "My theme".into(),
                background: hex(self.palette.background),
                surface: hex(self.palette.surface),
                text: hex(self.palette.text),
                accent: hex(self.palette.accent),
            }
        });
        self.custom_json = theme::export_json(&self.custom_draft).unwrap_or_default();
    }
    fn save_custom(&mut self, ctx: &egui::Context) {
        if let Err(error) = self.custom_draft.validate() {
            self.dialog_error = Some(error);
            return;
        }
        let replacing = self
            .config
            .appearance
            .custom_themes
            .iter()
            .position(|t| Some(&t.id) == self.custom_original.as_ref());
        if self
            .config
            .appearance
            .custom_themes
            .iter()
            .enumerate()
            .any(|(i, t)| t.id == self.custom_draft.id && Some(i) != replacing)
        {
            self.dialog_error = Some(
                "A different custom theme already uses this ID. Change the ID before saving."
                    .into(),
            );
            return;
        }
        let mut candidate = self.config.clone();
        if let Some(index) = replacing {
            let old_id = &candidate.appearance.custom_themes[index].id;
            for favorite in &mut candidate.appearance.favorites {
                if favorite == old_id {
                    favorite.clone_from(&self.custom_draft.id);
                }
            }
            candidate.appearance.custom_themes[index] = self.custom_draft.clone();
        } else if candidate.appearance.custom_themes.len() < 64 {
            candidate
                .appearance
                .custom_themes
                .push(self.custom_draft.clone());
        } else {
            self.dialog_error =
                Some("Limit: 64 custom themes. Delete one before adding another.".into());
            return;
        }
        candidate.appearance.theme_id = self.custom_draft.id.clone();
        if self.commit_config(candidate) {
            self.rebuild_themes();
            self.palette = theme::resolve(
                &self.config.appearance.theme_id,
                &self.config.appearance.custom_themes,
            );
            theme::apply(ctx, &self.palette, self.config.appearance.reduced_motion);
            self.dialog = None;
        }
    }
    fn custom_dialog(&mut self, ctx: &egui::Context) {
        let response = egui::Modal::new(egui::Id::new("custom-theme"))
            .backdrop_color(self.palette.background.gamma_multiply(0.8)).show(ctx, |ui| {
                ui.multiply_opacity(self.dialog_opacity);
                ui.set_width((ctx.content_rect().width() - 80.0).clamp(300.0, 470.0));
                ui.label(RichText::new("Your palette").size(21.0).strong());
                egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 140.0).max(230.0)).show(ui, |ui| {
                    text_field(ui, "Name", &mut self.custom_draft.name, 64);
                    text_field(ui, "Stable ID · lowercase-kebab-case", &mut self.custom_draft.id, 64);
                    for (label, value) in [("Background", &mut self.custom_draft.background), ("Surface", &mut self.custom_draft.surface), ("Text", &mut self.custom_draft.text), ("Accent", &mut self.custom_draft.accent)] {
                        ui.horizontal(|ui| {
                            ui.label(label); ui.add(egui::TextEdit::singleline(value).desired_width(110.0).char_limit(7));
                        });
                    }
                    match self.custom_draft.validate() {
                        Ok(t) => { theme_card(ui, &t, ui.available_width(), false, false, &self.palette); ui.weak("Contrast checked · other semantic colors are derived automatically."); }
                        Err(error) => { note(ui, error, self.palette.error); }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Reset to active palette").clicked() {
                            self.custom_draft.background = hex(self.palette.background); self.custom_draft.surface = hex(self.palette.surface);
                            self.custom_draft.text = hex(self.palette.text); self.custom_draft.accent = hex(self.palette.accent);
                        }
                        if ui.button("Copy JSON").clicked() {
                            match theme::export_json(&self.custom_draft) { Ok(json) => { ctx.copy_text(json.clone()); self.custom_json = json; }, Err(error) => self.dialog_error = Some(error) }
                        }
                    });
                    egui::CollapsingHeader::new("Import / export JSON").show(ui, |ui| {
                        ui.add(egui::TextEdit::multiline(&mut self.custom_json).desired_rows(4).desired_width(f32::INFINITY).char_limit(4097).font(egui::TextStyle::Monospace));
                        if ui.button("Import pasted JSON").clicked() {
                            match theme::import_json(&self.custom_json) { Ok(theme) => { self.custom_draft = theme; self.dialog_error = None; }, Err(error) => self.dialog_error = Some(error) }
                        }
                        text_field(ui, "Local JSON file path", &mut self.transfer_path, 4096);
                        ui.horizontal(|ui| {
                            if ui.button("Read file").clicked() {
                                let result = read_theme_file(&self.transfer_path);
                                match result { Ok(theme) => { self.custom_draft = theme; self.dialog_error = None; }, Err(error) => self.dialog_error = Some(error) }
                            }
                            if ui.button("Export file").clicked() {
                                let result = theme::export_json(&self.custom_draft).and_then(|text| export_theme_file(&self.transfer_path, &text));
                                match result { Ok(()) => { self.dialog_error = None; self.status = Some(("Theme exported. Existing files are never overwritten.".into(), false)); }, Err(error) => self.dialog_error = Some(error) }
                            }
                        });
                        ui.weak("Imports are validated. Export creates a new file and never overwrites existing files.");
                    });
                    if let Some(error) = &self.dialog_error { note(ui, error.clone(), self.palette.error); }
                });
                ui.horizontal(|ui| {
                    if ui.button("Save & apply").clicked() { self.save_custom(ctx); }
                    if ui.button("Cancel").clicked() { self.dialog = None; }
                });
            });
        if response.should_close() {
            self.dialog = None;
        }
    }
    fn logs_tab(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Launcher log").size(21.0).strong());
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(self.rep.logs().join("\n"));
            }
            if ui.button("Clear").clicked() {
                self.rep.clear_logs();
            }
        });
        ui.add(
            egui::Label::new(
                RichText::new(self.layout.log_file().display().to_string())
                    .small()
                    .color(self.palette.muted),
            )
            .truncate(),
        );
        egui::Frame::new().fill(self.palette.input).corner_radius(8).inner_margin(Margin::same(12)).show(ui, |ui| {
            egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                let lines = self.rep.logs();
                if lines.is_empty() { ui.weak("Nothing logged yet. Installation and Minecraft output appear here."); }
                for line in lines {
                    let color = if line.starts_with("ERROR") { self.palette.error } else { self.palette.text };
                    ui.add(egui::Label::new(RichText::new(line).monospace().color(color)).wrap().selectable(true));
                }
            });
        });
    }
    fn draw(&mut self, ctx: &egui::Context) {
        theme::apply(ctx, &self.palette, self.config.appearance.reduced_motion);
        self.header(ctx);
        if self.tab == Tab::Play {
            self.bottom_bar(ctx);
        }
        egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(self.palette.background)
                    .inner_margin(Margin::symmetric(24, 16)),
            )
            .show(ctx, |ui| {
                // Resolve nested navigation before choosing the reveal identity.
                if self.tab == Tab::Settings {
                    self.settings_navigation(ui);
                }
                let page_key = egui::Id::new((
                    self.tab as u8,
                    self.settings_tab as u8,
                    self.appearance_tab as u8,
                ));
                let page_opacity = 0.72 + 0.28 * self.page_reveal.sample(ctx, page_key);
                ui.multiply_opacity(page_opacity);
                match self.tab {
                    Tab::Play => {
                        egui::ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .show(ui, |ui| self.play_tab(ui));
                    }
                    Tab::Settings => self.settings_tab(ui, ctx),
                    Tab::Logs => self.logs_tab(ui),
                }
            });
        self.dialog_opacity = if let Some(dialog) = self.dialog {
            0.35 + 0.65 * self.dialog_reveal.sample(ctx, egui::Id::new(dialog as u8))
        } else {
            self.dialog_reveal.reset();
            1.0
        };
        match self.dialog {
            Some(Dialog::CustomTheme) => self.custom_dialog(ctx),
            Some(kind) => self.manager_dialog(ctx, kind),
            None => {}
        }
    }
}

fn hex(c: Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b())
}

fn read_theme_file(path: &str) -> Result<CustomTheme, String> {
    use std::io::Read;
    let file = std::fs::File::open(path.trim()).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    file.take((theme::MAX_CUSTOM_JSON_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > theme::MAX_CUSTOM_JSON_BYTES {
        return Err("Theme file exceeds 4 KiB".into());
    }
    theme::import_json(std::str::from_utf8(&bytes).map_err(|e| e.to_string())?)
}
fn export_theme_file(path: &str, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.trim())
        .map_err(|e| format!("Cannot create theme file: {e}"))?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

/// Real palette miniature: window header, content hierarchy and primary action.
/// The favorite is a separate keyboard-focusable control, not a nested click target.
fn theme_card(
    ui: &mut egui::Ui,
    t: &Theme,
    width: f32,
    active: bool,
    favorite: bool,
    current: &Theme,
) -> (bool, bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 112.0), Sense::hover());
    let preview = Rect::from_min_max(rect.min, rect.max - Vec2::new(0.0, 32.0));
    let response = ui.interact(preview, ui.id().with("select"), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            active,
            &t.name,
        )
    });
    let p = ui.painter_at(rect);
    p.rect_filled(preview, 8, t.background);
    p.rect_filled(
        Rect::from_min_size(
            preview.min + Vec2::splat(7.0),
            Vec2::new(width - 14.0, 12.0),
        ),
        3,
        t.surface,
    );
    p.rect_filled(
        Rect::from_min_size(preview.min + Vec2::new(14.0, 11.0), Vec2::new(18.0, 4.0)),
        1,
        t.accent,
    );
    p.rect_filled(
        Rect::from_min_size(
            preview.min + Vec2::new(12.0, 28.0),
            Vec2::new(width * 0.45, 5.0),
        ),
        1,
        t.text,
    );
    p.rect_filled(
        Rect::from_min_size(
            preview.min + Vec2::new(12.0, 39.0),
            Vec2::new(width * 0.34, 3.0),
        ),
        1,
        t.muted,
    );
    p.rect_filled(
        Rect::from_min_size(
            preview.min + Vec2::new(12.0, 53.0),
            Vec2::new(width * 0.32, 16.0),
        ),
        4,
        t.elevated,
    );
    p.rect_filled(
        Rect::from_min_size(
            preview.right_bottom() - Vec2::new(54.0, 24.0),
            Vec2::new(42.0, 16.0),
        ),
        4,
        t.accent,
    );
    let hover = motion::state(
        ui.ctx(),
        response.id.with("card-hover"),
        response.hovered() || response.has_focus(),
    );
    let selected = motion::state(ui.ctx(), response.id.with("card-selected"), active);
    let border = motion::mix(
        motion::mix(current.border, current.muted, hover),
        current.focus,
        selected,
    );
    p.rect_stroke(
        preview,
        8,
        Stroke::new(0.5 + 1.5 * selected, border),
        StrokeKind::Inside,
    );
    if active {
        p.text(
            preview.right_top() + Vec2::new(-10.0, 9.0),
            Align2::RIGHT_TOP,
            "✓",
            FontId::proportional(14.0),
            t.text,
        );
    }
    let name_rect = Rect::from_min_max(
        egui::pos2(rect.left(), preview.bottom() + 3.0),
        rect.max - Vec2::new(31.0, 0.0),
    );
    let name_response = ui.put(
        name_rect,
        egui::Button::new(RichText::new(&t.name).size(13.0))
            .frame(false)
            .selected(active),
    );

    let star_rect = Rect::from_min_size(
        egui::pos2(rect.right() - 29.0, preview.bottom() + 3.0),
        Vec2::new(29.0, 27.0),
    );
    let star = ui
        .put(
            star_rect,
            egui::Button::new(if favorite { "★" } else { "☆" }).frame(false),
        )
        .on_hover_text(if favorite {
            "Remove favorite"
        } else {
            "Add favorite"
        });
    (
        response.clicked() || name_response.clicked(),
        star.clicked(),
    )
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            let normal = ctx.input(|i| {
                !i.viewport().maximized.unwrap_or(false) && !i.viewport().minimized.unwrap_or(false)
            });
            if normal && rect.width() >= 640.0 && rect.height() >= 460.0 {
                self.config.window.width = rect.width();
                self.config.window.height = rect.height();
            }
        }
        self.draw(ctx);
        // Existing process/progress observation cadence, only while work is active.
        if self.busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_config();
        if matches!(self.phase, Phase::Installing { .. }) {
            self.rep.cancel();
        }
        // Closing the launcher deliberately leaves a running game alone.
    }
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        self.palette.background.to_normalized_gamma_f32()
    }
}

#[cfg(test)]
#[path = "ui_tests.rs"]
mod tests;
