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
    crate::local_config::servers_path()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Vec<SavedServer>>(&bytes).ok())
        .unwrap_or_default()
}

pub(super) fn write_servers(path: &std::path::Path, servers: &[SavedServer]) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(servers).map_err(|e| e.to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create server list directory {}: {e}", parent.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|e| format!("Cannot save server list {}: {e}", path.display()))
}

impl UiState {
    /// Local shell screens also accept semantic input before a session exists.
    pub fn shell_screen_active(&self) -> bool {
        matches!(
            self.screen,
            Screen::Title
                | Screen::Multiplayer
                | Screen::DirectConnect
                | Screen::Disconnected
                | Screen::Singleplayer
                | Screen::CreateWorld
                | Screen::DeleteWorld
                | Screen::SingleplayerStarting
        ) || (matches!(self.screen, Screen::Settings | Screen::Accessibility)
            && self.settings_parent == Screen::Title)
    }

    pub(crate) fn close_menu(&mut self) -> bool {
        if self.screen == Screen::Multiplayer && self.deleting_server {
            self.deleting_server = false;
            self.title_error = None;
            self.invalidate();
            return true;
        }
        let parent = match self.screen {
            Screen::Settings | Screen::Accessibility => self.settings_parent,
            Screen::Multiplayer | Screen::Disconnected | Screen::Singleplayer => Screen::Title,
            Screen::DirectConnect => Screen::Multiplayer,
            Screen::CreateWorld | Screen::DeleteWorld => {
                self.enter_singleplayer();
                return true;
            }
            _ => return false,
        };
        self.transition(parent);
        self.invalidate();
        true
    }

    pub(crate) fn show_disconnected(&mut self, reason: String) {
        self.show_disconnected_titled(None, reason);
    }

    pub(crate) fn show_disconnected_titled(&mut self, title: Option<&str>, reason: String) {
        self.disconnect_title = title.map(str::to_owned);
        // Packet context remains in the session log and error event. Only unwrap
        // the three actual disconnect packet envelopes, not arbitrary errors.
        let contextual = [
            ("Login", crate::protocol::packet_ids::login::clientbound::LOGIN_DISCONNECT, "login disconnect: "),
            ("Configuration", crate::protocol::packet_ids::configuration::clientbound::DISCONNECT, "configuration disconnect: "),
            ("Play", crate::protocol::packet_ids::play::clientbound::DISCONNECT, "Play disconnect: "),
        ].into_iter().find_map(|(phase, id, prefix)| {
            let rest = reason.strip_prefix(&format!("{phase} packet 0x{id:02x} sequence "))?;
            let (sequence, payload) = rest.split_once(": ")?;
            if sequence.is_empty() || !sequence.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            payload.strip_prefix(prefix)
        });
        let component = contextual.or_else(|| [
            "Play disconnect: ",
            "configuration disconnect: ",
            "login disconnect: ",
        ].into_iter().find_map(|prefix| reason.strip_prefix(prefix)))
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
        let mut servers = self.servers.clone();
        let selected = if let Some(index) = self.editing_server {
            *servers.get_mut(index).ok_or("server_not_found")? = entry;
            index
        } else {
            servers.push(entry);
            servers.len() - 1
        };
        let path = crate::local_config::servers_path().ok_or("Cannot locate local server list")?;
        write_servers(&path, &servers)?;
        self.servers = servers;
        self.selected_server = Some(selected);
        self.editing_server = None;
        self.title_error = None;
        self.transition(Screen::Multiplayer);
        self.invalidate();
        Ok(())
    }

    /// Themed translucent panel node (frame only; carries no action).
    pub(super) fn frame(&mut self, id: &str, rect: [f32; 4]) -> Result<()> {
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
            format!("Minecraft {} · Native client", crate::singleplayer::GAME_VERSION),
            [cx - 160., mark_y + 42., 320., 12.],
        )?;
        let top = (self.height / 2. - 22.).max(mark_y + 58.).floor();
        self.frame("title/panel", [cx - 100., top - 10., 200., 4. * 30. + 14.])?;
        for (i, (id, label, action)) in [
            (
                "title/singleplayer",
                "Singleplayer",
                NodeAction::Screen(Screen::Singleplayer),
            ),
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
                [cx - 88., top + i as f32 * 30., 176., 26.],
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
        if self.deleting_server {
            self.label("server/delete_title", "Delete this server?".into(), [cx - 150., 45., 300., 14.])?;
            let name = self.selected_server.and_then(|index| self.servers.get(index))
                .map(|server| server.name.clone()).unwrap_or_default();
            self.label("server/delete_name", name, [cx - 150., 70., 300., 24.])?;
            self.button("server/delete_confirm", "Delete", [cx - 154., 110., 150., 26.],
                NodeAction::ConfirmDeleteServer, true, None)?;
            self.button("server/delete_cancel", "Cancel", [cx + 4., 110., 150., 26.],
                NodeAction::CancelDeleteServer, true, None)?;
            self.label("server/delete_error", self.title_error.clone().unwrap_or_default(),
                [cx - 150., 145., 300., 40.])?;
            return Ok(());
        }
        self.frame(
            "multiplayer/panel",
            [cx - 172., 6., 344., (self.height - 12.).max(120.)],
        )?;
        self.label("multiplayer/title", "Play Multiplayer".into(), [cx - 150., 15., 300., 12.])?;
        let list_top = 34.;
        let buttons_y = self.height - 100.;
        let rows = (((buttons_y - 6. - list_top) / 30.).floor().max(1.)) as usize;
        self.page = self.page.min(self.servers.len().saturating_sub(1) / rows);
        let start = self.page * rows;
        for index in start..(start + rows).min(self.servers.len()) {
            let server = &self.servers[index];
            let label = format!("{}{}  —  {}", if self.selected_server == Some(index) { "> " } else { "" }, server.name, server.address);
            self.add(
                format!("server/{index}"),
                "button",
                label,
                [cx - 160., list_top + (index - start) as f32 * 30., 320., 26.],
                true,
                None,
                NodeAction::SelectServer(index),
                Paint::Split {
                    delim: "  —  ",
                    desc: "",
                    conflict: false,
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
        let selected = self.selected_server.is_some_and(|index| index < self.servers.len());
        for (id, label, x, action) in [
            ("multiplayer/join", "Join Server", cx - 160., NodeAction::JoinServer(self.selected_server.unwrap_or(0))),
            ("multiplayer/edit", "Edit", cx - 52., NodeAction::EditServer),
            ("multiplayer/delete", "Delete", cx + 56., NodeAction::DeleteServer),
        ] {
            self.button(id, label, [x, buttons_y, 104., 26.], action, selected, Some("no_server_selected"))?;
        }
        self.button(
            "multiplayer/direct",
            "Direct Connect",
            [cx - 160., buttons_y + 32., 157., 26.],
            NodeAction::Screen(Screen::DirectConnect),
            true,
            None,
        )?;
        self.button(
            "multiplayer/add",
            "Add Server",
            [cx + 3., buttons_y + 32., 157., 26.],
            NodeAction::AddServer,
            true,
            None,
        )?;
        self.button(
            "multiplayer/back",
            "Back",
            [cx - 75., buttons_y + 64., 150., 26.],
            NodeAction::Screen(Screen::Title),
            true,
            None,
        )?;
        if self.servers.len() > rows {
            self.button(
                "multiplayer/previous",
                "<",
                [cx - 160., buttons_y + 64., 36., 26.],
                NodeAction::ServerPage(-1),
                self.page > 0,
                None,
            )?;
            self.button(
                "multiplayer/next",
                ">",
                [cx + 124., buttons_y + 64., 36., 26.],
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
            if self.editing_server.is_some() {
                "Edit Server"
            } else if self.adding_server {
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
        let reason = self
            .title_error
            .clone()
            .unwrap_or_else(|| "Disconnected".into());
        // Server crash reports carry a log tail: grow the panel for extra lines.
        let rows: usize = reason
            .lines()
            .map(|line| line.chars().count().div_ceil(52).max(1))
            .sum();
        let extra = (rows.saturating_sub(4) as f32 * 10.).min(100.);
        let cy = self.height / 2. - extra / 2.;
        self.frame(
            "disconnect/panel",
            [cx - 180., cy - 74., 360., 148. + extra],
        )?;
        self.label(
            "disconnect/title",
            self.disconnect_title
                .clone()
                .unwrap_or_else(|| "Connection Lost".into()),
            [cx - 150., cy - 62., 300., 14.],
        )?;
        self.label(
            "disconnect/reason",
            reason,
            [cx - 164., cy - 38., 328., 48. + extra],
        )?;
        self.button(
            "disconnect/back",
            "Back to Title Screen",
            [cx - 100., cy + 22. + extra, 200., 26.],
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
    fn rc1_title_subtitle_uses_target_game_version() {
        let (_, _, mut ui) = state();
        ui.title_tree().unwrap();
        let subtitle = &ui.nodes.iter().find(|n| n.id == "title/subtitle").unwrap().label;
        assert_eq!(subtitle, &format!("Minecraft {} · Native client", crate::singleplayer::GAME_VERSION));
    }
    #[test]
    fn rc1_saved_server_row_selects_without_connecting() {
        let (content, mut inventory, mut ui) = state();
        ui.enter_title(None, Some("Player")).unwrap();
        ui.servers = vec![SavedServer {
            name: "Example".into(),
            address: "localhost:25565".into(),
        }];
        ui.transition(Screen::Multiplayer);
        ui.multiplayer_tree().unwrap();
        let action = ui.nodes.iter().find(|n| n.id == "server/0").unwrap().action.clone();
        ui.activate_for_test(action, &mut inventory, &content).unwrap();
        assert_eq!(ui.screen(), Screen::Multiplayer, "a row selects, Join connects");
        for id in ["multiplayer/join", "multiplayer/edit", "multiplayer/delete"] {
            assert!(ui.nodes.iter().any(|n| n.id == id), "missing {id}");
        }
    }
    #[test]
    fn rc1_saved_server_edit_delete_rewrites_file_with_confirmation() {
        if std::env::var_os("SOLARIS_SERVER_REWRITE_TEST").is_none() {
            let dir = std::env::temp_dir().join(format!("solaris-servers-{}", uuid::Uuid::new_v4()));
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "ui::menus::tests::rc1_saved_server_edit_delete_rewrites_file_with_confirmation", "--nocapture"])
                .env("SOLARIS_SERVER_REWRITE_TEST", "1")
                .env("SOLARIS_CLIENT_ROOT", &dir)
                .env("SOLARIS_CLIENT_SETTINGS", dir.join("client-settings.json"))
                .env("XDG_DATA_HOME", &dir)
                .status().unwrap();
            let _ = std::fs::remove_dir_all(dir);
            assert!(result.success());
            return;
        }
        let (content, mut inventory, mut ui) = state();
        ui.enter_title(None, Some("Player")).unwrap();
        ui.activate_for_test(NodeAction::AddServer, &mut inventory, &content).unwrap();
        ui.fields.get_mut("title/server").unwrap().text = "old.example".into();
        ui.save_server().unwrap();
        ui.multiplayer_tree().unwrap();
        let select = ui.nodes.iter().find(|n| n.id == "server/0").unwrap().action.clone();
        ui.activate_for_test(select, &mut inventory, &content).unwrap();
        ui.nodes.clear();
        ui.multiplayer_tree().unwrap();
        let edit = ui.nodes.iter().find(|n| n.id == "multiplayer/edit").expect("Edit button").action.clone();
        ui.activate_for_test(edit, &mut inventory, &content).unwrap();
        ui.fields.get_mut("title/server").unwrap().text = "new.example".into();
        ui.save_server().unwrap();
        assert_eq!(load_servers().len(), 1);
        assert_eq!(load_servers()[0].address, "new.example");
        ui.nodes.clear();
        ui.multiplayer_tree().unwrap();
        let delete = ui.nodes.iter().find(|n| n.id == "multiplayer/delete").unwrap().action.clone();
        ui.activate_for_test(delete.clone(), &mut inventory, &content).unwrap();
        assert_eq!(load_servers().len(), 1, "requesting Delete must not remove a server");
        assert!(ui.close_menu());
        assert_eq!(load_servers().len(), 1, "cancelling confirmation must preserve the entry");
        ui.activate_for_test(delete, &mut inventory, &content).unwrap();
        ui.nodes.clear();
        ui.multiplayer_tree().unwrap();
        let confirm = ui.nodes.iter().find(|n| n.id == "server/delete_confirm").unwrap().action.clone();
        ui.activate_for_test(confirm, &mut inventory, &content).unwrap();
        assert!(load_servers().is_empty());
        assert_eq!(ui.screen(), Screen::Multiplayer);
    }
    #[test]
    fn rc1_anvil_focused_text_and_editing_emit_rename() {
        let (content, mut inventory, mut ui) = state();
        inventory.screen_open = true;
        let mut menu = inventory.player.clone();
        menu.kind = crate::inventory::MenuKind::Anvil;
        menu.slots.resize(menu.kind.slot_count(), None);
        inventory.active = Some(menu);
        ui.synchronize(&inventory, &content).unwrap();
        ui.screen = Screen::Menu;
        ui.field("anvil/name", String::new(), 50, false).unwrap();
        ui.edit_node("anvil/name", "Name", [10., 10., 120., 20.], true).unwrap();
        ui.focus = Some("anvil/name".into());
        assert!(ui.text_field_focused());
        let effects = ui.execute(UiIntent::Text { text: "abcde".into() }, &mut inventory, &content).unwrap();
        assert!(effects.commands.iter().any(|c| matches!(c, crate::ui::UiCommand::InventoryRename { text, .. } if text == "abcde")));
        for key in ["ArrowLeft", "Backspace", "Delete"] {
            ui.execute(UiIntent::Key { key: key.into(), pressed: true, modifiers: Modifiers::default() }, &mut inventory, &content).unwrap();
        }
        assert_eq!(ui.fields["anvil/name"].text, "abc");
        assert_eq!(ui.screen(), Screen::Menu);
    }
    #[test]
    fn server_list_write_errors_name_target_and_preserve_old_file() {
        let dir = std::env::temp_dir().join(format!("solaris-server-write-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("servers.json");
        let original = vec![SavedServer { name: "Keep".into(), address: "keep.example".into() }];
        write_servers(&path, &original).unwrap();
        std::fs::create_dir(path.with_extension("json.tmp")).unwrap();
        let error = write_servers(&path, &[]).unwrap_err();
        assert!(error.contains(path.to_str().unwrap()));
        let saved: Vec<SavedServer> = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].address, "keep.example");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn rc1_edit_server_resets_old_address_selection_before_typing() {
        let (content, mut inventory, mut ui) = state();
        ui.enter_title(Some("a-very-long-previous.example:25565"), Some("Player")).unwrap();
        ui.servers = vec![SavedServer { name: "Short".into(), address: "é.test".into() }];
        ui.selected_server = Some(0);
        ui.activate_for_test(NodeAction::EditServer, &mut inventory, &content).unwrap();
        let field = ui.fields.get_mut("title/server").unwrap();
        assert!(field.text.is_char_boundary(field.cursor));
        assert!(field.text.is_char_boundary(field.anchor));
        field.insert("x").unwrap();
        assert!(field.text.contains('x'));
    }

    #[test]
    fn screenshot_feedback_enters_the_local_chat_history() {
        let (_, _, mut ui) = state();
        ui.local_message("Saved screenshot as 2026-10-29_12.34.56.png").unwrap();
        ui.local_message("Couldn't save screenshot: disk full").unwrap();
        assert_eq!(ui.chat().len(), 2);
        assert_eq!(ui.chat().back().unwrap().component["text"], "Couldn't save screenshot: disk full");
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
