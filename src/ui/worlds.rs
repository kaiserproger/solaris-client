//! Singleplayer shell screens: world list, world creation, delete confirmation
//! and the "starting world" progress screen. World files are managed by
//! `crate::singleplayer`; starting the server is a `UiCommand` for the session.
use super::{
    state::{NodeAction, Paint, UiCommand, UiEffects},
    Screen, UiState,
};
use crate::{
    protocol::Result,
    singleplayer::{self, CreateSpec},
};

impl UiState {
    pub(crate) fn enter_singleplayer(&mut self) {
        self.refresh_worlds();
        self.page = 0;
        self.selected_world = None;
        self.title_error = None;
        self.transition(Screen::Singleplayer);
        self.invalidate();
    }

    fn refresh_worlds(&mut self) {
        match singleplayer::data_root() {
            Ok(root) => {
                self.worlds = singleplayer::list_worlds(&root);
                self.worlds_error = None;
            }
            Err(error) => {
                self.worlds.clear();
                self.worlds_error = Some(error);
            }
        }
    }

    pub(crate) fn begin_local_start(&mut self, status: &str) {
        self.local_status = status.to_owned();
        self.transition(Screen::SingleplayerStarting);
        self.invalidate();
    }

    pub(crate) fn set_local_status(&mut self, status: String) {
        if self.local_status != status {
            self.local_status = status;
            self.invalidate();
        }
    }

    /// Back to the world list, optionally with an error line above the buttons.
    pub(crate) fn local_start_ended(&mut self, error: Option<String>) {
        let selected = self
            .selected_world
            .and_then(|i| self.worlds.get(i))
            .map(|w| w.dir_name.clone());
        self.enter_singleplayer();
        self.selected_world =
            selected.and_then(|name| self.worlds.iter().position(|w| w.dir_name == name));
        self.title_error = error;
    }

    pub(crate) fn singleplayer_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        self.frame(
            "worlds/panel",
            [cx - 172., 6., 344., (self.height - 12.).max(120.)],
        )?;
        self.label(
            "worlds/title",
            "Singleplayer".into(),
            [cx - 150., 15., 300., 12.],
        )?;
        let list_top = 34.;
        let buttons_y = self.height - 96.;
        // An error line gets its own band above the buttons, taking list rows.
        let list_bottom = buttons_y - 6. - if self.title_error.is_some() { 38. } else { 0. };
        let rows = (((list_bottom - list_top) / 30.).floor().max(1.)) as usize;
        self.page = self.page.min(self.worlds.len().saturating_sub(1) / rows);
        let start = self.page * rows;
        for index in start..(start + rows).min(self.worlds.len()) {
            let y = list_top + (index - start) as f32 * 30.;
            if self.selected_world == Some(index) {
                self.frame(
                    &format!("world/{index}/selected"),
                    [cx - 163., y - 3., 326., 32.],
                )?;
            }
            let world = &self.worlds[index];
            let label = format!(
                "{}  —  {} · {}",
                world.meta.name,
                singleplayer::format_time(world.meta.last_played_ms),
                singleplayer::format_size(world.size),
            );
            self.add(
                format!("world/{index}"),
                "button",
                label,
                [cx - 160., y, 320., 26.],
                true,
                None,
                NodeAction::World(index),
                Paint::Split {
                    delim: "  —  ",
                    desc: "",
                },
            )?;
        }
        if self.worlds.is_empty() {
            self.label(
                "worlds/empty",
                self.worlds_error.clone().unwrap_or_else(|| {
                    "No worlds yet. Create a new world to start playing.".into()
                }),
                [cx - 155., list_top + 8., 310., 35.],
            )?;
        }
        let selected = self.selected_world.is_some();
        self.button(
            "worlds/play",
            "Play Selected World",
            [cx - 160., buttons_y, 320., 26.],
            NodeAction::PlayWorld,
            selected,
            Some("no_world_selected"),
        )?;
        let second = buttons_y + 29.;
        self.button(
            "worlds/create",
            "Create New World",
            [cx - 160., second, 104., 26.],
            NodeAction::Screen(Screen::CreateWorld),
            true,
            None,
        )?;
        self.button(
            "worlds/delete",
            "Delete",
            [cx - 52., second, 104., 26.],
            NodeAction::Screen(Screen::DeleteWorld),
            selected,
            Some("no_world_selected"),
        )?;
        self.button(
            "worlds/folder",
            "Open Folder",
            [cx + 56., second, 104., 26.],
            NodeAction::OpenWorldsFolder,
            true,
            None,
        )?;
        let third = second + 29.;
        self.button(
            "worlds/back",
            "Back",
            [cx - 75., third, 150., 26.],
            NodeAction::Screen(Screen::Title),
            true,
            None,
        )?;
        if self.worlds.len() > rows {
            self.button(
                "worlds/previous",
                "<",
                [cx - 160., third, 36., 26.],
                NodeAction::ServerPage(-1),
                self.page > 0,
                None,
            )?;
            self.button(
                "worlds/next",
                ">",
                [cx + 124., third, 36., 26.],
                NodeAction::ServerPage(1),
                start + rows < self.worlds.len(),
                None,
            )?;
        }
        if let Some(error) = self.title_error.clone() {
            self.label(
                "worlds/error",
                error,
                [cx - 160., buttons_y - 38., 320., 34.],
            )?;
        }
        Ok(())
    }

    pub(crate) fn create_world_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        let top = ((self.height - 220.) / 2.).floor().max(2.);
        self.frame("create/panel", [cx - 120., top, 240., 220.])?;
        self.label(
            "create/title",
            "Create New World".into(),
            [cx - 110., top + 8., 220., 12.],
        )?;
        let x = cx - 100.;
        if !self.fields.contains_key("world/name") {
            self.field("world/name", "New World".into(), 32, false)?;
        }
        if !self.fields.contains_key("world/seed") {
            self.field("world/seed", String::new(), 32, false)?;
        }
        self.label(
            "create/name_label",
            "World name".into(),
            [x, top + 24., 200., 9.],
        )?;
        self.edit_node("world/name", "World name", [x, top + 34., 200., 22.], true)?;
        self.label(
            "create/seed_label",
            "Seed (blank = random)".into(),
            [x, top + 60., 200., 9.],
        )?;
        self.edit_node("world/seed", "Seed", [x, top + 70., 200., 22.], true)?;
        self.button(
            "create/mode",
            &format!("Game Mode: {}", self.new_world_mode.label()),
            [x, top + 98., 200., 24.],
            NodeAction::WorldOption("game_mode"),
            true,
            None,
        )?;
        self.button(
            "create/difficulty",
            &format!("Difficulty: {}", self.new_world_difficulty.label()),
            [x, top + 126., 200., 24.],
            NodeAction::WorldOption("difficulty"),
            true,
            None,
        )?;
        self.button(
            "create/submit",
            "Create New World",
            [x, top + 156., 200., 26.],
            NodeAction::CreateWorld,
            true,
            None,
        )?;
        self.button(
            "create/cancel",
            "Cancel",
            [x, top + 186., 200., 26.],
            NodeAction::Screen(Screen::Singleplayer),
            true,
            None,
        )?;
        if let Some(error) = self.title_error.clone() {
            self.label("create/error", error, [cx - 112., top + 222., 224., 20.])?;
        }
        Ok(())
    }

    pub(crate) fn delete_world_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        let cy = self.height / 2.;
        let name = self
            .selected_world
            .and_then(|i| self.worlds.get(i))
            .map(|w| w.meta.name.clone())
            .unwrap_or_default();
        self.frame("delete/panel", [cx - 160., cy - 60., 320., 120.])?;
        self.label(
            "delete/title",
            "Delete this world?".into(),
            [cx - 140., cy - 48., 280., 12.],
        )?;
        self.label(
            "delete/message",
            format!("'{name}' will be lost forever! This cannot be undone."),
            [cx - 140., cy - 28., 280., 28.],
        )?;
        self.button(
            "delete/confirm",
            "Delete",
            [cx - 140., cy + 12., 136., 26.],
            NodeAction::DeleteWorld,
            true,
            None,
        )?;
        self.button(
            "delete/cancel",
            "Cancel",
            [cx + 4., cy + 12., 136., 26.],
            NodeAction::Screen(Screen::Singleplayer),
            true,
            None,
        )
    }

    pub(crate) fn local_start_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        let cy = self.height / 2.;
        self.frame("starting/panel", [cx - 170., cy - 50., 340., 100.])?;
        self.label(
            "starting/title",
            "Starting Singleplayer World".into(),
            [cx - 150., cy - 38., 300., 12.],
        )?;
        self.label(
            "starting/status",
            self.local_status.clone(),
            [cx - 154., cy - 16., 308., 24.],
        )?;
        self.button(
            "starting/cancel",
            "Cancel",
            [cx - 75., cy + 16., 150., 26.],
            NodeAction::CancelLocalStart,
            true,
            None,
        )
    }

    /// Singleplayer actions; the arm set mirrors `NodeAction`'s world variants.
    pub(crate) fn activate_world_action(
        &mut self,
        action: NodeAction,
        e: &mut UiEffects,
    ) -> Result<()> {
        match action {
            NodeAction::World(index) => {
                if index >= self.worlds.len() {
                    return Err("world_not_found".into());
                }
                self.selected_world = Some(index);
                self.invalidate();
            }
            NodeAction::PlayWorld => {
                let world = self
                    .selected_world
                    .and_then(|i| self.worlds.get(i))
                    .ok_or("no_world_selected")?;
                e.commands.push(UiCommand::PlayWorld {
                    dir_name: world.dir_name.clone(),
                });
                self.begin_local_start("Starting world server");
            }
            NodeAction::CancelLocalStart => e.commands.push(UiCommand::CancelLocalStart),
            NodeAction::OpenWorldsFolder => {
                let root = singleplayer::data_root()?;
                singleplayer::open_folder(&singleplayer::worlds_dir(&root))?;
            }
            NodeAction::WorldOption(key) => {
                match key {
                    "game_mode" => self.new_world_mode = self.new_world_mode.next(),
                    "difficulty" => self.new_world_difficulty = self.new_world_difficulty.next(),
                    _ => return Err("unknown_world_option".into()),
                }
                self.invalidate();
            }
            NodeAction::CreateWorld => {
                let text = |ui: &Self, id: &str| {
                    ui.fields
                        .get(id)
                        .map(|f| f.text.clone())
                        .unwrap_or_default()
                };
                let spec = CreateSpec {
                    name: text(self, "world/name"),
                    seed: text(self, "world/seed"),
                    game_mode: self.new_world_mode,
                    difficulty: self.new_world_difficulty,
                };
                let root = singleplayer::data_root()?;
                match singleplayer::create_world(&root, &spec) {
                    Ok(world) => {
                        self.fields.remove("world/name");
                        self.fields.remove("world/seed");
                        self.enter_singleplayer();
                        self.selected_world = self
                            .worlds
                            .iter()
                            .position(|w| w.dir_name == world.dir_name);
                    }
                    Err(error) => {
                        self.title_error = Some(error);
                        self.invalidate();
                    }
                }
            }
            NodeAction::DeleteWorld => {
                let dir_name = self
                    .selected_world
                    .and_then(|i| self.worlds.get(i))
                    .map(|w| w.dir_name.clone())
                    .ok_or("no_world_selected")?;
                let root = singleplayer::data_root()?;
                let result = singleplayer::delete_world(&root, &dir_name);
                self.enter_singleplayer();
                if let Err(error) = result {
                    self.title_error = Some(error);
                }
            }
            _ => return Err("not_a_world_action".into()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inventory::InventoryStore,
        protocol::content::Content,
        singleplayer::GameMode,
        ui::{Modifiers, UiIntent},
    };

    #[test]
    fn world_screens_create_play_cancel_and_delete() {
        let root = std::env::temp_dir().join(format!("solaris-ui-worlds-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        // SAFETY: only this test reads SOLARIS_CLIENT_ROOT in the test binary.
        unsafe { std::env::set_var("SOLARIS_CLIENT_ROOT", &root) };
        let mut content = Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        let mut ui = UiState::new_state(&content).unwrap();
        let mut inventory = InventoryStore::default();
        ui.transition(Screen::Title);
        macro_rules! act {
            ($action:expr) => {
                ui.activate_for_test($action, &mut inventory, &content)
            };
        }

        act!(NodeAction::Screen(Screen::Singleplayer)).unwrap();
        assert_eq!(ui.screen(), Screen::Singleplayer);
        assert!(ui.worlds.is_empty());
        // No world selected: Play is refused.
        assert!(act!(NodeAction::PlayWorld).is_err());

        act!(NodeAction::Screen(Screen::CreateWorld)).unwrap();
        ui.field("world/name", "Alpha".into(), 32, false).unwrap();
        ui.field("world/seed", "77".into(), 32, false).unwrap();
        act!(NodeAction::WorldOption("game_mode")).unwrap();
        act!(NodeAction::CreateWorld).unwrap();
        assert_eq!(ui.screen(), Screen::Singleplayer);
        assert_eq!(ui.worlds.len(), 1);
        assert_eq!(ui.selected_world, Some(0));
        let meta = &ui.worlds[0].meta;
        assert_eq!((meta.seed, meta.game_mode), (77, GameMode::Creative));

        // An empty name stays on the create screen with an error.
        act!(NodeAction::Screen(Screen::CreateWorld)).unwrap();
        ui.field("world/name", " ".into(), 32, false).unwrap();
        act!(NodeAction::CreateWorld).unwrap();
        assert_eq!(ui.screen(), Screen::CreateWorld);
        assert!(ui.title_error.is_some());
        assert!(ui.close_menu());
        assert_eq!(ui.screen(), Screen::Singleplayer);
        ui.selected_world = Some(0);

        let effects = act!(NodeAction::PlayWorld).unwrap();
        assert!(matches!(
            effects.commands.as_slice(),
            [UiCommand::PlayWorld { dir_name }] if dir_name == "Alpha"
        ));
        assert_eq!(ui.screen(), Screen::SingleplayerStarting);
        let escape = ui
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
        assert!(matches!(
            escape.commands.as_slice(),
            [UiCommand::CancelLocalStart]
        ));
        ui.local_start_ended(Some("boom".into()));
        assert_eq!(ui.screen(), Screen::Singleplayer);
        assert_eq!(ui.title_error.as_deref(), Some("boom"));

        ui.selected_world = Some(0);
        act!(NodeAction::Screen(Screen::DeleteWorld)).unwrap();
        assert_eq!(ui.screen(), Screen::DeleteWorld);
        act!(NodeAction::DeleteWorld).unwrap();
        assert_eq!(ui.screen(), Screen::Singleplayer);
        assert!(ui.worlds.is_empty());
        assert!(!singleplayer::worlds_dir(&root).join("Alpha").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
