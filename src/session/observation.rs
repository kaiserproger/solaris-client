use super::*;
use crate::{
    environment::{CameraMedium, PlayerEnvironmentFacts},
    inventory::{MenuKind, Stack},
    player::Hit,
    protocol::content::FluidKind,
    ui::{HudState, UiPresentation},
};
impl Session {
    pub fn capture_terrain_metrics(&self) -> Value {
        self.terrain
            .as_ref()
            .map_or(Value::Null, Terrain::capture_metrics)
    }
    pub fn minecraft_observation(&self) -> Value {
        let screen = self
            .ui
            .as_ref()
            .map(|ui| ui.source_screen_metadata(&self.inventory))
            .transpose();
        let screen = match screen {
            Ok(Some(Some((class, title)))) => json!({"open":true,"class":class,"title":title}),
            Ok(_) => json!({"open":false,"class":"none","title":""}),
            Err(error) => json!({"open":true,"class":null,"title":null,"error":error}),
        };
        let recent_chat = self
            .ui
            .as_ref()
            .map(|ui| {
                let text = ui.text_renderer();
                ui.chat()
                    .iter()
                    .rev()
                    .take(20)
                    .map(|line| {
                        text.language
                            .plain(&line.component)
                            .map(Value::String)
                            .unwrap_or_else(|error| json!({"error":error}))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // `in_play` means a visible playable world, not just received bytes:
        // the spawn-neighborhood GPU latch (epoch tuple, bypassed headless/dead)
        // must also hold, else automation acts behind the Loading overlay.
        let in_play = self.phase == "play"
            && self.play_world_presentation_ready()
            && self.spawn_meshes_presented();
        let mut value = json!({"in_play":in_play,"screen":screen,"overlay":{"open":false,"class":"none"},"recent_chat":recent_chat,
            "debug_overlay":self.debug_overlay_pending,"state_version":self.state_version,"client_tick":self.client_tick,"origin":"received_state_and_explicit_local_player_prediction"});
        value["loader"] = self.loader.observation();
        value["far_terrain"] = self.far.metrics();
        if let Some(error) = &self.error {
            value["disconnect_reason"] = json!(error);
        }
        // Title screen state (pre-play): field text + error line so the
        // windowed Title proof can assert typed values and the disconnect
        // reason through the ordinary observation path.
        if let Some(ui) = &self.ui {
            value["dialog"] = ui.dialog_observation();
            if ui.screen() == crate::ui::Screen::Title {
                value["title_server"] = json!(ui.title_field("title/server"));
                value["title_username"] = json!(ui.title_field("title/username"));
                value["title_error"] = json!(ui.title_error_text());
            }
        }
        if let (Some(ui), Some(content)) = (&self.ui, &self.content) {
            value["statistics"] = ui
                .statistics()
                .observation(content)
                .unwrap_or_else(|error| json!({"error":error}));
        }
        if !in_play {
            return value;
        }
        // Local mining progress (same stage formula as the crack overlay):
        // lets automation screenshot mid-break cracks without wall-clock sleeps.
        if let Some(breaking) = self.player.breaking_observation() {
            value["breaking"] = breaking;
        }
        // Windowed-proof seams: F3/Options assertions read these (nodes carry
        // debug/line/* text and settings/* row labels; settings carries
        // applied values like fov).
        if let Some(ui) = &self.ui {
            value["ui_nodes"] = json!(ui.observed_nodes());
            value["client_settings"] = json!(ui.observed_settings());
            value["client_settings"]["render_distance"] = json!(self.render_distance);
        }
        let player = &self.player;
        value["dimension"] = json!(self.dimension);
        value["player"] = json!({"entity_id":player.entity_id,"entity_uuid":player.uuid,"entity_type":"minecraft:player",
            "x":player.position[0],"y":player.position[1],"z":player.position[2],"yaw":player.yaw,"pitch":player.pitch,
            "on_ground":player.grounded,"horizontal_collision":player.horizontal_collision,
            "velocity":vector(player.velocity),"health":player.health,"max_health":player.attributes.get("minecraft:max_health").map(|attribute| attribute.value),
            "food":player.food,"saturation":player.saturation,"experience_level":player.experience_level,
            "experience_progress":player.experience_progress,"total_experience":player.experience_total,
            "selected_hotbar_slot":self.inventory.selected_slot,"pose":player.pose,
            "hurt_tilt_deg":player.hurt_tilt_deg(self.client_tick,0.,self.damage_tilt_strength),
            "hurt_direction":player.hurt_direction,"hurt_tick":player.hurt_tick.map(|tick|tick.to_string()),
            "server_position":player.server_position,"origin":"locally_predicted_reconciled_by_server"});
        value["player"]["motion"] = player.visual_motion_observation();
        // Vanilla Entity.getAirSupply is only ever *received* for the local player; a server
        // that never sends DATA_AIR_SUPPLY_ID leaves it null. `eye_medium` is derived from the
        // received world so submersion is always observable, not only the server's air count.
        value["player"]["air_supply"] = json!(self.local_air_supply());
        value["player"]["max_air_supply"] = json!(300);
        value["player"]["eye_medium"] = match self
            .content
            .as_ref()
            .map(|content| player.eye_medium(&self.world, content))
        {
            Some(Ok(Some(FluidKind::Water))) => json!("water"),
            Some(Ok(Some(FluidKind::Lava))) => json!("lava"),
            Some(Ok(None)) => json!("air"),
            _ => Value::Null,
        };
        value["player"]["camera_roll_deg"] = json!(self.scene.roll_deg);
        value["player"]["camera_position"] = json!(self.scene.camera_position);
        value["player"]["block_motions"] = json!(self.block_motion.len());
        value["player"]["entity_motion"] = self.entities.motion_observation();
        value["selected_item"] = self.item_value(
            self.inventory
                .received_player_slot(36 + u16::from(self.inventory.selected_slot))
                .ok()
                .flatten(),
        );
        let mut inventory = Vec::new();
        for slot in 0..43u16 {
            let wire = match slot {
                0..=8 => 36 + slot,
                9..=35 => slot,
                36..=39 => 44 - slot,
                40 => 45,
                _ => slot + 5,
            };
            let stack = self.inventory.received_player_slot(wire).ok().flatten();
            if stack.is_some_and(Stack::empty) {
                continue;
            }
            let mut item = self.item_value(stack);
            item["slot"] = json!(slot);
            item["selected"] = json!(slot == u16::from(self.inventory.selected_slot));
            inventory.push(item);
        }
        value["inventory"] = json!(inventory);
        let menu = self.inventory.received_menu();
        let slots = menu
            .slots
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.as_ref().is_some_and(Stack::empty))
            .map(|(slot, stack)| {
                let mut item = self.item_value(stack.as_ref());
                item["slot"] = json!(slot);
                item
            })
            .collect::<Vec<_>>();
        value["container"] = json!({"container_id":menu.id,"container_state_id":menu.state_id,"menu_class":menu_class(menu.kind),"slot_count":menu.slots.len(),"slots":slots,"carried":self.item_value(self.inventory.received_cursor())});
        if let Some(trades) = &menu.trades {
            value["container"]["trades"] = self.trades_value(trades);
        }
        value["target"] = match self
            .content
            .as_ref()
            .map(|content| player.raycast(&self.world, content, &self.entities, self.client_tick))
        {
            Some(Ok(Some(Hit::Block(hit)))) => {
                json!({"type":"block","location":vector(hit.point),"x":hit.position[0],"y":hit.position[1],"z":hit.position[2],"face":(["down","up","north","south","west","east"][hit.face as usize]),"block_id":self.content.as_ref().and_then(|content| content.name(hit.state))})
            }
            Some(Ok(Some(Hit::Entity { handle, point, .. }))) => {
                match self.entities.resolve(handle) {
                    Ok(entity) => {
                        json!({"type":"entity","location":vector(point),"entity_id":handle.network_id,"entity_uuid":entity.uuid,"entity_type":entity.type_name})
                    }
                    Err(error) => json!({"type":"unknown","error":error}),
                }
            }
            Some(Err(error)) => json!({"type":"unknown","error":error}),
            _ => json!({"type":"none"}),
        };
        value["time"] = json!({"game_time":self.environment.game_time(),"clock_time":self.environment.default_clock_time(),"day":self.environment.default_clock_time()/24000});
        if let Some(content) = &self.content {
            match content.registry_tag("minecraft:fluid", "minecraft:water") {
                Ok(tag) => {
                    value["water_tag_entries"] = json!(tag.len());
                    value["water_tag_fluids"] = json!(
                        tag.iter()
                            .map(|holder| content.registry_name("minecraft:fluid", *holder))
                            .collect::<Result<Vec<_>>>()
                            .ok()
                    );
                }
                Err(error) => {
                    value["water_tag_entries"] = Value::Null;
                    value["water_tag_fluids"] = json!({"error":error});
                }
            }
        }
        value
    }
    /// Server-sent air supply of the local player (ticks; 300 is full).
    pub(crate) fn local_air_supply(&self) -> Option<i32> {
        self.entities
            .get(self.player.entity_id)
            .and_then(|entity| {
                let models = self.entities.models_ref()?;
                let index = models.accessor(&entity.type_name, "DATA_AIR_SUPPLY_ID")?;
                entity.metadata.get(&index)?.int()
            })
            .or_else(|| self.entities.own_metadata_int("DATA_AIR_SUPPLY_ID"))
    }
    /// Received MerchantOffers: `buy_a.count` is the effective price (demand and special
    /// price applied, as the vanilla trade button shows); `base_count` is the listed cost.
    pub(crate) fn trades_value(&self, trades: &crate::inventory::Trades) -> Value {
        let cost = |cost: &crate::inventory::ItemCost, count: Option<i32>| {
            let mut value = self.item_value(Some(&cost.stack));
            value["base_count"] = json!(cost.stack.count);
            if let Some(count) = count {
                value["count"] = json!(count);
            }
            value
        };
        let offers = trades
            .offers
            .iter()
            .enumerate()
            .map(|(index, offer)| {
                json!({"index":index,
                    "buy_a":cost(&offer.buy_a, offer.cost_a().ok()),
                    "buy_b":offer.buy_b.as_ref().map(|buy| cost(buy, None)),
                    "sell":self.item_value(Some(&offer.sell)),
                    "out_of_stock":offer.out_of_stock,"uses":offer.uses,"max_uses":offer.max_uses,
                    "available":!offer.out_of_stock && offer.uses < offer.max_uses,
                    "xp":offer.xp,"special_price":offer.special_price,
                    "price_multiplier":offer.price_multiplier,"demand":offer.demand})
            })
            .collect::<Vec<_>>();
        json!({"offers":offers,"villager_level":trades.level,"villager_xp":trades.xp,
            "show_progress":trades.show_progress,"can_restock":trades.can_restock,"selected":trades.selected})
    }
    fn item_value(&self, stack: Option<&Stack>) -> Value {
        let Some(stack) = stack else {
            return json!({"state":"unknown","item_id":null,"count":null,"origin":"server_observed"});
        };
        if stack.empty() {
            return json!({"item_id":"minecraft:air","count":0,"enchantments":[],"origin":"server_observed"});
        }
        let content = self.content.as_ref();
        let name = content
            .and_then(|content| crate::ui::text::item_name_component(content, stack).ok())
            .and_then(|component| {
                self.ui
                    .as_ref()
                    .and_then(|ui| ui.text_renderer().language.plain(&component).ok())
            });
        let enchantments = stack
            .component("minecraft:enchantments")
            .and_then(Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .map(|(id, level)| json!({"id":id,"level":level}))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let foil = stack
            .component("minecraft:enchantment_glint_override")
            .and_then(Value::as_bool)
            .unwrap_or(!enchantments.is_empty());
        json!({"item_id":content.and_then(|content| content.item_names.get(&stack.id)),"count":stack.count,"name":name,
            "damage":stack.component("minecraft:damage").and_then(Value::as_i64).unwrap_or(0),"max_damage":stack.component("minecraft:max_damage").and_then(Value::as_i64).unwrap_or(0),"foil":foil,"enchantments":enchantments,"origin":"server_observed"})
    }
    pub fn observation(&self) -> Observation {
        let entities = self.entities.observation(None, 512);
        let blocks = self.nearby_blocks();
        let truncated = blocks.len() > 128 || entities["truncated"].as_bool().unwrap_or(false);
        let data = json!({"phase":self.phase,"error":self.error,"server_status":self.status,
            "minecraft":self.minecraft_observation(),"player":{"position":self.player.position,"server_position":self.player.server_position,"yaw":self.player.yaw,"pitch":self.player.pitch,"on_ground":self.player.grounded,"velocity":self.player.velocity,"pose_known":self.player.pose_known,"entity_id":self.player.entity_id,"game_mode":self.player.game_mode,"health":self.player.health,"food":self.player.food,"origin":"locally_predicted_reconciled_by_server"},
            "inventory":self.inventory.observation(),"entities":entities,"loaded_blocks":blocks.into_iter().take(128).collect::<Vec<_>>(),"loaded_chunks":self.world.chunk_count(),
            "camera":{"hurt_tilt_deg":self.player.hurt_tilt_deg(self.client_tick,0.,self.damage_tilt_strength),
                "hurt_direction":self.player.hurt_direction,"damage_tilt_strength":self.damage_tilt_strength,
                "roll_deg":self.scene.roll_deg,"position":self.scene.camera_position,
                "motion":self.player.visual_motion_observation(),"block_motions":self.block_motion.len()},
            "inputs":{"held":self.keys},"screen":self.screen(),"breaking":self.player.breaking_observation(),"using":self.player.using_observation(self.client_tick),"events":self.events,
            "presentation_available":self.presentation,"packets":{"counts":self.packet_counts,"unsupported_presentation":self.ignored_presentation},"audio":self.audio.as_ref().map(Audio::observation),"terrain":self.terrain.as_ref().map(Terrain::metrics),"movement_blocked":self.player.movement_blocked,
            "far_terrain":self.far.metrics(),
            "corrections":self.player.corrections,"last_correction_distance":self.player.last_correction_distance,"state_version":self.state_version});
        self.observation_envelope(data, truncated)
    }

    /// Shared transport state is execution context, not an unsolicited full
    /// world query. Canonical observations run on demand on the session owner.
    pub(crate) fn execution_observation(&self) -> Observation {
        self.observation_envelope(json!({
            "snapshot_kind": "execution_context",
            "phase": self.phase,
            "error": self.error,
            "presentation_available": self.presentation,
            "state_version": self.state_version,
        }), false)
    }

    fn observation_envelope(&self, data: Value, truncated: bool) -> Observation {
        Observation {
            instance_id: self.instance_id.clone(),
            connection_epoch: self.connection_epoch.to_string(),
            dimension_epoch: self.dimension_epoch.to_string(),
            state_revision: self.revision.to_string(),
            last_applied_packet_sequence: self.packet_sequence.to_string(),
            client_tick: self.client_tick.to_string(),
            server_tick: None,
            resource_epoch: self.resource_epoch.to_string(),
            captured_frame_id: None,
            origin: "server_observed_and_explicit_local_prediction".into(),
            truncated,
            next_cursor: None,
            data,
        }
    }
    fn nearby_blocks(&self) -> Vec<Value> {
        let mut result = Vec::new();
        let Some(content) = &self.content else {
            return result;
        };
        let base = self
            .player
            .position
            .map(|coordinate| coordinate.floor() as i32);
        for y in base[1] - 3..=base[1] + 3 {
            for z in base[2] - 4..=base[2] + 4 {
                for x in base[0] - 4..=base[0] + 4 {
                    let position = [x, y, z];
                    if let Some(state) = self.received_block(position) {
                        if content.block_is_air(state).ok() == Some(false) {
                            result.push(json!({"position":position,"state_id":state,"block":content.name(state),"origin":"server_observed"}));
                            if result.len() == 129 {
                                return result;
                            }
                        }
                    }
                }
            }
        }
        result
    }
    pub(crate) fn spawn_meshes_ready(&self) -> bool {
        self.phase == "play"
            && self.play_world_presentation_ready()
            && self.terrain.as_ref().is_some_and(|terrain| {
                terrain.spawn_meshes_ready(self.player.eye_position())
            })
    }
    pub(crate) fn note_renderer_scene_confirmation(
        &mut self,
        connection: u64,
        dimension: u64,
        revision: u64,
    ) {
        if (connection, dimension) == self.context_epochs()
            && !self.renderer_scene_confirmation.is_some_and(|(c, d, _)| {
                (c, d) == (connection, dimension)
            })
        {
            self.renderer_scene_confirmation = Some((connection, dimension, revision));
            self.changed_state();
        }
    }
    pub(crate) fn spawn_meshes_presented(&self) -> bool {
        !self.presentation
            || self.player.death_message.is_some()
            || self.player.health.is_some_and(|health| health <= 0.)
            || self.renderer_scene_confirmation.is_some_and(|(connection, dimension, _)| {
                (connection, dimension) == self.context_epochs()
            })
    }
    pub(crate) fn play_world_presentation_ready(&self) -> bool {
        if !self.player.pose_known || self.dimension.is_none() {
            return false;
        }
        let camera = self.player.eye_position();
        let block = camera.map(|coordinate| coordinate.floor() as i32);
        if self.world.block(block).is_none()
            || self.world.light_levels(block).iter().any(Option::is_none)
        {
            return false;
        }
        // The environment renderer samples biome columns around the camera.
        // Keep the loading screen up until this required neighborhood is known.
        let floor = camera.map(|coordinate| (coordinate * 0.25 - 0.5).floor() as i32);
        let x_range = ((floor[0] - 2) * 4).div_euclid(16)..=((floor[0] + 3) * 4).div_euclid(16);
        x_range.into_iter().all(|x| {
            (((floor[2] - 2) * 4).div_euclid(16)..=((floor[2] + 3) * 4).div_euclid(16))
                .all(|z| self.world.chunk([x, z]).is_some())
        })
    }
    fn sync_ui_state(&mut self, world_ready: bool) -> Result<()> {
        let player_dead = self.player.death_message.is_some()
            || self.player.health.is_some_and(|health| health <= 0.);
        let loading = match self.phase.as_str() {
            "loading_resources" => Some("Loading resources"),
            "status" | "login" | "configuration" => Some("Connecting"),
            "play" if !player_dead && (!world_ready || !self.spawn_meshes_presented()) => {
                Some("Preparing terrain and light")
            }
            _ => None,
        }.map(str::to_owned);
        let local_air = self.local_air_supply();
        if let Some(ui) = &mut self.ui {
            // armor: LivingEntity.getArmorValue floors minecraft:armor.
            let armor = self
                .player
                .attributes
                .get("minecraft:armor")
                .map(|attribute| attribute.value.floor() as i32);
            // air: Entity DATA_AIR_SUPPLY_ID on the local player's own entity
            // row; max is the vanilla Entity.getMaxAirSupply constant (300).
            // Neither vanilla SetHealth nor UpdateAttributes carries air, so
            // without an observed local-player metadata row both stay None and
            // the bubbles row stays hidden.
            let (air, max_air) = (local_air, local_air.map(|_| 300));
            let absorption = self
                .entities
                .get(self.player.entity_id)
                .and_then(|entity| {
                    let models = self.entities.models_ref()?;
                    let index = models.accessor(&entity.type_name, "DATA_PLAYER_ABSORPTION_ID")?;
                    entity.metadata.get(&index).and_then(|value| match value {
                        crate::entities::Metadata::Float(amount) => Some(*amount),
                        _ => None,
                    })
                })
                .unwrap_or(0.);
            let hunger = self.player.effects.contains_key("minecraft:hunger");
            let player_list_visible = self.phase == "play"
                && ui.screen() == crate::ui::Screen::Game
                && self.keys.contains("player_list");
            let players = if player_list_visible {
                let board = &ui.network_hud.scoreboard;
                let mut profiles: Vec<_> = self.entities.player_profiles().filter(|p| p.listed).collect();
                // 26.3 PlayerTabOverlay static comparator, javap -c -p: negated
                // order, spectator last, team name, then compareToIgnoreCase.
                profiles.sort_by(|a, b| {
                    a.list_order.wrapping_neg().cmp(&b.list_order.wrapping_neg())
                        .then_with(|| (a.game_mode == 3).cmp(&(b.game_mode == 3)))
                        .then_with(|| board.membership.get(&a.name).map_or("", String::as_str)
                            .encode_utf16().cmp(board.membership.get(&b.name).map_or("", String::as_str).encode_utf16()))
                        .then_with(|| crate::ui::UiState::compare_player_names(&a.name, &b.name))
                });
                profiles.into_iter().take(80).map(|p| crate::ui::PlayerListEntry {
                    id: p.uuid.clone(),
                    name: p.display_name.clone().unwrap_or_else(|| {
                        board.formatted_name(&p.name, None).as_ref().clone()
                    }),
                    latency: p.latency,
                    game_mode: p.game_mode,
                }).collect()
            } else {
                Vec::new()
            };
            ui.set_presentation(UiPresentation {
                hud: HudState {
                    health: self.player.health,
                    max_health: self
                        .player
                        .attributes
                        .get("minecraft:max_health")
                        .map(|attribute| attribute.value as f32),
                    food: self.player.food,
                    armor,
                    air,
                    max_air,
                    absorption,
                    hunger,
                    experience: Some(self.player.experience_progress),
                    level: Some(self.player.experience_level),
                    player_tick: Some(self.client_tick as i32),
                    attack: Some(self.player.attack_strength(0.)),
                    players,
                    player_list_visible,
                    ..Default::default()
                },
                loading,
                death: self.player.death_message.clone(),
                can_respawn: self.player.health.is_some_and(|health| health <= 0.),
                editor: None,
                can_edit_operator_blocks: self
                    .player
                    .permission_level
                    .is_some_and(|level| level >= 2),
            })?;
            if self.presentation && self.inventory.screen_open
                && self.inventory.menu().kind == MenuKind::Inventory
            {
                let textures = self.player.uuid.as_deref()
                    .and_then(|uuid| self.entities.player_profile(uuid))
                    .and_then(|profile| profile.textures);
                ui.set_player_preview(
                    self.content.as_ref().ok_or("content unavailable")?,
                    self.player.uuid.as_deref(), textures,
                )?;
            }
            if ui.screen() != crate::ui::Screen::Loader {
                self.loader.clear_view(None)?;
            }
        }
        if self
            .ui
            .as_mut()
            .is_some_and(|ui| std::mem::take(&mut ui.release_pending))
        {
            self.keys.clear();
            self.player.release_all(&mut self.commands)?;
        }
        Ok(())
    }
    pub(super) fn refresh_scene(&mut self) -> Result<bool> {
        self.refresh_scene_mode(false)
    }

    /// Choose far-terrain tiles around the camera, send the server requests
    /// they need and publish changed tiles. True when the draw list changed.
    fn refresh_far(&mut self) -> bool {
        let world = &self.world;
        let requests = self.far.update(
            self.scene.camera_position,
            world.chunk_lifecycle_revision(),
            &|column| world.chunk(column).is_some(),
        );
        for payload in requests {
            if let Err(error) = self.send_custom("solaris:lod/request", &payload) {
                self.event(json!({"type":"far_terrain_request_failed","error":error}));
                break;
            }
        }
        if let Some(content) = self.content.as_ref()
            && let Err(error) = self.far.poll_generated(content)
        {
            self.event(json!({"type":"far_terrain_generation_failed","error":error}));
        }
        self.far.scene(&mut self.scene.far)
    }

    pub(super) fn refresh_scene_mode(&mut self, animation_only: bool) -> Result<bool> {
        self.camera();
        let world_ready = self.play_world_presentation_ready();
        if !animation_only {
            self.sync_ui_state(world_ready)?;
        }
        if !self.presentation {
            return Ok(false);
        }
        // Title screen is pre-play (no dimension/terrain): still render the
        // UI overlay so windowed Title captures show the real screen, not
        // the stale 3D scene behind it.
        if self.dimension.is_none() {
            if self.scene_state_revision == self.revision {
                return Ok(false);
            }
            self.scene.overlay.vertices.clear();
            for indices in &mut self.scene.overlay.indices {
                indices.clear();
            }
            if let Some(ui) = &mut self.ui {
                let content = self.content.as_ref().ok_or("content unavailable")?;
                ui.render(content, &self.inventory, &mut self.scene.overlay)?;
            }
            self.scene_state_revision = self.revision;
            self.scene.revision += 1;
            return Ok(true);
        }
        // Early v2 login may deliver player state before the spawn chunk and
        // its light. Do not submit known-lit meshes until the causal lightmap
        // can be built; that is a loading state, not a fatal renderer error.
        let mesh_changed = if !animation_only && (world_ready || self.scene.lightmap.is_some()) {
            self.terrain.as_mut().ok_or("terrain unavailable")?.update(
                &self.world,
                self.scene.camera_position,
                self.render_distance,
                &mut self.scene.chunks,
            )?
        } else {
            false
        };
        let far_changed = !animation_only && world_ready && self.refresh_far();
        let state_changed = self.scene_state_revision != self.revision;
        if !animation_only
            && !state_changed
            && !mesh_changed
            && !far_changed
            && (self.scene.lightmap.is_some() || !world_ready)
        {
            return Ok(false);
        }
        if !animation_only && !state_changed && self.scene.lightmap.is_some() {
            self.scene.revision += 1;
            return Ok(true);
        }
        self.scene.origin = [
            (self.player.position[0] / 256.).floor() * 256.,
            0.,
            (self.player.position[2] / 256.).floor() * 256.,
        ];
        self.scene.dynamic.vertices.clear();
        for indices in &mut self.scene.dynamic.indices {
            indices.clear();
        }
        self.scene.entity_presentation = Default::default();
        if !animation_only {
        self.scene.overlay.vertices.clear();
        for indices in &mut self.scene.overlay.indices {
            indices.clear();
        }
        }
        if world_ready {
            self.entities.sync_local_avatar(
                &self.player,
                &self.inventory,
                &self.world,
                self.content.as_ref().ok_or("content unavailable")?,
                (self.client_tick, self.environment.game_time()),
            )?;
            self.entities.set_local_visible(self.camera_mode != 0);
            if !animation_only {
            let medium = match self.player.eye_medium(
                &self.world,
                self.content.as_ref().ok_or("content unavailable")?,
            )? {
                Some(FluidKind::Water) => CameraMedium::Water,
                Some(FluidKind::Lava) => CameraMedium::Lava,
                None => CameraMedium::Atmospheric,
            };
            let facts = PlayerEnvironmentFacts {
                medium,
                spectator: self.player.game_mode == 3,
                fire_resistance: self
                    .player
                    .effect_duration("minecraft:fire_resistance", self.client_tick)
                    .is_some(),
                night_vision_duration: self
                    .player
                    .effect_duration("minecraft:night_vision", self.client_tick),
                blindness_duration: self
                    .player
                    .effect_duration("minecraft:blindness", self.client_tick),
                darkness_present: self
                    .player
                    .effect_duration("minecraft:darkness", self.client_tick)
                    .is_some(),
                water_vision: self.player.water_vision,
                conduit_power: self
                    .player
                    .effect_duration("minecraft:conduit_power", self.client_tick)
                    .is_some(),
                entity_tick: self.client_tick,
                flat_world: self.flat_world,
                ..Default::default()
            };
            self.scene.environment = self.environment.evaluate(
                &self.world,
                self.content.as_ref().ok_or("content unavailable")?,
                self.scene.camera_position,
                self.player.yaw,
                self.player.pitch,
                self.render_distance,
                &facts,
            )?;
            // Far terrain continues past the loaded chunks: the vanilla
            // render-distance fog moves out to the far distance with it.
            let far = self.far.active_distance() as f32;
            let environment = &mut self.scene.environment;
            if far > environment.render_fog_end {
                environment.render_fog_start = far - (far / 10.).clamp(4., 64.);
                environment.render_fog_end = far;
            }
            self.scene.lightmap = self.environment.lightmap_texture();
            self.scene.shader_game_time =
                Some((self.environment.game_time().rem_euclid(24000) as f32) / 24000.);
            }
            super::weather::append(
                &self.world,
                self.content.as_ref().ok_or("content unavailable")?,
                self.scene.camera_position,
                self.scene.origin,
                self.scene.environment.rain,
                self.client_tick as f64 + f64::from(self.presentation_partial),
                &mut self.scene.dynamic,
            )?;
            {
                let content = self.content.as_ref().ok_or("content unavailable")?;
                let world = &self.world;
                let sample_light = |point: [f64; 3]| {
                    let [sky, block] =
                        world.light_levels(point.map(|coordinate| coordinate.floor() as i32));
                    Ok(sky
                        .zip(block)
                        .map(|(sky, block)| [f32::from(sky), f32::from(block)]))
                };
                self.scene.entity_presentation = self.entities.append_geometry(
                    content,
                    self.scene.origin,
                    self.scene.camera_position,
                    [self.scene.yaw, self.scene.pitch],
                    self.render_distance as f32 * 16.,
                    self.client_tick,
                    self.presentation_partial,
                    &sample_light,
                    &mut self.scene.dynamic,
                )?;
                let entity_indices = self.scene.dynamic.indices.iter().map(Vec::len).sum::<usize>();
                self.scene.entity_presentation.entity_indices = entity_indices;
                let blocks = self.entities.append_static_block_geometry(
                    content,
                    world,
                    self.scene.origin,
                    self.scene.camera_position,
                    self.render_distance as f32 * 16.,
                    self.player.is_scoping(content),
                    // Match the filtering preference sent by client_information.
                    false,
                    self.presentation_partial,
                    &sample_light,
                    &mut self.scene.dynamic,
                )?;
                self.scene.entity_presentation.block_entity_indices =
                    self.scene.dynamic.indices.iter().map(Vec::len).sum::<usize>() - entity_indices;
                self.scene.entity_presentation.block_entities += blocks.block_entities;
                self.scene.entity_presentation.vertices += blocks.vertices;
                self.scene.entity_presentation.lighting_pending += blocks.lighting_pending;
                self.scene
                    .entity_presentation
                    .unrendered
                    .extend(blocks.unrendered);
                if let Some(ui) = &self.ui {
                    let crosshair = match self.player.raycast(
                        world,
                        content,
                        &self.entities,
                        self.client_tick,
                    )? {
                        Some(Hit::Entity { handle, .. }) => Some(handle),
                        _ => None,
                    };
                    let nameplates = self.entities.append_nameplate_geometry(
                        content,
                        self.scene.origin,
                        self.scene.camera_position,
                        [self.scene.yaw, self.scene.pitch],
                        self.render_distance as f32 * 16.,
                        self.client_tick,
                        self.presentation_partial,
                        &crate::entities::NameplateView {
                            crosshair,
                            // Session::camera currently follows the local player.
                            camera_entity: Some(self.player.entity_id),
                            viewer_name: self.username.as_deref(),
                            viewer_spectator: self.player.game_mode == 3,
                            render_names: !ui
                                .setting("hide_hud")
                                .and_then(Value::as_bool)
                                .unwrap_or(false),
                            scoreboard: ui.scoreboard(),
                            background_opacity: ui.text_background_opacity(0.25),
                        },
                        &sample_light,
                        &mut self.scene.dynamic,
                    )?;
                    self.scene.entity_presentation.vertices += nameplates.vertices;
                    self.scene.entity_presentation.lighting_pending += nameplates.lighting_pending;
                    self.scene
                        .entity_presentation
                        .unrendered
                        .extend(nameplates.unrendered);
                }
                if self.camera_mode == 0 && !self.ui.as_ref().and_then(|ui| ui.setting("hide_hud")).and_then(Value::as_bool).unwrap_or(false) {
                    if let Some(light) = sample_light(self.player.eye_position())? {
                        let result = self.player.append_first_person(
                            content,
                            &self.inventory,
                            &mut self.entities,
                            &mut self.scene,
                            self.presentation_partial,
                            light,
                        );
                        if let Err(reason) = result {
                            self.scene.entity_presentation.unrendered.insert(
                                "local:first_person".into(),
                                reason,
                            );
                        }
                    }
                }
            }
            self.append_destruction_overlays()?;
            self.block_effects.append(
                &self.world,
                self.scene.origin,
                self.player.yaw,
                self.player.pitch,
                self.presentation_partial,
                &mut self.scene.dynamic,
            )?;
            if let Some(terrain) = &self.terrain {
                let now = (self.client_tick as f64 + f64::from(self.presentation_partial)) * 0.05;
                self.block_motion.append(
                    terrain,
                    &self.world,
                    self.scene.origin,
                    now,
                    &mut self.scene.dynamic,
                )?;
            }
        }
        if !animation_only && let Some(ui) = &mut self.ui {
            let content = self.content.as_ref().ok_or("content unavailable")?;
            ui.render(content, &self.inventory, &mut self.scene.overlay)?;
        }
        self.scene_state_revision = self.revision;
        self.scene.revision += 1;
        Ok(true)
    }
}
impl Session {
    /// MCP seam for `minecraft_toggle_debug_overlay`: the App owner holds the
    /// flag (lib.rs), but the tool executes on the session. These bridge the
    /// two: read back the session-part lines (XYZ/facing/chunk/biome/
    /// entities/day/phase; fps/draws need the renderer's metrics and appear
    /// on the next observe). The flag itself is set by the App intercept in
    /// drain_requests before dispatch reaches the session.
    pub fn debug_overlay_lines_for_mcp(&self) -> Vec<String> {
        self.debug_lines(&serde_json::Value::Null)
    }
    /// F3 debug overlay lines (vanilla subset, real sources only).
    /// `render` is the `observation.data["render"]` object the renderer owns
    /// via `Renderer::update_metrics` (src/render/metrics.rs); missing keys
    /// are omitted, never fabricated. Capped at 16 lines.
    pub fn debug_lines(&self, render: &Value) -> Vec<String> {
        let mut lines = Vec::new();
        // The renderer owns the windowed submission rate. Until its first
        // complete window, omit FPS rather than showing a bursty reciprocal.
        if let Some(fps) = render.get("fps").and_then(Value::as_f64).filter(|fps| fps.is_finite()) {
            let mut line = format!("{} fps", fps.round() as u64);
            if let Some(wall) = render.get("frame_wall_ms").and_then(Value::as_f64)
                && wall.is_finite()
            {
                line.push_str(&format!(" ({wall:.2} ms)"));
            }
            lines.push(line);
        }
        if let (Some(draws), Some(indices)) = (
            render.get("draw_calls").and_then(Value::as_u64),
            render.get("index_count").and_then(Value::as_u64),
        ) {
            lines.push(format!("draws: {draws} indices: {indices}"));
        }
        // xyz (session.player.position, 3 decimals).
        let [x, y, z] = self.player.position;
        lines.push(format!("XYZ: {x:.3} / {y:.3} / {z:.3}"));
        // Facing: yaw convention from render/frame.rs (forward =
        // [-sin(yaw)cos(pitch), -sin(pitch), cos(yaw)cos(pitch)]): yaw 0 ->
        // +Z (south), yaw 180 -> -Z (north), yaw -90 -> +X (east).
        let yaw = f64::from(self.player.yaw).rem_euclid(360.);
        let compass = ((yaw + 22.5).rem_euclid(360.) / 45.) as usize % 8;
        let name = ["S", "SW", "W", "NW", "N", "NE", "E", "SE"][compass];
        lines.push(format!(
            "Facing: {name} (yaw {:.1} / pitch {:.1})",
            self.player.yaw, self.player.pitch
        ));
        // Chunk + loaded chunk count (World::chunk_count).
        let cx = (x.floor() as i32).div_euclid(16);
        let cz = (z.floor() as i32).div_euclid(16);
        lines.push(format!(
            "Chunk: {cx} {cz} (loaded: {})",
            self.world.chunk_count()
        ));
        // Biome: World::biome -> Content registry name; unknown when no
        // column data or no registry entry.
        let block = [x.floor() as i32, y.floor() as i32, z.floor() as i32];
        let biome = self
            .world
            .biome(block)
            .ok_or(())
            .and_then(|id| {
                self.content.as_ref().ok_or(()).and_then(|c| {
                    c.registry_name("minecraft:worldgen/biome", id as i32)
                        .map_err(|_| ())
                })
            })
            .map(str::to_owned)
            .unwrap_or_else(|_| "unknown".into());
        lines.push(format!("Biome: {biome}"));
        // Entities: scene.entity_presentation (known/eligible/submitted) with
        // store.len() fallback before the first scene build.
        let stats = &self.scene.entity_presentation;
        if stats.known > 0 || stats.eligible > 0 || stats.submitted > 0 {
            lines.push(format!(
                "Entities: {} known / {} eligible / {} submitted",
                stats.known, stats.eligible, stats.submitted
            ));
        } else {
            lines.push(format!("Entities: {} known", self.entities.len()));
        }
        // Game time/day (Environment::game_time).
        let time = self.environment.game_time();
        lines.push(format!("Day {} ({time})", time.div_euclid(24000)));
        lines.push(format!("Phase {} tick {}", self.phase, self.client_tick));
        lines.truncate(16);
        lines
    }
}

#[cfg(test)]
mod debug_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn fps_formats_from_render_metrics_and_missing_keys_omit() {
        let session = Session::new_mode("f3-test".into(), false);
        let full = session.debug_lines(&json!({
            "fps": 60.0,
            "frame_wall_ms": 5.125,
            "draw_calls": 42,
            "index_count": 1000,
        }));
        assert!(full[0].starts_with("60 fps"), "fps line: {full:?}");
        assert!(full[0].contains("5.12 ms"), "frame ms: {full:?}");
        // The windowed average wins over one (bursty) frame interval.
        let averaged = session.debug_lines(&json!({
            "fps": 143.6, "submitted_frame_interval_ms": 3.0, "frame_wall_ms": 2.0,
        }));
        assert!(averaged[0].starts_with("144 fps"), "fps line: {averaged:?}");
        assert!(
            full.iter().any(|l| l == "draws: 42 indices: 1000"),
            "{full:?}"
        );
        assert!(full.iter().any(|l| l.starts_with("XYZ: ")), "{full:?}");
        assert!(
            full.iter().any(|l| l.starts_with("Biome: unknown")),
            "{full:?}"
        );
        let bare = session.debug_lines(&json!({}));
        assert!(
            bare.iter().all(|l| !l.contains("fps")),
            "no fabricated fps: {bare:?}"
        );
        let initial = session.debug_lines(&json!({"submitted_frame_interval_ms": 3.0}));
        assert!(initial.iter().all(|line| !line.contains("fps")), "{initial:?}");
        assert!(bare.iter().all(|l| !l.starts_with("draws:")), "{bare:?}");
        assert!(bare.iter().any(|l| l.starts_with("XYZ: ")), "{bare:?}");
        assert!(bare.len() <= 16, "{bare:?}");
    }
    #[test]
    fn rc1_tab_roster_packet_visibility_and_release() {
        use crate::protocol::{packet_ids::play::clientbound as cb, network::Phase, Writer};
        use crate::api::InputIntent;

        let mut session = Session::new_mode("tab-test".into(), false);
        let mut content = crate::protocol::content::Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        // Authored empty-provider font exercises real missing-glyph layout without
        // depending on Mojang assets (the same fixture as render_nameplates).
        content.insert_test_font("minecraft:default", &serde_json::json!({"providers":[]}));
        content.insert_test_resource("assets/minecraft/lang/en_us.json", b"{}");
        session.ui = Some(crate::ui::UiState::new(&content).unwrap());
        session.content = Some(content);
        session.phase = "play".into();
        let mut add = Writer::default();
        add.byte(0x09); // add player, update listed
        add.var(2);
        for (id, name) in [(1u8, "WWWWWWWWWWWWWWWW"), (2, "Alice")] {
            add.0.extend([id; 16]);
            add.string(name);
            add.var(0);
            add.byte(1);
        }
        session.apply(Phase::Play, cb::PLAYER_INFO_UPDATE, &add.0).unwrap();
        let mut header = Writer::default();
        for text in ["Roster header\nSecond line", "Roster footer\nSecond line"] {
            header.byte(8); // network NBT string component
            header.0.extend((text.len() as u16).to_be_bytes());
            header.0.extend(text.as_bytes());
        }
        session.apply(Phase::Play, cb::TAB_LIST, &header.0).unwrap();
        session.sync_ui_state(true).unwrap();
        assert!(!session.ui.as_ref().unwrap().hud.player_list_visible);
        session.input(InputIntent::Key { key: "player_list".into(), pressed: true }).unwrap();
        session.sync_ui_state(true).unwrap();
        let ui = session.ui.as_mut().unwrap();
        assert!(ui.hud.player_list_visible);
        assert_eq!(ui.hud.players.len(), 2);
        ui.rebuild_for_test(&session.inventory, session.content.as_ref().unwrap()).unwrap();
        let nodes = &ui.nodes;
        let header = nodes.iter().find(|n| n.id == "tab/header").unwrap();
        let footer = nodes.iter().find(|n| n.id == "tab/footer").unwrap();
        assert_eq!(header.label, "Roster header\nSecond line");
        assert_eq!(footer.label, "Roster footer\nSecond line");
        let names: Vec<_> = nodes.iter().filter(|n| n.id.starts_with("tab/player/")).map(|n| n.label.as_str()).collect();
        assert_eq!(names, ["Alice 0ms", "WWWWWWWWWWWWWWWW 0ms"]);
        let long = nodes.iter().find(|n| n.label == "WWWWWWWWWWWWWWWW 0ms").unwrap();
        let natural_width = ui.text.layout(long.component.as_ref().unwrap(), f32::MAX).unwrap().width;
        assert!(long.bounds[2] >= natural_width);
        assert!((long.bounds[0] + long.bounds[2] / 2. - ui.width / 2.).abs() < 0.01);
        let alice = nodes.iter().find(|n| n.label == "Alice 0ms").unwrap();
        assert!(alice.bounds[1] + alice.bounds[3] <= long.bounds[1]);
        assert!(header.bounds[1] + header.bounds[3] + 2. <= alice.bounds[1]);
        assert!(long.bounds[1] + long.bounds[3] + 2. <= footer.bounds[1]);
        assert!(header.bounds[3] > 12. && footer.bounds[3] > 12.);
        // Local 26.3 d263 ClientboundPlayerInfoUpdatePacket.java:135-140:
        // UPDATE_LISTED=3, UPDATE_DISPLAY_NAME=5 (nullable component).
        let mut update = Writer::default();
        update.byte(0x28);
        update.var(1);
        update.0.extend([1u8; 16]);
        update.byte(0);
        update.byte(1);
        update.byte(8);
        update.0.extend(5u16.to_be_bytes());
        update.0.extend(b"Alias");
        session.apply(Phase::Play, cb::PLAYER_INFO_UPDATE, &update.0).unwrap();
        session.sync_ui_state(true).unwrap();
        assert_eq!(session.ui.as_ref().unwrap().hud.players.len(), 1);
        let mut relist = Writer::default();
        relist.byte(0x08);
        relist.var(1);
        relist.0.extend([1u8; 16]);
        relist.byte(1);
        session.apply(Phase::Play, cb::PLAYER_INFO_UPDATE, &relist.0).unwrap();
        session.sync_ui_state(true).unwrap();
        let ui = session.ui.as_mut().unwrap();
        ui.rebuild_for_test(&session.inventory, session.content.as_ref().unwrap()).unwrap();
        assert!(ui.nodes.iter().any(|n| n.id.starts_with("tab/player/") && n.label == "Alias 0ms"));
        session.input(InputIntent::Key { key: "player_list".into(), pressed: false }).unwrap();
        session.sync_ui_state(true).unwrap();
        assert!(!session.ui.as_ref().unwrap().hud.player_list_visible);
        session.input(InputIntent::Key { key: "player_list".into(), pressed: true }).unwrap();
        session.input(InputIntent::ReleaseAll).unwrap();
        session.sync_ui_state(true).unwrap();
        assert!(!session.ui.as_ref().unwrap().hud.player_list_visible);
        let ui = session.ui.as_mut().unwrap();
        ui.rebuild_for_test(&session.inventory, session.content.as_ref().unwrap()).unwrap();
        assert!(!ui.nodes.iter().any(|n| n.id.starts_with("tab/")));
        session.input(InputIntent::Key { key: "player_list".into(), pressed: true }).unwrap();
        let mut remove = Writer::default();
        remove.var(1);
        remove.0.extend([2u8; 16]);
        session.apply(Phase::Play, cb::PLAYER_INFO_REMOVE, &remove.0).unwrap();
        session.sync_ui_state(true).unwrap();
        assert_eq!(session.ui.as_ref().unwrap().hud.players.len(), 1);
    }
    #[test]
    fn play_observation_waits_for_received_spawn_world() {
        let mut session = Session::new_mode("world-readiness-test".into(), false);
        session.phase = "play".into();
        let mut content = crate::protocol::content::Content::test_cube();
        content.creative_tabs_report = std::sync::Arc::from(br#"{"tabs":[]}"#.as_slice());
        session.ui =
            Some(crate::ui::UiState::new_state(&content).expect("state-only UI must construct"));
        session.player.pose_known = true;
        session.keys.insert("forward".into());
        assert!(!session.play_world_presentation_ready());
        session
            .sync_ui_state(false)
            .expect("unready world must show loading");
        assert!(session.keys.is_empty(), "loading must release held input");
        assert_eq!(
            session.minecraft_observation()["in_play"].as_bool(),
            Some(false)
        );
    }
    #[test]
    fn renderer_confirmation_is_required_and_invalidated_by_epochs() {
        let mut session = Session::new_mode("mesh-readiness-test".into(), true);
        assert!(!session.spawn_meshes_presented());
        let epochs = session.context_epochs();
        session.note_renderer_scene_confirmation(epochs.0 + 1, epochs.1, 1);
        assert!(!session.spawn_meshes_presented(), "stale renderer receipt must not release loading");
        session.note_renderer_scene_confirmation(epochs.0, epochs.1, 1);
        assert!(session.spawn_meshes_presented());
        session.scene.revision += 1;
        assert!(session.spawn_meshes_presented(), "later UI frames must not relock loading");
        session.dimension_epoch += 1;
        assert!(!session.spawn_meshes_presented(), "respawn needs a fresh presented frame");
        session.presentation = false;
        assert!(session.spawn_meshes_presented(), "headless paths have no renderer");
    }
}
impl Session {
    /// Block-break crack overlays (vanilla `LevelRenderer.destroyingBlocks` +
    /// `CrumblingOverlay` equivalent). Merges server `ClientboundBlockDestruction`
    /// stages with the local mining stage: remote breakers on one position merge
    /// by greatest stage, the local mining stage wins on its own position.
    /// Re-renders each target block's baked quads into
    /// `EntityMaterial::Crumbling` with the `minecraft:block/destroy_stage_N`
    /// texture. Missing stage textures skip without error (one `event` per newly
    /// missing stage); air/unknown blocks are lazily skipped and pruned here.
    /// Cap: 64 blocks/frame. Safe `&mut self` method: `Content` is re-borrowed
    /// from `self.content` per lookup and never aliased against `&mut self`.
    pub(super) fn append_destruction_overlays(&mut self) -> Result<()> {
        let mut stages: BTreeMap<[i32; 3], u8> = BTreeMap::new();
        for (_, (position, stage)) in &self.destruction {
            stages
                .entry(*position)
                .and_modify(|kept| *kept = (*kept).max(*stage))
                .or_insert(*stage);
        }
        if let Some((position, stage)) = self.player.breaking_crack()
            && (0..=9).contains(&stage)
        {
            stages.insert(position, stage as u8);
        }
        if stages.is_empty() {
            return Ok(());
        }
        let origin = self.scene.origin;
        // Face outward step per baked-quad face id (matches DIRECTIONS order).
        const FACE_STEP: [[i32; 3]; 6] = [
            [0, -1, 0],
            [0, 1, 0],
            [0, 0, -1],
            [0, 0, 1],
            [-1, 0, 0],
            [1, 0, 0],
        ];
        let mut missing: Vec<u8> = Vec::new();
        let mut gone: Vec<[i32; 3]> = Vec::new();
        for (position, stage) in stages.iter().take(64) {
            let stage = *stage;
            let Some(state) = self.world.block(*position) else {
                gone.push(*position);
                continue;
            };
            let content = self.content.as_ref().ok_or("content unavailable")?;
            if content.block_is_air(state).unwrap_or(true) {
                gone.push(*position);
                continue;
            }
            let name = format!("minecraft:block/destroy_stage_{stage}");
            let Some(region) = content.texture(&name) else {
                if !missing.contains(&stage) {
                    missing.push(stage);
                }
                continue;
            };
            let Some(model) = content.model_for(state)? else {
                continue;
            };
            let offset = content.render_offset(state, *position);
            let emission = content
                .info(state)
                .map(|info| info.light_emission)
                .unwrap_or(0);
            // Crack lighting must match the lit outside of the solid block,
            // never its dark interior: sample the outward-neighbor cell of the
            // quad's own face (vanilla terrain samples the adjacent cell for
            // boundary faces), floored by the block cell with emission.
            let own = {
                let [sky, block] = self.world.light_levels(*position);
                match sky.zip(block) {
                    Some((sky, block)) => [f32::from(sky), f32::from(block.max(emission))],
                    None => continue,
                }
            };
            let mut failed: Option<String> = None;
            let dynamic = &mut self.scene.dynamic;
            let world = &self.world;
            model.for_each_quad(*position, |quad| {
                if failed.is_some() {
                    return;
                }
                let uv = crack_quad_uv(&quad.uv, region.uv);
                let step = FACE_STEP[usize::from(quad.face.min(5))];
                let at = [
                    position[0] + step[0],
                    position[1] + step[1],
                    position[2] + step[2],
                ];
                let [sky, block] = world.light_levels(at);
                let light = sky.zip(block).map_or(own, |(sky, block)| {
                    [own[0].max(f32::from(sky)), own[1].max(f32::from(block))]
                });
                let positions = quad.positions.map(|p| {
                    [
                        (p[0] + position[0] as f32 + offset[0]) - origin[0] as f32,
                        (p[1] + position[1] as f32 + offset[1]) - origin[1] as f32,
                        (p[2] + position[2] as f32 + offset[2]) - origin[2] as f32,
                    ]
                });
                if let Err(error) = crate::entities::presentation_emit(
                    positions,
                    uv,
                    [1.; 3],
                    region.layer,
                    crate::api::EntityMaterial::Crumbling,
                    1.,
                    light,
                    dynamic,
                ) {
                    failed = Some(error);
                }
            });
            if let Some(error) = failed {
                return Err(error);
            }
        }
        for stage in missing {
            self.event(
                json!({"type":"destruction_texture_missing","stage":stage,"origin":"verified_packs"}),
            );
        }
        if !gone.is_empty() {
            let gone_set: std::collections::BTreeSet<[i32; 3]> = gone.into_iter().collect();
            self.destruction
                .retain(|_, (pos, _)| !gone_set.contains(pos));
            self.changed_state();
        }
        Ok(())
    }
}

/// Maps a baked quad's atlas-space UVs into a destroy-stage region: normalize
/// into the quad's own sprite 0..1 range, then into `stage_uv`. Axes stay
/// independent (the old fold destructured `([umin, umax], [vmin, vmax])` while
/// accumulating `([umin, vmin], [umax, vmax])`, crossing axes on non-square
/// UV bounds).
fn crack_quad_uv(quad_uv: &[[f32; 2]; 4], stage_uv: [f32; 4]) -> [[f32; 2]; 4] {
    let (mut umin, mut umax) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut vmin, mut vmax) = (f32::INFINITY, f32::NEG_INFINITY);
    for [u, v] in quad_uv {
        umin = umin.min(*u);
        umax = umax.max(*u);
        vmin = vmin.min(*v);
        vmax = vmax.max(*v);
    }
    quad_uv.map(|[u, v]| {
        let tu = if umax - umin < f32::EPSILON {
            0.5
        } else {
            (u - umin) / (umax - umin)
        };
        let tv = if vmax - vmin < f32::EPSILON {
            0.5
        } else {
            (v - vmin) / (vmax - vmin)
        };
        [
            stage_uv[0] + tu * (stage_uv[2] - stage_uv[0]),
            stage_uv[1] + tv * (stage_uv[3] - stage_uv[1]),
        ]
    })
}

#[cfg(test)]
mod destruction_tests {
    use super::*;
    use crate::protocol::content::TextureRegion;
    use std::sync::Arc;

    fn session_with_cube(position: [i32; 3], light: bool) -> Session {
        let mut session = Session::new_mode("destruction-test".into(), false);
        session.content = Some(Content::test_cube());
        session.world = World::new(0, 16);
        let uniform = |size| crate::protocol::world::Palette {
            bits: 0,
            values: vec![0],
            words: Vec::new(),
            size,
        };
        let mut sky = BTreeMap::new();
        let mut block = BTreeMap::new();
        if light {
            // light_levels indexes layer (section + 1) with 2048 packed nibbles.
            sky.insert(1, Arc::new(vec![0xFF; 2048]));
            block.insert(1, Arc::new(vec![0xFF; 2048]));
        }
        session
            .world
            .insert(
                [position[0].div_euclid(16), position[2].div_euclid(16)],
                crate::protocol::world::Chunk {
                    sections: vec![Arc::new(crate::protocol::world::Section {
                        blocks: uniform(4096),
                        biomes: uniform(64),
                        non_empty_block_count: 0,
                    })],
                    sky,
                    block,
                    block_entities: crate::protocol::world::empty_block_entities(),
                    surface_heightmap: Arc::new(
                        crate::protocol::world::SurfaceHeightmap::new(16).unwrap(),
                    ),
                    surface_heightmap_received: false,
                },
            )
            .unwrap();
        session
            .world
            .set(position, 1, |state| Ok(state == 0))
            .unwrap();
        session
    }

    fn with_stage_texture(session: &mut Session, stage: u8) {
        session.content.as_mut().unwrap().insert_test_texture(
            &format!("minecraft:block/destroy_stage_{stage}"),
            TextureRegion {
                uv: [0., 0., 1., 1.],
                layer: 0,
                width: 16,
                height: 16,
            },
        );
    }

    fn overlay(session: &mut Session) -> Result<()> {
        session.append_destruction_overlays()
    }

    fn crumbling_vertices(session: &Session) -> Vec<crate::api::Vertex> {
        let family = crate::api::EntityMaterial::Crumbling as usize;
        let indices = &session.scene.dynamic.indices[family];
        // Emit order per quad is [first, first+1, first+2, first+2, first+3,
        // first]; gather the four unique vertices via [0, 1, 2, 4].
        indices
            .chunks(6)
            .flat_map(|quad| {
                [quad[0], quad[1], quad[2], quad[4]]
                    .into_iter()
                    .map(|index| session.scene.dynamic.vertices[index as usize])
            })
            .collect()
    }

    #[test]
    fn stage_formula_progress_maps_to_crack_stage() {
        // breaking_observation stage formula: (progress * 10) as i32 - 1.
        assert_eq!((0.35f32 * 10.) as i32 - 1, 2);
        assert_eq!((0.05f32 * 10.) as i32 - 1, -1);
    }

    #[test]
    fn server_stage_emits_crumbling_quads() {
        let position = [0, 3, 0];
        let mut session = session_with_cube(position, true);
        with_stage_texture(&mut session, 2);
        session.destruction.insert(9, (position, 2));
        overlay(&mut session).unwrap();
        let family = crate::api::EntityMaterial::Crumbling as usize;
        // test_cube state 1 bakes 6 face quads.
        assert_eq!(session.scene.dynamic.indices[family].len(), 6 * 6);
        assert_eq!(session.scene.dynamic.vertices.len(), 6 * 4);
        assert!(
            session
                .scene
                .dynamic
                .vertices
                .iter()
                .all(|v| v.texture_layer == 0)
        );
    }

    #[test]
    fn missing_texture_skips_without_error() {
        let position = [0, 3, 0];
        let mut session = session_with_cube(position, true);
        session.destruction.insert(9, (position, 4));
        overlay(&mut session).unwrap();
        let family = crate::api::EntityMaterial::Crumbling as usize;
        assert!(session.scene.dynamic.indices[family].is_empty());
        assert_eq!(session.events.len(), 1);
    }
    #[test]
    fn air_block_prunes_server_entry() {
        let position = [0, 3, 0];
        let mut session = session_with_cube(position, true);
        with_stage_texture(&mut session, 1);
        session.destruction.insert(9, (position, 1));
        session
            .world
            .set(position, 0, |state| Ok(state == 0))
            .unwrap();
        overlay(&mut session).unwrap();
        assert!(session.destruction.is_empty());
        let family = crate::api::EntityMaterial::Crumbling as usize;
        assert!(session.scene.dynamic.indices[family].is_empty());
    }

    #[test]
    fn remote_breakers_merge_by_greatest_stage() {
        let position = [0, 3, 0];
        let mut session = session_with_cube(position, true);
        for (stage, left) in [(2u8, 0.0f32), (7u8, 0.5f32)] {
            session.content.as_mut().unwrap().insert_test_texture(
                &format!("minecraft:block/destroy_stage_{stage}"),
                TextureRegion {
                    uv: [left, 0., left + 0.5, 1.],
                    layer: 0,
                    width: 16,
                    height: 16,
                },
            );
        }
        session.destruction.insert(9, (position, 2));
        session.destruction.insert(11, (position, 7));
        overlay(&mut session).unwrap();
        let vertices = crumbling_vertices(&session);
        assert_eq!(vertices.len(), 6 * 4, "one merged overlay: {vertices:?}");
        assert!(
            vertices.iter().all(|v| (0.5..=1.0).contains(&v.uv[0])),
            "greatest stage wins: {vertices:?}"
        );
    }

    #[test]
    fn non_square_quad_corners_reach_distinct_stage_corners() {
        // Narrow-U / tall-V quad: the crossed-axis fold mapped both U and V
        // through one shared span, collapsing opposite corners together.
        let quad = [[0.1, 0.2], [0.1, 0.9], [0.3, 0.9], [0.3, 0.2]];
        let stage = [0., 0., 1., 1.];
        let mapped = crack_quad_uv(&quad, stage);
        assert_eq!(mapped[0], [0., 0.], "lower-left: {mapped:?}");
        assert_eq!(mapped[1], [0., 1.], "upper-left: {mapped:?}");
        assert_eq!(mapped[2], [1., 1.], "upper-right: {mapped:?}");
        assert_eq!(mapped[3], [1., 0.], "lower-right: {mapped:?}");
        let position = [0, 3, 0];
        let mut session = session_with_cube(position, true);
        with_stage_texture(&mut session, 1);
        session.destruction.insert(9, (position, 1));
        overlay(&mut session).unwrap();
        let vertices = crumbling_vertices(&session);
        assert_eq!(vertices.len(), 6 * 4);
        assert!(
            vertices.iter().any(|v| v.uv[0] < 0.25) && vertices.iter().any(|v| v.uv[0] > 0.75),
            "U spans the stage region: {vertices:?}"
        );
        assert!(
            vertices.iter().any(|v| v.uv[1] < 0.25) && vertices.iter().any(|v| v.uv[1] > 0.75),
            "V spans the stage region: {vertices:?}"
        );
    }

    #[test]
    fn crack_uses_lit_outside_not_dark_interior() {
        let position = [0, 3, 0];
        let mut session = session_with_cube(position, false);
        with_stage_texture(&mut session, 3);
        session.destruction.insert(9, (position, 3));
        // Dark interior cell (sky 0 / block 0) with only the +Y neighbor lit
        // (sky 15 / block 15): top-face cracks render lit, the dark -Y face
        // keeps the interior floor. block_index([0,4,0]) = 1024 -> byte 512.
        let mut sky = BTreeMap::new();
        let mut block = BTreeMap::new();
        let mut both = vec![0u8; 2048];
        both[512] = 0xFF;
        sky.insert(1, Arc::new(both.clone()));
        block.insert(1, Arc::new(both));
        session
            .world
            .insert(
                [position[0].div_euclid(16), position[2].div_euclid(16)],
                crate::protocol::world::Chunk {
                    sections: vec![Arc::new(crate::protocol::world::Section {
                        blocks: crate::protocol::world::Palette {
                            bits: 0,
                            values: vec![0],
                            words: Vec::new(),
                            size: 4096,
                        },
                        biomes: crate::protocol::world::Palette {
                            bits: 0,
                            values: vec![0],
                            words: Vec::new(),
                            size: 64,
                        },
                        non_empty_block_count: 0,
                    })],
                    sky,
                    block,
                    block_entities: crate::protocol::world::empty_block_entities(),
                    surface_heightmap: Arc::new(
                        crate::protocol::world::SurfaceHeightmap::new(16).unwrap(),
                    ),
                    surface_heightmap_received: false,
                },
            )
            .unwrap();
        session
            .world
            .set(position, 1, |state| Ok(state == 0))
            .unwrap();
        overlay(&mut session).unwrap();
        let vertices = crumbling_vertices(&session);
        assert_eq!(vertices.len(), 6 * 4);
        let lit = vertices.iter().filter(|v| v.light == [15., 15.]).count();
        let dark = vertices.iter().filter(|v| v.light == [0., 0.]).count();
        assert_eq!(lit, 4, "only the +Y face sees the lit neighbor: {vertices:?}");
        assert_eq!(dark, 5 * 4, "other faces keep the dark interior floor: {vertices:?}");
    }
}
fn vector([x, y, z]: [f64; 3]) -> Value {
    json!({"x":x,"y":y,"z":z})
}
fn menu_class(kind: MenuKind) -> &'static str {
    match kind {
        MenuKind::Inventory => "net.minecraft.world.inventory.InventoryMenu",
        MenuKind::Chest(_) => "net.minecraft.world.inventory.ChestMenu",
        MenuKind::Dispenser => "net.minecraft.world.inventory.DispenserMenu",
        MenuKind::Crafter => "net.minecraft.world.inventory.CrafterMenu",
        MenuKind::Anvil => "net.minecraft.world.inventory.AnvilMenu",
        MenuKind::Beacon => "net.minecraft.world.inventory.BeaconMenu",
        MenuKind::BlastFurnace => "net.minecraft.world.inventory.BlastFurnaceMenu",
        MenuKind::BrewingStand => "net.minecraft.world.inventory.BrewingStandMenu",
        MenuKind::Crafting => "net.minecraft.world.inventory.CraftingMenu",
        MenuKind::Enchantment => "net.minecraft.world.inventory.EnchantmentMenu",
        MenuKind::Furnace => "net.minecraft.world.inventory.FurnaceMenu",
        MenuKind::Grindstone => "net.minecraft.world.inventory.GrindstoneMenu",
        MenuKind::Hopper => "net.minecraft.world.inventory.HopperMenu",
        MenuKind::Lectern => "net.minecraft.world.inventory.LecternMenu",
        MenuKind::Loom => "net.minecraft.world.inventory.LoomMenu",
        MenuKind::Merchant => "net.minecraft.world.inventory.MerchantMenu",
        MenuKind::ShulkerBox => "net.minecraft.world.inventory.ShulkerBoxMenu",
        MenuKind::Smithing => "net.minecraft.world.inventory.SmithingMenu",
        MenuKind::Smoker => "net.minecraft.world.inventory.SmokerMenu",
        MenuKind::Cartography => "net.minecraft.world.inventory.CartographyTableMenu",
        MenuKind::Stonecutter => "net.minecraft.world.inventory.StonecutterMenu",
        MenuKind::Horse => "net.minecraft.world.inventory.HorseInventoryMenu",
    }
}
