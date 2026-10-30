use crate::protocol::packet_ids::play::serverbound as sb;
use super::*;
use crate::{
    entities::Handle,
    player::{BlockHit, Hit, InteractionIntent, UiTrigger, UsePrediction},
    ui::{EditorState, UiCommand, UiIntent},
};
impl Session {
    pub fn input(&mut self, intent: InputIntent) -> Result<Value> {
        match intent {
            InputIntent::ReleaseAll => {
                self.keys.clear();
                self.player.release_all(&mut self.commands)?;
                if let Some(ui) = &mut self.ui {
                    ui.release_all();
                }
                if self.network.is_some() {
                    self.flush_commands()?;
                } else {
                    self.commands.clear();
                }
            }
            InputIntent::Look { yaw, pitch } => self.player.look(yaw, pitch)?,
            InputIntent::Hotbar { slot } => {
                if slot > 8 {
                    return Err("hotbar slot outside 0..8".into());
                }
                self.inventory.selected_slot = slot;
                if self.phase == "play" {
                    let mut w = Writer::default();
                    w.0.extend(i16::from(slot).to_be_bytes());
                    self.send(sb::SET_CARRIED_ITEM, w)?;
                }
            }
            InputIntent::Key { key, pressed } => {
                let key = if key == "key.keyboard.tab" && self.screen() == "game" {
                    self.bindings.action(crate::controls::Binding::Key(winit::keyboard::KeyCode::Tab))
                } else {
                    normalize_key(&key)
                }.ok_or_else(|| format!("unsupported ordinary key {key}"))?;
                let edge = if pressed {
                    self.keys.insert(key.into())
                } else {
                    self.keys.remove(key)
                };
                // Dialog screens (death/pause/dialog overlays + title) own
                // keyboard UX: Escape/Enter/Tab must reach UiState even when no
                // container is open. Other gameplay keys are ignored there
                // (fail-closed: no movement while dead) instead of erroring noise.
                // Title additionally runs pre-play; the dialog arm below only
                // fires in play, the title path goes through execute_ui's gate.
                if (self.phase == "play"
                    || self
                        .ui
                        .as_ref()
                        .is_some_and(UiState::shell_screen_active))
                    && self.ui.as_ref().is_some_and(|ui| {
                        ui.dialog_screen_active() && !ui.has_open_container(&self.inventory)
                    })
                {
                    if matches!(key, "ui_cancel" | "ui_confirm" | "ui_focus_next") {
                        if pressed {
                            let intent = crate::ui::UiIntent::Key {
                                key: match key {
                                    "ui_cancel" => "Escape".into(),
                                    "ui_confirm" => "Enter".into(),
                                    _ => "Tab".into(),
                                },
                                pressed: true,
                                modifiers: crate::ui::Modifiers::default(),
                            };
                            self.execute_ui(intent)?;
                        }
                    } else if !matches!(
                        key,
                        "forward"
                            | "backward"
                            | "left"
                            | "right"
                            | "jump"
                            | "sneak"
                            | "sprint"
                            | "attack"
                            | "use"
                    ) {
                        // Inventory/drop/pick etc. on a dialog screen: refuse loudly, stay put.
                        if edge && pressed {
                            match key {
                                "inventory" | "drop" | "swap_offhand" | "pick" => {
                                    return Err("dialog_screen_blocks_gameplay_key".into());
                                }
                                _ => {}
                            }
                        }
                    }
                    // Movement/held keys never latch while a dialog screen owns the UI.
                    self.keys.remove(key);
                    self.revision += 1;
                    self.camera();
                    return Ok(
                        json!({"phase":"applied","origin":"dialog_screen_key","held_inputs":self.keys,"selected_slot":self.inventory.selected_slot}),
                    );
                }
                if edge && pressed {
                    match key {
                        "ui_cancel" if self.phase == "play" => {
                            self.execute_ui(UiIntent::Key {
                                key: "Escape".into(),
                                pressed: true,
                                modifiers: crate::ui::Modifiers::default(),
                            })?;
                        }
                        "inventory" if self.phase == "play" => {
                            let intent = if self.inventory.screen_open {
                                UiIntent::Close {
                                    context: self
                                        .ui
                                        .as_ref()
                                        .ok_or("ui unavailable")?
                                        .context(&self.inventory),
                                }
                            } else {
                                UiIntent::OpenInventory
                            };
                            self.execute_ui(intent)?;
                        }
                        "drop" if self.phase == "play" => {
                            self.interact(InteractionIntent::Drop { whole_stack: false })?;
                        }
                        "swap_offhand" if self.phase == "play" => {
                            self.interact(InteractionIntent::SwapHands)?;
                        }
                        "pick" if self.phase == "play" => {
                            self.interact(InteractionIntent::Pick {
                                include_data: false,
                            })?;
                        }
                        "perspective" if self.phase == "play" && self.screen() == "game" => {
                            self.cycle_camera();
                        }
                        // F1: the same UI intent the native keyboard path sends.
                        "hide_hud" if self.phase == "play" && self.screen() == "game" => {
                            self.execute_ui(UiIntent::Key {
                                key: "F1".into(),
                                pressed: true,
                                modifiers: crate::ui::Modifiers::default(),
                            })?;
                        }
                        _ => {}
                    }
                }
            }
            InputIntent::Ui { intent } => return self.execute_ui(intent),
            InputIntent::ReloadResources => {
                self.reload_resources()?;
            }
            // The F3 flag lives on the App owner (lib.rs), next to the
            // physical-key path; the session layer rejects it loudly.
            InputIntent::DebugOverlay { .. } => {
                return Err("debug overlay belongs to application owner".into());
            }
        }
        self.revision += 1;
        self.camera();
        Ok(
            json!({"phase":"applied","origin":"ordinary_local_input","held_inputs":self.keys,"selected_slot":self.inventory.selected_slot}),
        )
    }
    /// Searches the target's bounding box for a look direction whose ordinary
    /// raycast resolves to `handle`. The centre is tried first, then inset face and
    /// corner samples, because a villager behind a fence/log/door frame is visible
    /// from some points of its box but not through its centre. `Ok(None)` means the
    /// look now targets the entity (kept when `apply`, restored otherwise);
    /// `Ok(Some(reason))` is a typed refusal and always restores the previous look.
    pub(crate) fn aim_at_entity(&mut self, handle: Handle, apply: bool) -> Result<Option<String>> {
        if self.phase != "play" || !self.player.pose_known {
            return Err("client is not in play".into());
        }
        let bounds = self.entities.entity_bounds(handle, self.client_tick)?;
        let eye = self.player.eye_position();
        let reach = self.player.attribute("minecraft:entity_interaction_range");
        let nearest: [f64; 3] = std::array::from_fn(|a| eye[a].clamp(bounds.min[a], bounds.max[a]));
        let distance = (0..3).map(|a| (nearest[a] - eye[a]).powi(2)).sum::<f64>().sqrt();
        let (yaw, pitch) = (self.player.yaw, self.player.pitch);
        if distance > reach {
            return Ok(Some(format!(
                "target entity is out of interaction reach: {distance:.2} blocks from the eye, limit {reach:.2}; approach first"
            )));
        }
        const XZ: [(f64, f64); 9] = [
            (0.5, 0.5),
            (0.2, 0.5),
            (0.8, 0.5),
            (0.5, 0.2),
            (0.5, 0.8),
            (0.2, 0.2),
            (0.8, 0.2),
            (0.2, 0.8),
            (0.8, 0.8),
        ];
        const Y: [f64; 5] = [0.5, 0.85, 0.15, 0.7, 0.3];
        let at = |fx: f64, fy: f64, fz: f64| -> [f64; 3] {
            [
                bounds.min[0] + (bounds.max[0] - bounds.min[0]) * fx,
                bounds.min[1] + (bounds.max[1] - bounds.min[1]) * fy,
                bounds.min[2] + (bounds.max[2] - bounds.min[2]) * fz,
            ]
        };
        let samples = Y
            .iter()
            .flat_map(|fy| XZ.iter().map(move |(fx, fz)| (*fx, *fy, *fz)))
            .map(|(fx, fy, fz)| at(fx, fy, fz))
            .chain(std::iter::once(nearest));
        for point in samples {
            let delta: [f64; 3] = std::array::from_fn(|a| point[a] - eye[a]);
            let horizontal = delta[0].hypot(delta[2]);
            if horizontal < 1e-9 && delta[1].abs() < 1e-9 {
                continue;
            }
            self.player.look(
                (delta[2].atan2(delta[0]).to_degrees() - 90.) as f32,
                -delta[1].atan2(horizontal).to_degrees() as f32,
            )?;
            let content = self.content.as_ref().ok_or("content unavailable")?;
            let hit = self
                .player
                .raycast(&self.world, content, &self.entities, self.client_tick)?;
            if matches!(hit, Some(Hit::Entity { handle: target, .. }) if target == handle) {
                if !apply {
                    self.player.look(yaw, pitch)?;
                }
                return Ok(None);
            }
        }
        // Diagnose through the centre ray, then restore the caller's look.
        let centre = at(0.5, 0.5, 0.5);
        let delta: [f64; 3] = std::array::from_fn(|a| centre[a] - eye[a]);
        self.player.look(
            (delta[2].atan2(delta[0]).to_degrees() - 90.) as f32,
            -delta[1].atan2(delta[0].hypot(delta[2])).to_degrees() as f32,
        )?;
        let content = self.content.as_ref().ok_or("content unavailable")?;
        let blocker = self
            .player
            .raycast(&self.world, content, &self.entities, self.client_tick)?;
        self.player.look(yaw, pitch)?;
        Ok(Some(match blocker {
            Some(Hit::Block(hit)) => format!(
                "target entity is occluded by {} at {},{},{}; reposition for a clear line",
                content.name(hit.state).unwrap_or("unknown block"),
                hit.position[0],
                hit.position[1],
                hit.position[2]
            ),
            Some(Hit::Entity { handle: other, .. }) => format!(
                "target entity is occluded by entity {}; reposition for a clear line",
                other.network_id
            ),
            None => "ordinary raycast does not target requested entity".into(),
        }))
    }
    pub fn interact(&mut self, intent: InteractionIntent) -> Result<Value> {
        self.interact_checked(None, intent)?
            .ok_or_else(|| "ordinary interaction omitted its result".into())
    }
    /// occluded. None means no action was dispatched, not a stale identity.
    pub fn interact_entity(
        &mut self,
        handle: Handle,
        intent: InteractionIntent,
    ) -> Result<Option<Value>> {
        self.interact_checked(Some(handle), intent)
    }
    fn interact_checked(
        &mut self,
        handle: Option<Handle>,
        intent: InteractionIntent,
    ) -> Result<Option<Value>> {
        if self.phase != "play" {
            return Err("client is not in play".into());
        }
        let content = self.content.as_ref().ok_or("content unavailable")?;
        let context = self.player.interaction_context(
            &self.world,
            content,
            &self.inventory,
            &self.entities,
            self.client_tick,
        )?;
        if let Some(handle) = handle {
            self.entities.resolve(handle)?;
            if !matches!(context.target.as_ref(), Some(Hit::Entity { handle: target, .. }) if *target == handle)
            {
                return Ok(None);
            }
        }
        let attack = matches!(intent, InteractionIntent::Attack);
        let use_item = matches!(intent, InteractionIntent::Use { .. });
        let strength = self.player.attack_strength(0.);
        let start = self.commands.len();
        self.player.execute(
            intent,
            &context,
            &self.world,
            content,
            &self.inventory,
            &self.entities,
            self.client_tick,
            &mut self.commands,
        )?;
        let started = self.commands[start..].iter().any(|command| {
            command.id == sb::ATTACK
                || (command.id == sb::PLAYER_ACTION
                    && matches!(
                        crate::protocol::Reader::new(&command.payload.0).var(),
                        Ok(0 | 2 | 7)
                    ))
        });
        let result = self.player.last_interaction_result();
        let hit = context.target.as_ref().map(|hit| match hit {
            Hit::Block(hit) => hit.point,
            Hit::Entity { point, .. } => *point,
        });
        let value = if attack {
            json!({"started":started,"attack_strength_before":strength})
        } else if use_item {
            let result =
                result.ok_or("ordinary use executor did not produce an interaction result")?;
            let hit = hit.unwrap_or(self.player.eye_position());
            json!({"result":result,"consumes_action":result.starts_with("Success["),"hit_x":hit[0],"hit_y":hit[1],"hit_z":hit[2]})
        } else {
            json!({"phase":"dispatched","origin":"ordinary_player_action"})
        };
        self.consume_player_effects()?;
        self.flush_commands()?;
        self.revision += 1;
        Ok(Some(value))
    }
    pub fn use_item_on(&mut self, position: [i32; 3], face: i32, hand: u8) -> Result<Value> {
        if self.phase != "play" || !self.player.pose_known {
            return Err("client is not in play".into());
        }
        if !(0..=5).contains(&face) || hand > 1 {
            return Err("invalid block face or hand".into());
        }
        if self.inventory.screen_open {
            return Err("gameplay_screen_blocked".into());
        }
        let content = self.content.as_ref().ok_or("content unavailable")?;
        let state = self
            .world
            .block(position)
            .ok_or("client block is not loaded")?;
        let point = position.map(|v| f64::from(v) + 0.5);
        let distance = point
            .iter()
            .zip(self.player.eye_position())
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            .sqrt();
        let hit = BlockHit {
            position,
            face,
            point,
            state,
            distance,
            inside: false,
        };
        let result = self.player.minecraft_use_item_on(
            &hit,
            hand,
            &self.world,
            content,
            &self.inventory,
            &self.entities,
            self.client_tick,
            &mut self.commands,
        )?;
        self.consume_player_effects()?;
        self.flush_commands()?;
        self.revision += 1;
        Ok(
            json!({"result":result,"consumes_action":result.starts_with("Success["),"hit_x":point[0],"hit_y":point[1],"hit_z":point[2]}),
        )
    }
    pub fn execute_ui(&mut self, intent: UiIntent) -> Result<Value> {
        // Title screen runs pre-play: the title UI is installed with real
        // Content at startup, so the full UiState::execute path works once
        // content exists. Only title-safe intents pass (no containers).
        if self
            .ui
            .as_ref()
            .is_some_and(UiState::shell_screen_active)
            && self.content.is_some()
            && is_title_intent(&intent)
        {
            return self.execute_title_ui(intent);
        }
        if self.phase != "play" {
            return Err("client is not in play".into());
        }
        let content = self.content.as_ref().ok_or("content unavailable")?;
        // In particular Source's closed-screen player-menu Swap/QuickMove is not blocked here.
        // Typed container actions share InventoryStore's own context validator and executor.
        if matches!(
            intent,
            UiIntent::ClickSlot { .. }
                | UiIntent::MenuButton { .. }
                | UiIntent::Beacon { .. }
                | UiIntent::SlotToggle { .. }
                | UiIntent::Rename { .. }
                | UiIntent::PlaceRecipe { .. }
                | UiIntent::RecipeBook { .. }
                | UiIntent::CreativeSlot { .. }
        ) {
            let commands = self.inventory.execute(&intent, content)?;
            self.commands.extend(commands);
        } else {
            let effects = self.ui.as_mut().ok_or("ui unavailable")?.execute(
                intent,
                &mut self.inventory,
                content,
            )?;
            if effects.release_input {
                self.keys.clear();
                self.player.release_all(&mut self.commands)?;
            }
            for command in effects.commands {
                self.execute_ui_command(command)?;
            }
        }
        // UI effects apply inventory commands after UiState::execute returns.
        // Publish that resulting screen state without depending on a render frame.
        if let Some(ui) = &mut self.ui {
            ui.synchronize(
                &self.inventory,
                self.content.as_ref().ok_or("content unavailable")?,
            )?;
        }
        if self
            .ui
            .as_ref()
            .is_none_or(|ui| ui.screen() != crate::ui::Screen::Loader)
        {
            self.loader.clear_view(None)?;
        }
        if !self.commands.is_empty() {
            self.flush_commands()?;
        }
        self.consume_player_effects()?;
        self.revision += 1;
        Ok(json!({"phase":"applied","origin":"ordinary_ui_action"}))
    }
    /// MCP seam for the Title screen tools (`minecraft_set_screen_input`,
    /// `minecraft_click_screen_button` pre-play): the windowed playthrough
    /// drives the ordinary Connect path through these. Fail-closed outside
    /// the Title screen via `execute_ui`'s gate.
    pub fn execute_title_ui_for_mcp(&mut self, intent: UiIntent) -> Result<Value> {
        self.execute_title_ui(intent)
    }
    /// Title-screen intent path (pre-play): runs the full `UiState::execute`
    /// with the installed real content. Container intents are rejected; a
    /// `Connect` command bootstraps the session (preserving title prefill on
    /// validation failure), `Shutdown` propagates to the app owner.
    fn execute_title_ui(&mut self, intent: UiIntent) -> Result<Value> {
        if matches!(
            intent,
            UiIntent::ClickSlot { .. }
                | UiIntent::MenuButton { .. }
                | UiIntent::Beacon { .. }
                | UiIntent::SlotToggle { .. }
                | UiIntent::Rename { .. }
                | UiIntent::PlaceRecipe { .. }
                | UiIntent::RecipeBook { .. }
                | UiIntent::CreativeSlot { .. }
        ) {
            return Err("title_screen_no_containers".into());
        }
        let content = self.content.as_ref().ok_or("content unavailable")?;
        let effects = self.ui.as_mut().ok_or("ui unavailable")?.execute(
            intent,
            &mut self.inventory,
            content,
        )?;
        for command in effects.commands {
            self.execute_ui_command(command)?;
        }
        if let Some(ui) = &mut self.ui {
            ui.synchronize(
                &self.inventory,
                self.content.as_ref().ok_or("content unavailable")?,
            )?;
        }
        self.revision += 1;
        Ok(json!({"phase":"applied","origin":"title_ui_action"}))
    }
    pub fn send_chat(&mut self, message: &str, command: bool) -> Result<()> {
        let packet = if command {
            ui_packets::unsigned_command(message)?
        } else {
            let seen = self.ui.as_mut().ok_or("ui unavailable")?.last_seen_update();
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_millis();
            let timestamp = i64::try_from(timestamp).map_err(|_| "chat timestamp overflow")?;
            let mut salt = [0u8; 8];
            getrandom::fill(&mut salt).map_err(|e| e.to_string())?;
            ui_packets::chat(message, timestamp, i64::from_ne_bytes(salt), seen)?
        };
        self.send(packet.id, packet.payload)
    }
    pub fn respawn(&mut self) -> Result<()> {
        // Fail-closed network guard: only a dead play-phase player may send the
        // ClientCommand respawn (packet 12, action 0). UiState already requires
        // Death screen + death message + can_respawn before this is reachable.
        if self.phase != "play" {
            return Err("player is not dead".into());
        }
        if !self.player.health.is_some_and(|health| health <= 0.) {
            return Err("player is not dead".into());
        }
        if self
            .ui
            .as_ref()
            .is_some_and(|ui| !ui.dialog_screen_active())
        {
            return Err("respawn_requires_death_screen".into());
        }
        let packet = ui_packets::respawn();
        self.send(packet.id, packet.payload)?;
        self.event(json!({"type":"ordinary_respawn_dispatched","packet_id":packet.id,"origin":"ordinary_ui_action"}));
        Ok(())
    }
    pub(super) fn consume_ui_packet_commands(&mut self) -> Result<()> {
        while let Some(command) = self.ui.as_mut().and_then(UiState::take_packet_command) {
            self.execute_ui_command(command)?;
        }
        if !self.commands.is_empty() {
            self.flush_commands()?;
        }
        Ok(())
    }
    fn execute_ui_command(&mut self, command: UiCommand) -> Result<()> {
        match command {
            UiCommand::Loader(message) => self.loader_action(message)?,
            UiCommand::DialogClick { id, payload } => {
                if self.phase != "play" {
                    return Err("dialog_click_requires_play".into());
                }
                self.commands
                    .push(ui_packets::custom_click(&id, payload.as_ref())?);
                self.event(json!({"type":"dialog_click_sent","id":id,"payload":payload.as_ref().map(crate::protocol::nbt::Nbt::to_json),"origin":"ordinary_ui_action"}));
            }
            UiCommand::Inventory(intent) => self.commands.extend(
                self.inventory
                    .execute(&intent, self.content.as_ref().ok_or("content unavailable")?)?,
            ),
            UiCommand::InventoryGesture(intents) => {
                self.commands.extend(self.inventory.execute_gesture(
                    &intents,
                    self.content.as_ref().ok_or("content unavailable")?,
                )?)
            }
            UiCommand::CreativePick {
                context,
                stack,
                button,
                mode,
            } => self.commands.extend(self.inventory.execute_creative_pick(
                &context,
                &stack,
                button,
                mode,
                self.content.as_ref().ok_or("content unavailable")?,
            )?),
            UiCommand::CreativeClear { context, all } => {
                self.commands.extend(self.inventory.execute_creative_clear(
                    &context,
                    all,
                    self.content.as_ref().ok_or("content unavailable")?,
                )?)
            }
            UiCommand::CreativeSetStack {
                context,
                slot,
                stack,
            } => self.commands.extend(self.inventory.execute_creative_stack(
                &context,
                slot,
                stack,
                self.content.as_ref().ok_or("content unavailable")?,
            )?),
            UiCommand::InventoryRename {
                context,
                text,
                hover_name,
            } => self.commands.extend(self.inventory.execute_rename(
                &context,
                &text,
                &hover_name,
                self.content.as_ref().ok_or("content unavailable")?,
            )?),
            UiCommand::ChatAck { offset } => self.commands.push(ui_packets::chat_ack(offset)),
            UiCommand::CommandSuggestion { id, command } => {
                if self.phase != "play" { return Err("command_suggestion_requires_play".into()); }
                self.commands.push(ui_packets::command_suggestion(id, &command)?);
            }
            UiCommand::Chat { message, command } => self.send_chat(&message, command)?,
            UiCommand::Sign { pos, front, lines } => {
                self.commands.push(ui_packets::sign(pos, front, &lines)?);
                self.entities.predict_sign_text(pos, front, &lines)?;
                // A sign editor is not a container; closing it restores gameplay
                // without sending a container-close packet.
                self.inventory.close();
            }
            UiCommand::Book { slot, pages, title } => {
                self.commands
                    .push(ui_packets::book(slot, &pages, title.as_deref())?)
            }
            UiCommand::Command(edit) => self.commands.push(ui_packets::command_block(&edit)?),
            UiCommand::Structure(edit) => self.commands.push(ui_packets::structure(&edit)?),
            UiCommand::Jigsaw(edit) => self.commands.push(ui_packets::jigsaw(&edit)?),
            UiCommand::JigsawGenerate { pos, levels, keep } => self
                .commands
                .push(ui_packets::generate_jigsaw(pos, levels, keep)),
            UiCommand::Respawn => self.respawn()?,
            UiCommand::RequestStatistics => {
                if self.phase != "play" {
                    return Err("statistics_request_requires_play".into());
                }
                if !self
                    .ui
                    .as_ref()
                    .is_some_and(|ui| ui.screen() == crate::ui::Screen::Statistics)
                {
                    return Err("statistics_request_requires_screen".into());
                }
                self.commands.push(ui_packets::request_statistics());
            }
            UiCommand::SeenAdvancements { tab } => {
                if self.phase != "play" {
                    return Err("advancements_seen_requires_play".into());
                }
                if let Some(root) = &tab {
                    let ui = self.ui.as_ref().ok_or("ui unavailable")?;
                    if ui.screen() != crate::ui::Screen::Advancements
                        || ui.selected_advancement_root() != Some(root.as_str())
                    {
                        return Err("advancements_seen_requires_selected_tab".into());
                    }
                }
                self.commands
                    .push(ui_packets::seen_advancements(tab.as_deref()));
            }
            UiCommand::Disconnect => self.disconnect(),
            UiCommand::ReloadResources => self.reload_resources()?,
            UiCommand::Connect { address, username } => {
                self.connect(&address, &username)?;
            }
            UiCommand::Shutdown => self.shutdown_requested = true,
            UiCommand::PlayWorld { dir_name } => self.start_local_world(&dir_name)?,
            UiCommand::CancelLocalStart => self.cancel_local_start(),
            UiCommand::OpenUrl(_) => {
                return Err("external URL opening requires application approval".into());
            }
            UiCommand::Window(crate::ui::WindowEffect::UiScale(_)) => {
                if let Some((width, height, scale)) = self.ui_viewport
                    && let Some(ui) = &mut self.ui
                {
                    ui.resize(width, height, scale);
                }
            }
            UiCommand::Window(_) => {
                return Err("window operation requires native application owner".into());
            }
            UiCommand::Settings { key, value } => {
                self.apply_client_setting(&key, &value)?;
                self.event(json!({"type":"local_ui_setting","key":key,"value":value,"origin":"ordinary_ui_action"}));
            }
        }
        Ok(())
    }
    /// Apply a client-local Options value. Sanitizes into range, stores it on
    /// the session (mirroring `Scene::fov_deg` for the renderer), and forwards
    /// Master gain to the audio worker. Session-only keys (saved_hotbars,
    /// toggles, …) are accepted and stored nowhere. `render_distance` also
    /// publishes to the network worker and resends Play ClientInformation.
    pub fn apply_client_setting(&mut self, key: &str, value: &Value) -> Result<()> {
        let Some(clean) = UiState::sanitize_setting_value(key, value) else {
            return Ok(());
        };
        match key {
            "key_bindings" => {
                self.bindings = crate::controls::Bindings::from_value(&clean)
                    .ok_or("invalid key bindings")?;
            }
            "fullscreen" | "vsync" | "max_fps" | "graphics_preset" | "ui_scale" => {
                self.presentation_settings.insert(key.into(), clean.clone());
            }
            "render_distance" => {
                self.render_distance = clean
                    .as_u64()
                    .ok_or("client setting render_distance lost its number")?
                    as u32;
                self.world.set_view_distance(self.render_distance);
                self.scene.render_distance = self.render_distance;
                self.push_render_distance();
            }
            "fov" => {
                self.fov_deg = clean.as_f64().ok_or("client setting fov lost its number")? as f32;
                self.scene.fov_deg = self.fov_deg;
            }
            "mouse_sensitivity" => {
                self.mouse_sensitivity = clean
                    .as_f64()
                    .ok_or("client setting mouse_sensitivity lost its number")?
                    as f32;
            }
            "damage_tilt_strength" => {
                self.damage_tilt_strength = clean
                    .as_f64()
                    .ok_or("client setting damage_tilt_strength lost its number")?;
                self.camera();
            }
            "volume" => {
                self.volume = clean
                    .as_f64()
                    .ok_or("client setting volume lost its number")?
                    as f32;
                if let Some(audio) = &self.audio {
                    audio.set_volume(crate::audio::SoundCategory::Master, self.volume)?;
                }
            }
            _ => {}
        }
        if let Some(ui) = &mut self.ui {
            ui.apply_setting(key, clean)?;
        }
        crate::local_config::save_client_settings(&self.client_settings_snapshot());
        Ok(())
    }

    /// Publish the setting; the I/O owner alone chooses its wire-phase encoding.
    fn push_render_distance(&self) {
        let Some(network) = self.network.as_ref() else {
            return;
        };
        let radius = self
            .render_distance
            .clamp(UiState::MIN_RENDER_DISTANCE, UiState::MAX_RENDER_DISTANCE)
            as u8;
        network.set_view_distance(radius);
    }
    /// Sanitized managed settings for persistence.
    pub fn client_settings_snapshot(&self) -> BTreeMap<String, Value> {
        let mut out = BTreeMap::new();
        out.insert("render_distance".into(), json!(self.render_distance));
        out.insert("fov".into(), json!(self.fov_deg));
        out.insert("mouse_sensitivity".into(), json!(self.mouse_sensitivity));
        out.insert(
            "damage_tilt_strength".into(),
            json!(self.damage_tilt_strength),
        );
        out.insert("volume".into(), json!(self.volume));
        out.insert("key_bindings".into(), self.bindings.value());
        out.extend(self.presentation_settings.iter().map(|(key, value)| (key.clone(), value.clone())));
        if let Some(ui) = &self.ui {
            if let Some(scale) = ui
                .setting("ui_scale")
                .and_then(|v| UiState::sanitize_setting_value("ui_scale", v))
            {
                out.insert("ui_scale".into(), scale);
            }
        }
        out
    }
    /// Load persisted (or default) managed settings into session + UI state,
    /// applying volume to the audio worker. Called once per UI install and by
    /// the native owner at startup.
    pub fn apply_persisted_settings(&mut self, settings: BTreeMap<String, Value>) {
        for key in UiState::MANAGED_SETTING_KEYS {
            // Persisted value wins when valid; anything missing or corrupt falls
            // back to the default (never retains a stale in-memory value).
            let raw = settings
                .get(*key)
                .and_then(|v| UiState::sanitize_setting_value(key, v))
                .or_else(|| UiState::default_setting_value(key));
            let Some(clean) = raw else { continue };
            if let Some(ui) = &mut self.ui {
                let _ = ui.apply_setting(key, clean.clone());
            }
            match *key {
                "key_bindings" => {
                    if let Some(bindings) = crate::controls::Bindings::from_value(&clean) {
                        self.bindings = bindings;
                    }
                }
                "fullscreen" | "vsync" | "max_fps" | "graphics_preset" | "ui_scale" => {
                    self.presentation_settings.insert((*key).into(), clean.clone());
                }
                "render_distance" => {
                    if let Some(n) = clean.as_u64() {
                        self.render_distance = n as u32;
                        self.world.set_view_distance(self.render_distance);
                        self.scene.render_distance = self.render_distance;
                    }
                }
                "fov" => {
                    if let Some(f) = clean.as_f64() {
                        self.fov_deg = f as f32;
                        self.scene.fov_deg = self.fov_deg;
                    }
                }
                "mouse_sensitivity" => {
                    if let Some(f) = clean.as_f64() {
                        self.mouse_sensitivity = f as f32;
                    }
                }
                "damage_tilt_strength" => {
                    if let Some(f) = clean.as_f64() {
                        self.damage_tilt_strength = f;
                    }
                }
                "volume" => {
                    if let Some(f) = clean.as_f64() {
                        self.volume = f as f32;
                        if let Some(audio) = &self.audio {
                            let _ =
                                audio.set_volume(crate::audio::SoundCategory::Master, self.volume);
                        }
                    }
                }
                _ => {}
            }
        }
        // Far terrain distance in blocks (`far_terrain`, 0 = off); the Low
        // graphics preset defaults to a shorter reach for weak GPUs.
        // SOLARIS_CLIENT_FAR_TERRAIN overrides both for scripted runs.
        let low = self
            .presentation_settings
            .get("graphics_preset")
            .and_then(Value::as_str)
            .is_some_and(|preset| preset.eq_ignore_ascii_case("low"));
        let distance = std::env::var("SOLARIS_CLIENT_FAR_TERRAIN")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .or_else(|| {
                settings
                    .get("far_terrain")
                    .and_then(Value::as_u64)
                    .map(|v| v.min(u64::from(crate::far::MAX_DISTANCE)) as u32)
            })
            .unwrap_or(if low {
                crate::far::LOW_PRESET_DISTANCE
            } else {
                crate::far::DEFAULT_DISTANCE
            });
        self.far.set_distance(distance);
        self.camera();
    }
    pub(super) fn consume_player_effects(&mut self) -> Result<()> {
        if let Some(content) = self.content.as_ref() {
            let result = self.player.apply_pending_use_release(
                &self.world, content, &self.inventory, &self.entities, self.client_tick,
            );
            if let Err(error) = result {
                let item_id = self.inventory.player_slot(36 + u16::from(self.inventory.selected_slot))
                    .ok().flatten().map(|stack| stack.id);
                self.player.warn_local_prediction(item_id, "item use release", &error);
            }
        }
        while let Some(prediction) = self.player.take_use_prediction() {
            if let Err(error) = self.apply_local_prediction(prediction) {
                let item_id = self.inventory.player_slot(36 + u16::from(self.inventory.selected_slot))
                    .ok().flatten().map(|stack| stack.id);
                self.player.warn_local_prediction(item_id, "local effect application", &error);
            }
        }
        while let Some(trigger) = self.player.take_ui_trigger() {
            self.open_trigger(trigger)?;
        }
        Ok(())
    }
    fn apply_local_prediction(&mut self, prediction: UsePrediction) -> Result<()> {
            match prediction {
                UsePrediction::Sound(request) => { if let Some(audio) = &self.audio { audio.play(request)?; } }
                UsePrediction::WorldBlock {position,state,sequence,..} => {
                    let content = self.content.as_ref().ok_or("content unavailable")?;
                    let received = self.received_block(position).ok_or("predicted block is unloaded")?;
                    if self.predicted_blocks.len() >= 4096 && !self.predicted_blocks.contains_key(&position) { return Err("block prediction bound".into()); }
                    let prediction = self.predicted_blocks.entry(position).and_modify(|block| block.sequence = sequence)
                        .or_insert(PredictedBlock {received,sequence,block_entity:None});
                    let old = self.world.block(position);
                    if let Some(previous) = old
                        && content.info(previous)?.block != content.info(state)?.block
                    {
                        let entity = self.entities.take_block_entity_snapshot(position);
                        if prediction.block_entity.is_none()
                            && content.info(previous)?.block == content.info(prediction.received)?.block
                        {
                            prediction.block_entity = entity;
                        }
                    }
                    self.world.set(position,state,|id| content.block_is_air(id))?;
                    self.entities.ensure_block_entity(position,state,content,self.client_tick)?;
                    self.entities.block_changed(position,state);
                    let now = (self.client_tick as f64 + f64::from(self.presentation_partial)) * 0.05;
                    if let Some(terrain) = &mut self.terrain {
                        terrain.invalidate_block(&self.world,position)?;
                        if old != Some(state) {
                            self.block_motion.changed(terrain,&self.world,content,self.scene.camera_position,now,position,old,state)?;
                        }
                    }
                }
                UsePrediction::PlayerSlot {slot,stack,replacement} => self.inventory.predict_use_slot(slot,stack,replacement)?,
                UsePrediction::ExtraItem {stack,drop_remainder} => {
                    let (changes,left) = self.inventory.source_add_item(&stack)?;
                    for (slot,stack,replacement) in changes { self.inventory.predict_use_slot(slot,Some(stack),replacement)?; }
                    if drop_remainder && !left.empty() { self.event(json!({"type":"local_extra_item_remainder","stack":left,"origin":"locally_predicted","server_entity_confirmation_required":true})); }
                }
                UsePrediction::EntityEquipment {handle,slot,stack} => self.entities.predict_equipment(handle,slot,stack)?,
                UsePrediction::EntityUseState {handle,state} => self.entities.apply_use_state(handle,state)?,
                UsePrediction::EntityLeash {handle,holder} => self.entities.predict_leash(handle,holder)?,
                UsePrediction::EntityDiscard {handle} => self.entities.predict_discard(handle)?,
                UsePrediction::BlockEntityComponents {position,components,patch} => {
                    let state = self.world.block(position).ok_or("block component target unloaded")?;
                    let content = self.content.as_ref().ok_or("content unavailable")?;
                    let kind = content.placement_metadata(state)?["block_entity_type"].as_str().ok_or("Source block entity type missing")?;
                    self.entities.predict_block_entity_components(position,components,patch,kind)?;
                }
                UsePrediction::ComparatorOutput {position,signal} => self.entities.predict_comparator_output(position,signal)?,
                UsePrediction::EntitySwing {handle,hand} => self.entities.predict_swing(handle,hand,self.client_tick)?,
                UsePrediction::EntityEffect {handle,effect,duration,amplifier,flags,tick} => self.entities.predict_effect(handle,effect,duration,amplifier,flags,tick)?,
                UsePrediction::EntityMetadata {handle,key,value} => self.entities.predict_metadata(handle,key,value)?,
                UsePrediction::Particles {count,..} => self.event(json!({"type":"unsupported_local_particle_presentation","count":count,"origin":"locally_predicted","rendered":false})),
                UsePrediction::LevelEvent {event,position,data} => {
                    let presented = self.present_block_break(event, position, data)?;
                    self.event(json!({"type":"local_level_event","event":event,"position":position,"data":data,"origin":"locally_predicted","presentation_supported":presented}));
                }
            }
        Ok(())
    }
    fn open_trigger(&mut self, trigger: UiTrigger) -> Result<()> {
        let (item_id, owner) = match &trigger {
            UiTrigger::Book { hand } | UiTrigger::ItemUsed { hand } => {
                let slot = if *hand == 1 { 45 } else { 36 + u16::from(self.inventory.selected_slot) };
                (self.inventory.player_slot(slot).ok().flatten().map(|stack| stack.id), "item UI")
            }
            UiTrigger::Sign { .. } => (None, "Sign editor"),
            UiTrigger::Command { .. } => (None, "Command editor"),
            UiTrigger::MinecartCommand { .. } => (None, "MinecartCommand editor"),
            UiTrigger::Structure { .. } => (None, "Structure editor"),
            UiTrigger::Jigsaw { .. } => (None, "Jigsaw editor"),
            UiTrigger::TestBlock { .. } => (None, "TestBlock editor"),
            UiTrigger::TestInstanceBlock { .. } => (None, "TestInstanceBlock editor"),
        };
        if let Err(error) = self.open_trigger_local(trigger) {
            self.player.warn_local_prediction(item_id, owner, &error);
        }
        if !self.commands.is_empty() {
            self.flush_commands()?;
        }
        Ok(())
    }
    fn open_trigger_local(&mut self, trigger: UiTrigger) -> Result<()> {
        match trigger {
            UiTrigger::Book { hand } => {
                let (menu_slot, inventory_slot) = if hand == 1 {
                    (45, 40)
                } else {
                    let selected = i32::from(self.inventory.selected_slot);
                    (36 + selected, selected)
                };
                let stack = self
                    .inventory
                    .player_slot(menu_slot as u16)?
                    .ok_or("book stack unreceived")?;
                self.ui
                    .as_mut()
                    .ok_or("ui unavailable")?
                    .open_book(stack, inventory_slot)?;
                self.keys.clear();
                self.player.release_all(&mut self.commands)?;
            }
            UiTrigger::Sign { position, front } => {
                self.open_sign_editor(position, front)?;
            }
            UiTrigger::ItemUsed { hand } => self.event(
                json!({"type":"ordinary_item_used","hand":hand,"origin":"locally_predicted"}),
            ),
            UiTrigger::Command { .. }
            | UiTrigger::MinecartCommand { .. }
            | UiTrigger::Structure { .. }
            | UiTrigger::Jigsaw { .. }
            | UiTrigger::TestBlock { .. }
            | UiTrigger::TestInstanceBlock { .. } => {
                return Err("operator editor activation is not supported by session".into());
            }
        }
        Ok(())
    }
    pub(super) fn open_sign_editor(&mut self, position: [i32; 3], front: bool) -> Result<bool> {
        let Some(state) = self.world.block(position) else {
            return Ok(false);
        };
        let content = self.content.as_ref().ok_or("content unavailable")?;
        if !self
            .entities
            .ensure_sign_block_entity(position, state, content, self.client_tick)?
        {
            return Ok(false);
        }
        let data = self
            .entities
            .block_entity_data(position)
            .ok_or("sign data unavailable")?;
        let messages = &data[if front { "front_text" } else { "back_text" }]["messages"];
        let language = self.ui.as_ref().ok_or("ui unavailable")?.text_renderer();
        // SignText's codec requires exactly four decodable components.
        // SignBlockEntity.loadAdditional defaults the entire failed side.
        let lines = messages
            .as_array()
            .filter(|messages| messages.len() == 4)
            .and_then(|messages| {
                let mut lines: [String; 4] = Default::default();
                for (line, message) in lines.iter_mut().zip(messages) {
                    *line = language.language.plain(message).ok()?;
                }
                Some(lines)
            })
            .unwrap_or_default();
        self.ui.as_mut().ok_or("ui unavailable")?.open_editor(
            EditorState::Sign {
                pos: position,
                front,
                lines,
            },
            false,
        )?;
        self.keys.clear();
        self.player.release_all(&mut self.commands)?;
        Ok(true)
    }
    fn reload_resources(&mut self) -> Result<()> {
        if !self.presentation {
            return Err("resource reload unavailable in state-only client".into());
        }
        let content = self
            .content
            .as_ref()
            .ok_or("content unavailable")?
            .stage_current_reload()?;
        let models = Arc::new(crate::entities::Models::load(&content)?);
        self.ui
            .as_mut()
            .ok_or("ui unavailable")?
            .reset_resources(&content)?;
        self.entities.activate_models(models.clone());
        self.entities.configure_interaction_tags(&content)?;
        self.player.clear_visual_resources();
        self.player.set_visual_models(Arc::clone(&models));
        self.ui.as_mut().unwrap().set_entity_models(models);
        self.entities
            .set_text_renderer(self.ui.as_ref().unwrap().text_renderer());
        self.resource_epoch += 1;
        self.install_atlas(&content);
        self.content = Some(content);
        self.block_effects.clear();
        self.torch_particles.clear();
        self.block_motion.clear();
        if self.dimension.is_some() {
            let terrain = self.terrain.as_mut().ok_or("terrain unavailable")?;
            terrain.reset(&mut self.scene.chunks);
            terrain.activate(
                self.content.as_ref().unwrap(),
                [
                    self.connection_epoch,
                    self.dimension_epoch,
                    self.resource_epoch,
                ],
            )?;
            for (position, _) in self.world.chunks() {
                terrain.invalidate_chunk(&self.world, *position)?;
            }
        }
        if let Some(audio) = &self.audio {
            audio.reload()?;
        }
        self.changed_state();
        Ok(())
    }
}
fn normalize_key(key: &str) -> Option<&'static str> {
    match key.strip_prefix("key.keyboard.").unwrap_or(key) {
        "w" | "forward" => Some("forward"),
        "s" | "backward" | "back" => Some("backward"),
        "a" | "left" => Some("left"),
        "d" | "right" => Some("right"),
        "space" | "jump" => Some("jump"),
        "left.shift" | "right.shift" | "shift" | "sneak" => Some("sneak"),
        "left.control" | "right.control" | "control" | "sprint" => Some("sprint"),
        "attack" | "mouse_left" => Some("attack"),
        "use" | "mouse_right" => Some("use"),
        "drop" | "q" => Some("drop"),
        "inventory" | "e" => Some("inventory"),
        "swap_offhand" | "f" => Some("swap_offhand"),
        "f5" | "perspective" => Some("perspective"),
        "f1" | "hide_hud" => Some("hide_hud"),
        "pick" | "mouse_middle" => Some("pick"),
        "player_list" => Some("player_list"),
        // Dialog-screen UI keys (death/pause overlays): cancel/confirm/focus-next.
        "escape" | "ui_cancel" => Some("ui_cancel"),
        "enter" | "ui_confirm" => Some("ui_confirm"),
        "tab" | "ui_focus_next" => Some("ui_focus_next"),
        _ => None,
    }
}
/// Intents the Title screen may run pre-play: field focus/typing, pointer,
/// key handling, and node activation. Container/chat/editor intents stay
/// play-gated (`title_screen_no_containers` / `client is not in play`).
fn is_title_intent(intent: &UiIntent) -> bool {
    matches!(
        intent,
        UiIntent::Activate { .. }
            | UiIntent::Focus { .. }
            | UiIntent::Pointer { .. }
            | UiIntent::Scroll { .. }
            | UiIntent::Key { .. }
            | UiIntent::Text { .. }
            | UiIntent::Composition { .. }
            | UiIntent::SelectText { .. }
            | UiIntent::ScrollTarget { .. }
            | UiIntent::Close { .. }
    )
}
#[cfg(test)]
mod client_settings_tests {
    use super::*;
    #[test]
    fn disconnects_local_ui_open_error_is_not_session_failure() {
        let mut session = Session::new_mode("local-ui-warning".into(), false);
        session.open_trigger(UiTrigger::Book { hand: 0 }).unwrap();
        assert!(session.error.is_none());
    }
    #[test]
    fn stores_fov_and_render_distance() {
        // State-only session: no audio worker, so volume applies without IO.
        let mut session = Session::new_mode("settings-apply-test".into(), false);
        assert_eq!(session.render_distance(), UiState::DEFAULT_RENDER_DISTANCE);
        assert_eq!(session.fov_deg(), UiState::DEFAULT_FOV_DEG);
        session.apply_client_setting("fov", &json!(90.)).unwrap();
        assert_eq!(session.fov_deg(), 90.);
        assert_eq!(session.scene().fov_deg, 90.);
        session
            .apply_client_setting("render_distance", &json!(12))
            .unwrap();
        assert_eq!(session.render_distance(), 12);
        session
            .apply_client_setting("mouse_sensitivity", &json!(2.))
            .unwrap();
        assert_eq!(session.mouse_sensitivity(), 2.);
        session.apply_client_setting("volume", &json!(0.5)).unwrap();
        assert_eq!(session.volume(), 0.5);
        // Out-of-range values clamp instead of erroring.
        session.apply_client_setting("fov", &json!(500.)).unwrap();
        assert_eq!(session.fov_deg(), UiState::MAX_FOV_DEG);
        session.apply_client_setting("volume", &json!(9.)).unwrap();
        assert_eq!(session.volume(), UiState::MAX_VOLUME);
        // Unknown/session-only keys are accepted and stored nowhere.
        session
            .apply_client_setting("hide_hud", &json!(true))
            .unwrap();
        session
            .apply_client_setting("saved_hotbars", &json!([]))
            .unwrap();
        assert_eq!(session.fov_deg(), UiState::MAX_FOV_DEG);
    }
    #[test]
    fn persisted_settings_load_with_defaults() {
        let mut session = Session::new_mode("settings-load-test".into(), false);
        let mut map = BTreeMap::new();
        map.insert("fov".into(), json!(100.));
        map.insert("bogus".into(), json!(1));
        session.apply_persisted_settings(map);
        assert_eq!(session.fov_deg(), 100.);
        // Unmentioned keys fall back to defaults.
        assert_eq!(session.render_distance(), UiState::DEFAULT_RENDER_DISTANCE);
        assert_eq!(session.volume(), UiState::DEFAULT_VOLUME);
        assert_eq!(
            session.mouse_sensitivity(),
            UiState::DEFAULT_MOUSE_SENSITIVITY
        );
        assert_eq!(session.scene().fov_deg, 100.);
        // Corrupt values fall back to defaults, not errors.
        let mut bad = BTreeMap::new();
        bad.insert("fov".into(), json!("wide"));
        session.apply_persisted_settings(bad);
        assert_eq!(session.fov_deg(), UiState::DEFAULT_FOV_DEG);
    }
    #[test]
    fn p1_sign_submit_clears_screen_admission_and_queues_update_only() {
        let mut session = Session::new_mode("sign-submit-test".into(), false);
        let position = [-1, 3, -1];
        session.inventory.sign_editor = Some((position, false));
        session.inventory.screen_open = true;
        let lines = [
            "P1 sign".into(),
            String::new(),
            String::new(),
            String::new(),
        ];
        session
            .execute_ui_command(UiCommand::Sign {
                pos: position,
                front: false,
                lines: lines.clone(),
            })
            .unwrap();
        assert!(!session.inventory.screen_open);
        assert_eq!(session.inventory.sign_editor, None);
        assert_eq!(session.commands.len(), 1);
        let packet = &session.commands[0];
        assert_eq!(packet.id, 62);
        let mut reader = crate::protocol::Reader::new(&packet.payload.0);
        assert_eq!(crate::protocol::unpack_pos(reader.i64().unwrap()), position);
        for line in lines {
            assert_eq!(reader.string(384).unwrap(), line);
        }
        assert_eq!(reader.var().unwrap(), 0);
        reader.done().unwrap();
    }

    fn p1_review_sign_session(data: serde_json::Value) -> Session {
        use crate::protocol::world::{Chunk, Palette, Section, SurfaceHeightmap, World};
        let mut session = Session::new_mode("review-sign".into(), false);
        let mut content = crate::protocol::content::Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        content
            .block_entity_names
            .insert(0, "minecraft:sign".into());
        content.insert_test_placement_metadata(0, json!({"block_entity_type":"minecraft:sign"}));
        session.ui = Some(UiState::new_state(&content).unwrap());
        session.world = World::new(0, 16);
        let uniform = |size| Palette {
            bits: 0,
            values: vec![0],
            words: Vec::new(),
            size,
        };
        session
            .world
            .insert(
                [0, 0],
                Chunk {
                    sections: vec![Arc::new(Section {
                        blocks: uniform(4096),
                        biomes: uniform(64),
                        non_empty_block_count: 0,
                    })],
                    sky: BTreeMap::new(),
                    block: BTreeMap::new(),
                    block_entities: crate::protocol::world::empty_block_entities(),
                    surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                    surface_heightmap_received: false,
                },
            )
            .unwrap();
        session
            .entities
            .apply_block_entity(
                [1, 2, 3],
                0,
                Arc::new(data),
                Arc::from([10, 0]),
                Some(0),
                &content,
                0,
            )
            .unwrap();
        session.content = Some(content);
        session.inventory.sign_editor = Some(([1, 2, 3], true));
        session.inventory.screen_open = true;
        session
    }

    #[test]
    fn p1_review_escape_and_done_submit_current_lines_and_predict_selected_side() {
        use crate::ui::Modifiers;
        let received = json!({
            "front_text":{"messages":["old","","",""],"color":"red","has_glowing_text":true},
            "back_text":{"messages":["back","two","three","four"],"color":"blue","has_glowing_text":false},
            "is_waxed":true
        });
        for front in [false, true] {
            for escape in [false, true] {
                let mut session = p1_review_sign_session(received.clone());
                let mut open = crate::protocol::Writer::default();
                open.i64(crate::protocol::pack_pos([1, 2, 3]));
                open.var(i32::from(front));
                session
                    .apply(crate::session::Phase::Play, 61, &open.0)
                    .unwrap();
                let ui = session.ui.as_mut().unwrap();
                ui.synchronize(&session.inventory, session.content.as_ref().unwrap())
                    .unwrap();
                let lines = ["new one", "new two", "new three", "new four"].map(String::from);
                for (line, text) in lines.iter().enumerate() {
                    ui.execute(
                        UiIntent::EditSignLine {
                            context: ui.context(&session.inventory),
                            line: line as u8,
                            text: text.clone(),
                        },
                        &mut session.inventory,
                        session.content.as_ref().unwrap(),
                    )
                    .unwrap();
                }
                let intent = if escape {
                    UiIntent::Key {
                        key: "Escape".into(),
                        pressed: true,
                        modifiers: Modifiers::default(),
                    }
                } else {
                    UiIntent::SubmitSign {
                        context: ui.context(&session.inventory),
                    }
                };
                let effects = ui
                    .execute(
                        intent,
                        &mut session.inventory,
                        session.content.as_ref().unwrap(),
                    )
                    .unwrap();
                assert!(
                    matches!(effects.commands.as_slice(), [UiCommand::Sign { lines: actual, .. }] if actual == &lines)
                );
                for command in effects.commands {
                    session.execute_ui_command(command).unwrap();
                }
                assert_eq!(session.commands.len(), 1);
                let packet = &session.commands[0];
                assert_eq!(packet.id, 62);
                let mut reader = crate::protocol::Reader::new(&packet.payload.0);
                assert_eq!(
                    crate::protocol::unpack_pos(reader.i64().unwrap()),
                    [1, 2, 3]
                );
                for line in &lines {
                    assert_eq!(reader.string(384).unwrap(), *line);
                }
                assert_eq!(reader.var().unwrap(), i32::from(front));
                reader.done().unwrap();
                assert!(!session.inventory.screen_open);
                let side = if front { "front_text" } else { "back_text" };
                let other = if front { "back_text" } else { "front_text" };
                let predicted = session.entities.block_entity_data([1, 2, 3]).unwrap();
                assert_eq!(predicted[side]["messages"], json!(lines));
                assert_eq!(predicted[side]["color"], received[side]["color"]);
                assert_eq!(
                    predicted[side]["has_glowing_text"],
                    received[side]["has_glowing_text"]
                );
                assert_eq!(predicted[other], received[other]);
                assert_eq!(predicted["is_waxed"], received["is_waxed"]);
                assert_eq!(
                    session.entities.block_entity_nbt([1, 2, 3]).unwrap(),
                    &[10, 0]
                );
                let server = json!({"front_text":{"messages":["server","","",""]},"back_text":{"messages":["echo","","",""]}});
                session
                    .entities
                    .apply_block_entity(
                        [1, 2, 3],
                        0,
                        Arc::new(server.clone()),
                        Arc::from([10, 1]),
                        Some(0),
                        session.content.as_ref().unwrap(),
                        1,
                    )
                    .unwrap();
                assert_eq!(
                    session.entities.block_entity_data([1, 2, 3]).unwrap(),
                    &server
                );
            }
        }
    }

    #[test]
    fn p1_review_sign_editor_invalid_sides_default_and_valid_sides_preserve_lines() {
        for (data, expected) in [
            (json!({}), ["", "", "", ""]),
            (json!({"front_text":{}}), ["", "", "", ""]),
            (
                json!({"front_text":{"messages":["observed", {"text":"second"}]}}),
                ["", "", "", ""],
            ),
            (
                json!({"front_text":{"messages":["observed", null, "", ""]}}),
                ["", "", "", ""],
            ),
            (
                json!({"front_text":{"messages":["observed", {"text":"second"}, "", "fourth"]}}),
                ["observed", "second", "", "fourth"],
            ),
        ] {
            let mut session = p1_review_sign_session(data);
            for front in [false, true] {
                assert!(session.open_sign_editor([1, 2, 3], front).unwrap());
                for line in 0..4 {
                    assert_eq!(
                        session
                            .ui
                            .as_ref()
                            .unwrap()
                            .title_field(&format!("sign/line/{line}")),
                        Some(if front { expected[line] } else { "" }.into())
                    );
                }
            }
        }
    }
    #[test]
    fn p1_hurt_tilt_setting_loads_clamps_scales_and_defaults() {
        let mut session = Session::new_mode("hurt-settings".into(), false);
        session.player.hurt_tick = Some(100);
        session.client_tick = 102;
        session.apply_persisted_settings(BTreeMap::new());
        assert_eq!(session.damage_tilt_strength, 1.);
        let full = session.scene.hurt_tilt_deg;
        session.apply_persisted_settings(BTreeMap::from([(
            "damage_tilt_strength".into(),
            json!(0.5),
        )]));
        assert_eq!(session.scene.hurt_tilt_deg, full * 0.5);
        assert_eq!(
            session.client_settings_snapshot()["damage_tilt_strength"],
            json!(0.5)
        );
        session.apply_persisted_settings(BTreeMap::from([(
            "damage_tilt_strength".into(),
            json!(-1.),
        )]));
        assert_eq!(session.scene.hurt_tilt_deg, 0.);
        session
            .apply_persisted_settings(BTreeMap::from([("damage_tilt_strength".into(), json!(9.))]));
        assert_eq!(session.scene.hurt_tilt_deg, full);
        session.apply_persisted_settings(BTreeMap::from([(
            "damage_tilt_strength".into(),
            json!("bad"),
        )]));
        assert_eq!(session.damage_tilt_strength, 1.);
        assert_eq!(
            UiState::default_setting_value("damage_tilt_strength"),
            Some(json!(1.))
        );
        assert_eq!(
            UiState::next_setting_value("damage_tilt_strength", Some(&json!(1.))),
            Some(json!(0.))
        );
        assert_eq!(
            UiState::next_setting_value("damage_tilt_strength", Some(&json!(0.))),
            Some(json!(0.1))
        );
    }
}

#[cfg(test)]
mod statistics_request_tests {
    use super::*;

    #[test]
    fn p1_statistics_session_requests_only_from_live_statistics_screen() {
        let mut session = Session::new_mode("statistics-request".into(), false);
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        session.ui = Some(UiState::new_state(&content).unwrap());
        assert_eq!(
            session
                .execute_ui_command(UiCommand::RequestStatistics)
                .unwrap_err(),
            "statistics_request_requires_play"
        );
        assert!(session.commands.is_empty());
        session.phase = "play".into();
        assert_eq!(
            session
                .execute_ui_command(UiCommand::RequestStatistics)
                .unwrap_err(),
            "statistics_request_requires_screen"
        );
        assert!(session.commands.is_empty());
        session
            .ui
            .as_mut()
            .unwrap()
            .transition(crate::ui::Screen::Statistics);
        session
            .execute_ui_command(UiCommand::RequestStatistics)
            .unwrap();
        assert_eq!(session.commands.len(), 1);
        assert_eq!(session.commands[0].id, 0x0c);
        assert_eq!(session.commands[0].payload.0, [1]);
    }
}

#[cfg(test)]
mod advancement_request_tests {
    use super::*;

    #[test]
    fn p1_advancement_seen_commands_require_play_and_real_selected_root() {
        let mut session = Session::new_mode("advancement-request".into(), false);
        assert_eq!(
            session
                .execute_ui_command(UiCommand::SeenAdvancements { tab: None })
                .unwrap_err(),
            "advancements_seen_requires_play"
        );
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        session.ui = Some(UiState::new_state(&content).unwrap());
        session.phase = "play".into();
        assert_eq!(
            session
                .execute_ui_command(UiCommand::SeenAdvancements {
                    tab: Some("minecraft:story/root".into())
                })
                .unwrap_err(),
            "advancements_seen_requires_selected_tab"
        );
        assert!(session.commands.is_empty());
        session
            .execute_ui_command(UiCommand::SeenAdvancements { tab: None })
            .unwrap();
        let expected = ui_packets::seen_advancements(None);
        assert_eq!(session.commands.len(), 1);
        assert_eq!(session.commands[0].id, expected.id);
        assert_eq!(session.commands[0].payload.0, expected.payload.0);
    }
}

#[cfg(test)]
mod render_distance_wire_tests {
    use super::*;

    #[test]
    fn render_distance_wire_inputs_reuse_ui_bounds() {
        let mut session = Session::new_mode("distance-32".into(), false);
        session.apply_client_setting("render_distance", &json!(32)).unwrap();
        assert_eq!(session.render_distance(), 32);
        assert_eq!(session.client_settings_snapshot()["render_distance"], json!(32));
        assert_eq!(
            UiState::sanitize_setting_value("render_distance", &json!(100)),
            Some(json!(UiState::MAX_RENDER_DISTANCE)),
        );
        assert_eq!(
            UiState::sanitize_setting_value("render_distance", &json!(0)),
            Some(json!(UiState::MIN_RENDER_DISTANCE)),
        );
    }

}

#[cfg(test)]
mod chat_completion_tests {
    use super::*;
    use crate::protocol::{packet_ids::play::clientbound as cb, Writer};
    use crate::ui::Modifiers;

    #[test]
    fn rc1_chat_tab_completes_advertised_literal_and_cycles() {
        let mut session = Session::new_mode("chat-completion".into(), false);
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        session.ui = Some(UiState::new_state(&content).unwrap());
        session.content = Some(content);
        session.phase = "play".into();

        // Root with two literal children, as encoded by the 26.3 commands packet.
        let mut packet = Writer::default();
        packet.var(3);
        packet.byte(0); packet.var(2); packet.var(1); packet.var(2);
        packet.byte(1); packet.var(0); packet.string("help");
        packet.byte(1); packet.var(0); packet.string("hello");
        packet.var(0);
        session.apply(Phase::Play, cb::COMMANDS, &packet.0).unwrap();

        let ui = session.ui.as_mut().unwrap();
        let content = session.content.as_ref().unwrap();
        ui.execute(UiIntent::OpenChat { command: true }, &mut session.inventory, content).unwrap();
        ui.rebuild_for_test(&session.inventory, content).unwrap();
        ui.execute(UiIntent::Text { text: "he".into() }, &mut session.inventory, content).unwrap();
        let tab = || UiIntent::Key { key: "Tab".into(), pressed: true, modifiers: Modifiers::default() };
        ui.execute(tab(), &mut session.inventory, content).unwrap();
        let first = ui.fields["chat/input"].text.clone();
        assert!(first == "/help" || first == "/hello", "{first}");
        ui.execute(tab(), &mut session.inventory, content).unwrap();
        let second = &ui.fields["chat/input"].text;
        assert!(second == "/help" || second == "/hello", "{second}");
        assert_ne!(&first, second);
        ui.execute(UiIntent::Key { key: "Tab".into(), pressed: true, modifiers: Modifiers { shift: true, ..Modifiers::default() } }, &mut session.inventory, content).unwrap();
        assert_eq!(ui.fields["chat/input"].text, first);
    }

    #[test]
    fn rc1_chat_argument_suggestions_correlate_and_replace_utf16_range() {
        use crate::protocol::{Reader, packet_ids::play::serverbound as sb};
        let mut session = Session::new_mode("chat-suggestions".into(), false);
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        session.ui = Some(UiState::new_state(&content).unwrap());
        session.content = Some(content);
        session.phase = "play".into();
        let mut tree = Writer::default();
        tree.var(3);
        tree.byte(0); tree.var(1); tree.var(1);
        tree.byte(1); tree.var(1); tree.var(2); tree.string("msg");
        tree.byte(18); tree.var(0); tree.string("target");
        tree.var(5); tree.var(0); tree.string("minecraft:ask_server");
        tree.var(0);
        session.apply(Phase::Play, cb::COMMANDS, &tree.0).unwrap();
        let ui = session.ui.as_mut().unwrap();
        let content = session.content.as_ref().unwrap();
        ui.execute(UiIntent::OpenChat { command: true }, &mut session.inventory, content).unwrap();
        ui.rebuild_for_test(&session.inventory, content).unwrap();
        ui.execute(UiIntent::Text { text: "msg 😀a tail".into() }, &mut session.inventory, content).unwrap();
        let field = ui.fields.get_mut("chat/input").unwrap();
        let cursor = "/msg 😀a".len();
        field.select(cursor, cursor).unwrap();
        let tab = || UiIntent::Key { key: "Tab".into(), pressed: true, modifiers: Modifiers::default() };
        let effects = ui.execute(tab(), &mut session.inventory, content).unwrap();
        for command in effects.commands { session.execute_ui_command(command).unwrap(); }
        assert_eq!(session.commands.len(), 1);
        assert_eq!(session.commands[0].id, sb::COMMAND_SUGGESTION);
        let mut reader = Reader::new(&session.commands[0].payload.0);
        let id = reader.var().unwrap();
        assert_eq!(reader.string(32500).unwrap(), "/msg 😀a");
        reader.done().unwrap();
        session.commands.clear(); // Simulate the outbound packet having been sent.
        let response = |id| {
            let mut w = Writer::default();
            w.var(id); w.var(5); w.var(3); w.var(2);
            w.string("Alice"); w.byte(0); w.string("Alex"); w.byte(0);
            w.0
        };
        session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &response(id.wrapping_add(1))).unwrap();
        assert_eq!(session.ui.as_ref().unwrap().fields["chat/input"].text, "/msg 😀a tail");
        session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &response(id)).unwrap();
        let ui = session.ui.as_mut().unwrap();
        assert_eq!(ui.fields["chat/input"].text, "/msg Alice tail");
        ui.execute(tab(), &mut session.inventory, session.content.as_ref().unwrap()).unwrap();
        assert_eq!(ui.fields["chat/input"].text, "/msg Alex tail");
        ui.execute(UiIntent::Key { key: "Tab".into(), pressed: true, modifiers: Modifiers { shift: true, ..Modifiers::default() } }, &mut session.inventory, session.content.as_ref().unwrap()).unwrap();
        assert_eq!(ui.fields["chat/input"].text, "/msg Alice tail");
        let field = ui.fields.get_mut("chat/input").unwrap();
        field.select(5, 10).unwrap();
        ui.execute(UiIntent::Text { text: "q".into() }, &mut session.inventory, session.content.as_ref().unwrap()).unwrap();
        let effects = ui.execute(tab(), &mut session.inventory, session.content.as_ref().unwrap()).unwrap();
        for command in effects.commands { session.execute_ui_command(command).unwrap(); }
        let mut reader = Reader::new(&session.commands.last().unwrap().payload.0);
        let new_id = reader.var().unwrap();
        assert_ne!(new_id, id);
        session.commands.clear();
        let ui = session.ui.as_mut().unwrap();
        ui.execute(UiIntent::Text { text: "x".into() }, &mut session.inventory, session.content.as_ref().unwrap()).unwrap();
        session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &response(new_id)).unwrap();
        assert_eq!(session.ui.as_ref().unwrap().fields["chat/input"].text, "/msg qx tail");
        let ui = session.ui.as_mut().unwrap();
        let effects = ui.execute(
            UiIntent::Key { key: "Tab".into(), pressed: true, modifiers: Modifiers { shift: true, ..Modifiers::default() } },
            &mut session.inventory, session.content.as_ref().unwrap()
        ).unwrap();
        for command in effects.commands { session.execute_ui_command(command).unwrap(); }
        let mut reader = Reader::new(&session.commands.last().unwrap().payload.0);
        let reverse_id = reader.var().unwrap();
        session.commands.clear();
        let mut reverse_response = Writer::default();
        reverse_response.var(reverse_id); reverse_response.var(5); reverse_response.var(2); reverse_response.var(2);
        reverse_response.string("Alice"); reverse_response.byte(0);
        reverse_response.string("Alex"); reverse_response.byte(0);
        session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &reverse_response.0).unwrap();
        assert_eq!(session.ui.as_ref().unwrap().fields["chat/input"].text, "/msg Alex tail");
    }
}

#[cfg(test)]
#[path = "book_edit_tests.rs"]
mod book_edit_tests;
