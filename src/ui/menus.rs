//! Native shell menus. Only addresses/settings are persisted, never server resources.
use super::{
    state::{NodeAction, Paint},
    Screen, UiState,
};
use crate::protocol::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SavedServer {
    pub name: String,
    pub address: String,
}

pub(super) fn load_servers() -> Vec<SavedServer> {
    crate::local_config::client_settings_path()
        .and_then(|path| std::fs::read(path.with_file_name("servers.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Vec<SavedServer>>(&bytes).ok())
        .unwrap_or_default()
}

impl UiState {
    /// Local shell screens also accept semantic input before a session exists.
    pub fn shell_screen_active(&self) -> bool {
        matches!(
            self.screen,
            Screen::Title | Screen::Multiplayer | Screen::DirectConnect | Screen::Disconnected
        ) || (matches!(self.screen, Screen::Settings | Screen::Accessibility)
            && self.settings_parent == Screen::Title)
    }

    pub(crate) fn close_menu(&mut self) -> bool {
        let parent = match self.screen {
            Screen::Settings | Screen::Accessibility => self.settings_parent,
            Screen::Multiplayer | Screen::Disconnected => Screen::Title,
            Screen::DirectConnect => Screen::Multiplayer,
            _ => return false,
        };
        self.transition(parent);
        self.invalidate();
        true
    }

    pub(crate) fn show_disconnected(&mut self, reason: String) {
        let component = [
            "Play disconnect: ",
            "configuration disconnect: ",
            "login disconnect: ",
        ]
        .into_iter()
        .find_map(|prefix| reason.strip_prefix(prefix))
        .and_then(|payload| serde_json::from_str::<Value>(payload).ok());
        self.title_error = Some(
            component
                .as_ref()
                .and_then(|component| self.text.language.plain(component).ok())
                .unwrap_or(reason),
        );
        self.transition(Screen::Disconnected);
        self.invalidate();
    }

    pub(crate) fn save_server(&mut self) -> Result<()> {
        let address = self.fields["title/server"].text.trim().to_owned();
        if address.is_empty() {
            return Err("Server address is required".into());
        }
        let name = self
            .fields
            .get("server/name")
            .map(|f| f.text.trim())
            .unwrap_or("");
        let entry = SavedServer {
            name: if name.is_empty() {
                address.clone()
            } else {
                name.to_owned()
            },
            address,
        };
        let path = crate::local_config::client_settings_path()
            .ok_or("Cannot locate local server list")?
            .with_file_name("servers.json");
        let mut servers = self.servers.clone();
        servers.push(entry);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(&servers).map_err(|e| e.to_string())?,
        )
        .and_then(|()| std::fs::rename(&tmp, &path))
        .map_err(|e| format!("Cannot save server list: {e}"))?;
        self.servers = servers;
        self.transition(Screen::Multiplayer);
        self.invalidate();
        Ok(())
    }

    /// Themed translucent panel node (frame only; carries no action).
    fn frame(&mut self, id: &str, rect: [f32; 4]) -> Result<()> {
        self.add(
            id.into(),
            "presentation",
            String::new(),
            rect,
            true,
            None,
            NodeAction::None,
            Paint::Frame,
        )
    }

    pub(crate) fn title_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        let mark_y = (self.height * 0.13).floor();
        self.label("title/name", "Solaris".into(), [cx - 150., mark_y, 300., 36.])?;
        self.label(
            "title/subtitle",
            "Minecraft 26.1.2 · Native client".into(),
            [cx - 160., mark_y + 42., 320., 12.],
        )?;
        let top = (self.height / 2. - 2.).max(mark_y + 66.).floor();
        self.frame("title/panel", [cx - 100., top - 10., 200., 3. * 32. + 14.])?;
        for (i, (id, label, action)) in [
            (
                "title/multiplayer",
                "Multiplayer",
                NodeAction::Screen(Screen::Multiplayer),
            ),
            (
                "title/options",
                "Options...",
                NodeAction::Screen(Screen::Settings),
            ),
            ("title/quit", "Quit Game", NodeAction::Quit),
        ]
        .into_iter()
        .enumerate()
        {
            self.button(
                id,
                label,
                [cx - 88., top + i as f32 * 32., 176., 26.],
                action,
                true,
                None,
            )?;
        }
        self.label(
            "title/version",
            "Independent client · Offline test profile".into(),
            [8., self.height - 15., self.width - 16., 10.],
        )?;
        Ok(())
    }

    pub(crate) fn multiplayer_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        self.frame(
            "multiplayer/panel",
            [cx - 172., 6., 344., (self.height - 12.).max(120.)],
        )?;
        self.label("multiplayer/title", "Play Multiplayer".into(), [cx - 150., 15., 300., 12.])?;
        let list_top = 34.;
        let buttons_y = self.height - 68.;
        let rows = (((buttons_y - 6. - list_top) / 30.).floor().max(1.)) as usize;
        self.page = self.page.min(self.servers.len().saturating_sub(1) / rows);
        let start = self.page * rows;
        for index in start..(start + rows).min(self.servers.len()) {
            let server = &self.servers[index];
            let label = format!("{}  —  {}", server.name, server.address);
            self.add(
                format!("server/{index}"),
                "button",
                label,
                [cx - 160., list_top + (index - start) as f32 * 30., 320., 26.],
                true,
                None,
                NodeAction::JoinServer(index),
                Paint::Split {
                    delim: "  —  ",
                    desc: "",
                },
            )?;
        }
        if self.servers.is_empty() {
            self.label(
                "multiplayer/empty",
                "No saved servers. Add a server or use Direct Connect.".into(),
                [cx - 155., list_top + 8., 310., 35.],
            )?;
        }
        self.button(
            "multiplayer/direct",
            "Direct Connect",
            [cx - 160., buttons_y, 157., 26.],
            NodeAction::Screen(Screen::DirectConnect),
            true,
            None,
        )?;
        self.button(
            "multiplayer/add",
            "Add Server",
            [cx + 3., buttons_y, 157., 26.],
            NodeAction::AddServer,
            true,
            None,
        )?;
        self.button(
            "multiplayer/back",
            "Back",
            [cx - 75., buttons_y + 32., 150., 26.],
            NodeAction::Screen(Screen::Title),
            true,
            None,
        )?;
        if self.servers.len() > rows {
            self.button(
                "multiplayer/previous",
                "<",
                [cx - 160., buttons_y + 32., 36., 26.],
                NodeAction::ServerPage(-1),
                self.page > 0,
                None,
            )?;
            self.button(
                "multiplayer/next",
                ">",
                [cx + 124., buttons_y + 32., 36., 26.],
                NodeAction::ServerPage(1),
                start + rows < self.servers.len(),
                None,
            )?;
        }
        Ok(())
    }

    pub(crate) fn connect_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        let top = ((self.height - 212.) / 2.).floor().max(2.);
        self.frame("connect/panel", [cx - 120., top, 240., 212.])?;
        self.label(
            "connect/title",
            if self.adding_server {
                "Add Server"
            } else {
                "Direct Connect"
            }
            .into(),
            [cx - 110., top + 10., 220., 12.],
        )?;
        let x = cx - 100.;
        let field = if self.adding_server {
            "server/name"
        } else {
            "title/username"
        };
        let label = if self.adding_server {
            "Server name"
        } else {
            "Offline username"
        };
        if !self.fields.contains_key(field) {
            self.field(field, "Minecraft Server".into(), 64, false)?;
        }
        self.label("connect/name_label", label.into(), [x, top + 30., 200., 9.])?;
        self.edit_node(field, label, [x, top + 41., 200., 24.], true)?;
        self.label(
            "title/server_label",
            "Server Address".into(),
            [x, top + 72., 200., 9.],
        )?;
        self.edit_node(
            "title/server",
            "Server address",
            [x, top + 83., 200., 24.],
            true,
        )?;
        self.button(
            if self.adding_server {
                "server/save"
            } else {
                "title/connect"
            },
            if self.adding_server {
                "Done"
            } else {
                "Join Server"
            },
            [x, top + 117., 200., 26.],
            if self.adding_server {
                NodeAction::SaveServer
            } else {
                NodeAction::Connect
            },
            true,
            None,
        )?;
        self.button(
            "connect/cancel",
            "Cancel",
            [x, top + 147., 200., 26.],
            NodeAction::Screen(Screen::Multiplayer),
            true,
            None,
        )?;
        self.label(
            "title/error",
            self.title_error.clone().unwrap_or_default(),
            [cx - 112., top + 178., 224., 28.],
        )?;
        Ok(())
    }

    pub(crate) fn disconnected_tree(&mut self) -> Result<()> {
        self.panel();
        let cx = self.width / 2.;
        let cy = self.height / 2.;
        self.frame("disconnect/panel", [cx - 180., cy - 74., 360., 148.])?;
        self.label(
            "disconnect/title",
            "Connection Lost".into(),
            [cx - 150., cy - 62., 300., 14.],
        )?;
        self.label(
            "disconnect/reason",
            self.title_error
                .clone()
                .unwrap_or_else(|| "Disconnected".into()),
            [cx - 164., cy - 38., 328., 48.],
        )?;
        self.button(
            "disconnect/back",
            "Back to Title Screen",
            [cx - 100., cy + 22., 200., 26.],
            NodeAction::Screen(Screen::Title),
            true,
            None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        inventory::InventoryStore,
        protocol::content::Content,
        ui::{Modifiers, UiIntent},
    };
    fn state() -> (Content, InventoryStore, UiState) {
        let mut content = Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        let ui = UiState::new_state(&content).unwrap();
        (content, InventoryStore::default(), ui)
    }
    #[test]
    fn options_escape_returns_to_origin_and_pause_escape_resumes() {
        let (content, mut inventory, mut ui) = state();
        ui.transition(Screen::Title);
        ui.activate_for_test(
            NodeAction::Screen(Screen::Settings),
            &mut inventory,
            &content,
        )
        .unwrap();
        assert!(ui.shell_screen_active());
        ui.execute(
            UiIntent::Key {
                key: "Escape".into(),
                pressed: true,
                modifiers: Modifiers::default(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
        assert_eq!(ui.screen(), Screen::Title);
        ui.transition(Screen::Pause);
        ui.activate_for_test(
            NodeAction::Screen(Screen::Settings),
            &mut inventory,
            &content,
        )
        .unwrap();
        assert!(!ui.shell_screen_active());
        assert!(ui.close_menu());
        assert_eq!(ui.screen(), Screen::Pause);
        ui.execute(
            UiIntent::Key {
                key: "Escape".into(),
                pressed: true,
                modifiers: Modifiers::default(),
            },
            &mut inventory,
            &content,
        )
        .unwrap();
        assert_eq!(ui.screen(), Screen::Game);
    }
    #[test]
    fn disconnect_keeps_reason_and_closes_to_title() {
        let (_, _, mut ui) = state();
        ui.show_disconnected("Kicked: example reason".into());
        assert_eq!(ui.screen(), Screen::Disconnected);
        assert!(ui.dialog_screen_active());
        assert_eq!(
            ui.title_error_text().as_deref(),
            Some("Kicked: example reason")
        );
        assert!(ui.close_menu());
        assert_eq!(ui.screen(), Screen::Title);
        ui.show_disconnected(r#"Play disconnect: {"text":"Removed by server"}"#.into());
        assert_eq!(ui.title_error_text().as_deref(), Some("Removed by server"));
    }
}
