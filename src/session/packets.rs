use crate::protocol::packet_ids::play::serverbound as sb;
use crate::protocol::packet_ids::play::clientbound as cb;
use crate::protocol::packet_ids::{login, configuration};
use super::*;
use crate::protocol::{self, world::Chunk, Reader};
impl Session {
    fn initialize_player_attributes(&mut self) -> Result<()> {
        let schema = self
            .entities
            .models_ref()
            .and_then(|models| models.schemas.get("minecraft:player"))
            .ok_or("local player DefaultAttributes supplier unavailable")?;
        self.player.init_vanilla_defaults(
            schema
                .default_attributes
                .iter()
                .map(|(name, attribute)| (name.as_str(), attribute.value)),
        );
        Ok(())
    }
    pub(super) fn apply(&mut self, phase: Phase, id: i32, bytes: &[u8]) -> Result<()> {
        let mut r = Reader::new(bytes);
        match phase {
            Phase::Login => match id {
                login::clientbound::LOGIN_DISCONNECT => return Err(format!("login disconnect: {}", r.string(32767)?)),
                login::clientbound::LOGIN_FINISHED => {
                    let uuid = uuid::Uuid::from_slice(r.take(16)?).map_err(|error| error.to_string())?.to_string();
                    self.player.uuid = Some(Arc::from(uuid));
                    r.string(16)?;
                    for _ in 0..r.count(16)? { r.string(32767)?; r.string(32767)?; if r.bool()? { r.string(32767)?; } }
                    uuid::Uuid::from_slice(r.take(16)?).map_err(|error| error.to_string())?;
                    r.done()?; self.phase = "configuration".into();
                    eprintln!("session startup: configuration");
                }
                login::clientbound::LOGIN_COMPRESSION => { r.count(protocol::MAX_FRAME)?; r.done()?; }
                login::clientbound::CUSTOM_QUERY | login::clientbound::COOKIE_REQUEST => {}
                _ => return Err(format!("unsupported mandatory Login packet {id}")),
            },
            Phase::Configuration => match id {
                configuration::clientbound::DISCONNECT => return Err(format!("configuration disconnect: {}", r.nbt()?)),
                configuration::clientbound::FINISH_CONFIGURATION => {
                    r.done()?;
                    let content = self.content.as_mut().ok_or("content unavailable")?;
                    content.set_registries(std::mem::take(&mut self.registries));
                    content.set_tags(std::mem::take(&mut self.registry_tags))?;
                    content.set_enabled_features(self.features.clone())?;
                    self.entities.configure_interaction_tags(content)?;
                    self.player.set_enabled_features(self.features.clone());
                    self.inventory.set_enabled_features(self.features.clone());
                    if let Some(ui) = &mut self.ui { ui.set_enabled_features(self.features.clone()); }
                    self.phase = "play".into();
                    eprintln!("session startup: play");
                }
                configuration::clientbound::REGISTRY_DATA => {
                    let registry = r.string(32767)?;
                    let n = r.count(65536)?;
                    if self.registries.len() >= 128 && !self.registries.contains_key(&registry) { return Err("registry collection bound".into()); }
                    let mut entries = Vec::with_capacity(n);
                    for _ in 0..n {
                        let name = r.string(32767)?;
                        if !r.bool()? { return Err(format!("registry entry {name} omitted despite empty known-pack response")); }
                        entries.push((name, r.nbt()?));
                    }
                    r.done()?;
                    self.environment.configure_registry(&registry, &entries)?;
                    self.entities.configure_registry(&registry, &entries)?;
                    self.registries.insert(registry, entries);
                }
                configuration::clientbound::UPDATE_ENABLED_FEATURES => {
                    let mut features = BTreeSet::new();
                    for _ in 0..r.count(1024)? {
                        let feature = r.string(32767)?;
                        if feature != "minecraft:vanilla" { return Err(format!("unsupported mandatory feature flag {feature}")); }
                        features.insert(feature);
                    }
                    r.done()?; self.features = Arc::new(features);
                }
                configuration::clientbound::UPDATE_TAGS => {
                    let tags = protocol::read_registry_tags(&mut r)?;
                    for (registry, values) in &tags { self.environment.configure_tags(registry, values)?; }
                    self.registry_tags = tags;
                }
                configuration::clientbound::SELECT_KNOWN_PACKS => { for _ in 0..r.count(1024)? { r.string(32767)?; r.string(32767)?; r.string(32767)?; } r.done()?; }
                configuration::clientbound::KEEP_ALIVE => { r.i64()?; r.done()?; }
                configuration::clientbound::PING => { r.i32()?; r.done()?; }
                configuration::clientbound::COOKIE_REQUEST | configuration::clientbound::RESET_CHAT => {}
                configuration::clientbound::CUSTOM_PAYLOAD => self.custom_payload(&mut r)?,
                configuration::clientbound::RESOURCE_PACK_POP | configuration::clientbound::STORE_COOKIE | configuration::clientbound::CUSTOM_REPORT_DETAILS | configuration::clientbound::SERVER_LINKS | configuration::clientbound::CLEAR_DIALOG => { *self.ignored_presentation.entry(id).or_default() += 1; }
                configuration::clientbound::POST_EFFECTS => {
                    for _ in 0..r.count(1024)? {
                        r.string(32767)?;
                    }
                    r.done()?;
                    *self.ignored_presentation.entry(id).or_default() += 1;
                }
                configuration::clientbound::RESOURCE_PACK_PUSH => return Err("server resource-pack activation unsupported; refusing ambiguous mandatory content".into()),
                configuration::clientbound::TRANSFER => return Err("server transfer unsupported".into()),
                configuration::clientbound::SHOW_DIALOG | configuration::clientbound::CODE_OF_CONDUCT => return Err("configuration dialog/code-of-conduct requires unsupported human UI".into()),
                _ => return Err(format!("unknown mandatory Configuration packet {id}")),
            },
            Phase::Play => self.play(id, &mut r)?,
        }
        Ok(())
    }
    fn custom_payload(&mut self, r: &mut Reader<'_>) -> Result<()> {
        let channel = r.string(32767)?;
        if channel.starts_with("solaris:loader") {
            let payload = r.take(r.bytes.len() - r.offset)?;
            match channel.as_str() {
                "solaris:loader/probe" if self.phase == "configuration" => {
                    if !payload.is_empty() {
                        return Err("Loader probe carries no payload".into());
                    }
                    // The network state machine already wrote the hello in packet order.
                }
                "solaris:loader/manifest" if self.phase == "configuration" => {
                    let grants = crate::loader::Loader::local_permissions()?;
                    let outgoing = self.loader.begin(payload, &grants)?;
                    self.send_loader(outgoing)?;
                }
                "solaris:loader/artifact" if self.phase == "configuration" => {
                    if let Some(outgoing) = self.loader.artifact(payload)? {
                        let activated = outgoing.channel == "solaris:loader/ack";
                        if activated {
                            self.activate_loader_content()?;
                            self.event(json!({"type":"loader_activated","platform":"native","origin":"hash_verified_received_bundle"}));
                        }
                        self.send_loader(outgoing)?;
                    }
                }
                "solaris:loader/view" if self.phase == "play" => {
                    let ui = self
                        .ui
                        .as_mut()
                        .ok_or("Loader view requires active UI state")?;
                    match self.loader.view(payload, ui, self.network.is_some()) {
                        Ok(message) => {
                            self.keys.clear();
                            self.event(json!({"type":"loader_view_received","message":message,"origin":"server_observed"}));
                        }
                        Err(error) => self.event(json!({"type":"loader_view_rejected","error":error,"origin":"received_optional_payload"})),
                    }
                }
                "solaris:loader/sound" => {
                    match self.loader.sound(
                        payload,
                        self.phase == "play" && self.network.is_some(),
                        self.client_tick,
                    ) {
                        Ok(command) => {
                            let mode = match &command.request {
                                None => "stop",
                                Some(request)
                                    if matches!(
                                        request.source,
                                        crate::audio::SoundOrigin::Relative
                                    ) =>
                                {
                                    "relative"
                                }
                                Some(_) => "position",
                            };
                            let outcome = match (&self.audio, command.request) {
                                (Some(audio), Some(request)) => audio
                                    .play_verified(request, command.clip)
                                    .map(|handle| json!({"phase":"queued","handle":handle,"audible_output_verified":false})),
                                (Some(audio), None) => audio
                                    .stop_sound(Some(crate::audio::SoundCategory::Master), Some(&command.id))
                                    .map(|_| json!({"phase":"stop_queued","audible_output_verified":false})),
                                (None, _) => Ok(json!({"phase":"decoded_no_audio_output","reason":"state_only_session","audible_output_verified":false})),
                            };
                            match outcome {
                                Ok(outcome) => self.event(json!({"type":"loader_sound_received","sound":command.id,
                                    "mode":mode,"outcome":outcome,"origin":"hash_verified_loader_asset"})),
                                Err(error) => self.event(json!({"type":"loader_sound_playback_rejected","sound":command.id,
                                    "error":error,"origin":"received_optional_payload"})),
                            }
                        }
                        Err(error) => self
                            .event(json!({"type":"loader_sound_rejected","error":error,
                            "origin":"received_optional_payload"})),
                    }
                }
                _ => return Err(format!("unsupported Loader channel or phase: {channel}")),
            }
        } else if channel.starts_with("solaris:lod/") && self.phase == "play" {
            let payload = r.take(r.bytes.len() - r.offset)?;
            let outcome = match channel.as_str() {
                "solaris:lod/hello" => self.far.hello(payload),
                "solaris:lod/program" => self.far.program(payload),
                "solaris:lod/tile" => match self.content.as_ref() {
                    Some(content) => self.far.tile(payload, content),
                    None => Ok(()),
                },
                _ => Ok(()),
            };
            // Far terrain is optional presentation: a bad payload is reported, never fatal.
            if let Err(error) = outcome {
                self.event(
                    json!({"type":"far_terrain_payload_rejected","channel":channel,"error":error}),
                );
            }
        } else if channel != "minecraft:brand" {
            self.event(json!({"type":"optional_custom_payload_ignored","channel":channel}));
        }
        Ok(())
    }
    /// Fold one received column into the far-terrain store (optional presentation).
    fn ingest_far_chunk(&mut self, position: [i32; 2]) {
        let (Some(chunk), Some(content)) = (self.world.chunk(position), self.content.as_ref())
        else {
            return;
        };
        if let Err(error) = self
            .far
            .ingest_chunk(position, chunk, self.world.min_y, content)
        {
            self.event(json!({"type":"far_terrain_ingest_failed","error":error}));
        }
    }
    fn play(&mut self, id: i32, r: &mut Reader<'_>) -> Result<()> {
        if id == cb::OPEN_SIGN_EDITOR {
            let position = protocol::unpack_pos(r.i64()?);
            let front = r.var()? == 1;
            r.done()?;
            // Admit the editor before inventory.apply changes screen/fence state.
            // Vanilla ignores an invalid entity without touching the current menu.
            let opened = match self.open_sign_editor(position, front) {
                Ok(opened) => opened,
                Err(error) => {
                    self.player.warn_local_prediction(None, "Sign editor", &error);
                    false
                }
            };
            if !opened {
                return Ok(());
            }
        }
        // Independently decoded by each owner: overlapping local/entity packets update both.
        // A false return leaves that owner's reader untouched, never hides mandatory bytes.
        let content = self
            .content
            .as_ref()
            .ok_or("Play before content activation")?;
        let mut handled = false;
        if self
            .environment
            .apply(id, &mut Reader::new(r.bytes), content, self.client_tick)?
        {
            self.environment.publish_world(&mut self.world)?;
            if id == cb::SET_TIME {
                self.player.update_world_time(self.environment.game_time(), self.client_tick);
            }
            self.server_frozen = self.environment.frozen;
            if id == cb::TICKING_STEP {
                self.step_ticks = Reader::new(r.bytes).var()?;
            }
            handled = true;
        }
        if self
            .player
            .apply(id, &mut Reader::new(r.bytes), content, self.client_tick)?
        {
            if id == cb::SET_HEALTH || id == cb::PLAYER_COMBAT_KILL {
                self.health_version += 1;
            }
            self.player.drain_commands(&mut self.commands);
            handled = true;
        }
        if self
            .inventory
            .apply(id, &mut Reader::new(r.bytes), content)?
        {
            self.container_packet_version += 1;
            handled = true;
        }
        if self
            .entities
            .apply(id, &mut Reader::new(r.bytes), content, self.client_tick)?
        {
            handled = true;
        }
        if let Some(ui) = &mut self.ui {
            if ui.apply(id, &mut Reader::new(r.bytes), content)? {
                handled = true;
            }
        }
        if handled {
            if id == cb::BLOCK_CHANGED_ACK {
                self.reconcile_ack(Reader::new(r.bytes).var()?)?;
            }
            if !self.commands.is_empty() {
                self.flush_commands()?;
            }
            self.consume_ui_packet_commands()?;
            return Ok(());
        }
        match id {
            cb::BUNDLE_DELIMITER | cb::CHUNK_BATCH_START | cb::KEEP_ALIVE => {}
            cb::PING => { r.i32()?; r.done()?; } // Pong is sent immediately by the socket owner.
            cb::DISCONNECT => return Err(format!("Play disconnect: {}", r.nbt()?)),
            cb::LOGIN => {
                self.player.entity_id = r.i32()?;
                self.entities.own_id = Some(self.player.entity_id);
                r.bool()?;
                for _ in 0..r.count(64)? {
                    r.string(32767)?;
                }
                r.var()?;
                r.var()?;
                r.var()?;
                r.bool()?;
                r.bool()?;
                r.bool()?;
                self.spawn_info(r)?;
                r.bool()?;
                r.bool()?;
                r.done()?;
                self.initialize_player_attributes()?;
                if let Some(uuid) = self.player.uuid.as_deref() {
                    self.entities
                        .set_local_player(self.player.entity_id, uuid)?;
                }
            }
            cb::RESPAWN => {
                self.spawn_info(r)?;
                let keep = r.u8()?;
                r.done()?;
                self.player.reset_dimension(keep & 1 != 0);
                if keep & 1 == 0 {
                    self.initialize_player_attributes()?;
                }
                self.player.update_environment_context(&self.world)?;
                self.inventory.close();
                self.keys.clear();
            }
            cb::LEVEL_CHUNK_WITH_LIGHT => {
                if self.world.height <= 0 {
                    return Err("chunk before dimension registry activated".into());
                }
                let content = self.content.as_ref().ok_or("content unavailable")?;
                let (position, chunk) =
                    Chunk::decode(r, (self.world.height / 16) as usize, |state| {
                        content.block_is_air(state)
                    })?;
                self.entities.unload_chunk(position);
                for (section_y, section) in chunk.sections.iter().enumerate() {
                    for index in 0..4096 {
                        let state = section.blocks.get(index);
                        if !content.info(state)?.is_air
                            && content.placement_metadata(state)?["block_entity_type"].is_string()
                        {
                            // A block-entity update tag is optional payload, not
                            // geometry admission. Project the block constructor
                            // before applying any actual received NBT below.
                            let block_position = [
                                position[0] * 16 + (index % 16) as i32,
                                self.world.min_y + section_y as i32 * 16 + (index / 256) as i32,
                                position[1] * 16 + (index / 16 % 16) as i32,
                            ];
                            self.entities.ensure_block_entity(
                                block_position,
                                state,
                                content,
                                self.client_tick,
                            )?;
                        }
                    }
                }
                for block in chunk.block_entities.iter() {
                    let section = (block.position[1] - self.world.min_y).div_euclid(16) as usize;
                    let index = (block.position[1].rem_euclid(16) * 256
                        + block.position[2].rem_euclid(16) * 16
                        + block.position[0].rem_euclid(16))
                        as usize;
                    let state = chunk
                        .sections
                        .get(section)
                        .map(|section| section.blocks.get(index));
                    self.entities.apply_block_entity(
                        block.position,
                        block.kind,
                        block.data.clone(),
                        block.raw_nbt.clone(),
                        state,
                        content,
                        self.client_tick,
                    )?;
                }
                self.world.insert(position, chunk)?;
                self.predicted_blocks
                    .retain(|p, _| chunk_key(*p) != position);
                self.destruction
                    .retain(|_, (pos, _)| chunk_key(*pos) != position);
                self.received_chunks.insert(position, self.packet_sequence);
                if let Some(terrain) = &mut self.terrain {
                    terrain.invalidate_chunk(&self.world, position)?;
                }
                self.ingest_far_chunk(position);
            }
            cb::LIGHT_UPDATE => {
                let position = [r.var()?, r.var()?];
                self.world.update_light(position, r)?;
                if let Some(terrain) = &mut self.terrain {
                    terrain.invalidate_chunk(&self.world, position)?;
                }
            }
            cb::FORGET_LEVEL_CHUNK => {
                let packed = r.i64()?;
                let position = [packed as i32, (packed >> 32) as i32];
                r.done()?;
                self.ingest_far_chunk(position);
                self.world.remove(position);
                self.entities.unload_chunk(position);
                self.predicted_blocks
                    .retain(|p, _| chunk_key(*p) != position);
                self.destruction
                    .retain(|_, (pos, _)| chunk_key(*pos) != position);
                self.received_blocks
                    .retain(|p, _| chunk_key(*p) != position);
                self.received_chunks.remove(&position);
                if let Some(terrain) = &mut self.terrain {
                    terrain.invalidate_chunk(&self.world, position)?;
                }
            }
            cb::BLOCK_UPDATE => {
                let position = protocol::unpack_pos(r.i64()?);
                let state = r.count(1_000_000)? as u32;
                r.done()?;
                self.receive_block(position, state)?;
            }
            cb::SECTION_BLOCKS_UPDATE => {
                let packed = r.i64()?;
                let sx = (packed >> 42) as i32;
                let sy = ((packed << 44) >> 44) as i32;
                let sz = ((packed << 22) >> 42) as i32;
                for _ in 0..r.count(4096)? {
                    let value = r.varlong()?;
                    self.receive_block(
                        [
                            sx * 16 + ((value >> 8) & 15) as i32,
                            sy * 16 + (value & 15) as i32,
                            sz * 16 + ((value >> 4) & 15) as i32,
                        ],
                        (value >> 12) as u32,
                    )?;
                }
                r.done()?;
            }
            cb::CHUNK_BATCH_FINISHED => {
                r.var()?;
                r.done()?;
                let mut w = Writer::default();
                w.f32(4.);
                self.send(sb::CHUNK_BATCH_RECEIVED, w)?;
            }
            cb::GAME_EVENT => {
                let event = r.u8()?;
                let value = r.f32()?;
                r.done()?;
                if event == 3 {
                    self.player.set_mode(value as u8)?;
                    self.inventory.game_mode = self.player.game_mode;
                } else {
                    self.event(json!({"type":"game_event","event":event,"value":value,"origin":"server_observed"}));
                }
            }
            // 26.3 ClientboundLevelEvent: raw i32, BlockPos, raw i32, bool.
            cb::LEVEL_EVENT => {
                let event = r.i32()?;
                let position = protocol::unpack_pos(r.i64()?);
                let data = r.i32()?;
                let global = r.bool()?;
                r.done()?;
                let presented = self.present_block_break(event, position, data)?;
                self.event(
                    json!({"type":"level_event","event":event,"position":position,
                    "data":data,"global":global,"origin":"server_observed",
                    "packet_sequence":self.packet_sequence.to_string(),
                    "presentation_supported":presented}),
                );
            }
            cb::SOUND => {
                let sound = self.read_sound(r)?;
                let source = r.count(16)?;
                let position = [
                    f64::from(r.i32()?) / 8.,
                    f64::from(r.i32()?) / 8.,
                    f64::from(r.i32()?) / 8.,
                ];
                let volume = r.f32()?;
                let pitch = r.f32()?;
                let seed = r.i64()? as u64;
                r.done()?;
                if let Some(audio) = &self.audio {
                    audio.play(crate::audio::SoundRequest::positional(
                        &sound,
                        crate::audio::SoundCategory::from_wire(source as i32)?,
                        position,
                        volume,
                        pitch,
                        seed,
                    ))?;
                }
                self.event(json!({"type":"sound","sound":sound,"source":source,"position":position,"origin":"server_observed"}));
            }
            cb::SOUND_ENTITY => {
                let sound = self.read_sound(r)?;
                let source = r.count(16)?;
                let entity = r.var()?;
                let volume = r.f32()?;
                let pitch = r.f32()?;
                let seed = r.i64()? as u64;
                r.done()?;
                let position = if entity == self.player.entity_id {
                    Some(self.player.position)
                } else {
                    self.entities.get(entity).map(|entity| entity.position)
                };
                if let Some(position) = position {
                    if let Some(audio) = &self.audio {
                        audio.play(crate::audio::SoundRequest::positional(
                            &sound,
                            crate::audio::SoundCategory::from_wire(source as i32)?,
                            position,
                            volume,
                            pitch,
                            seed,
                        ))?;
                    }
                } else {
                    self.event(
                        json!({"type":"sound_entity_unresolved","entity_id":entity,"sound":sound}),
                    );
                }
            }
            cb::STOP_SOUND => {
                if let Some(audio) = &self.audio {
                    audio.stop();
                }
                self.event(json!({"type":"sound_stop","policy":"all_voices_stopped_bounded_C02"}));
            }
            cb::START_CONFIGURATION => {
                self.phase = "configuration".into();
                self.loader.reset();
                self.far.disconnect();
                self.clear_loader_content()?;
                self.keys.clear();
                self.player.reset_dimension(false);
                self.world.clear();
                self.entities.clear_connection();
                self.registries.clear();
                self.registry_tags.clear();
                self.environment = Environment::new();
                self.inventory.reset();
                self.predicted_blocks.clear();
                self.destruction.clear();
                self.block_effects.clear();
                self.block_motion.clear();
                self.received_chunks.clear();
                self.received_blocks.clear();
                self.dimension = None;
                if let Some(ui) = &mut self.ui {
                    ui.clear_session();
                }
                if let Some(terrain) = &mut self.terrain {
                    terrain.reset(&mut self.scene.chunks);
                }
                self.dimension_epoch += 1;
                if let Some(audio) = &self.audio {
                    audio.stop();
                }
            }
            cb::RESOURCE_PACK_PUSH => return Err("Play resource-pack activation unsupported".into()),
            cb::TRANSFER => return Err("server transfer unsupported".into()),
            cb::COOKIE_REQUEST => {
                let key = r.string(32767)?;
                r.done()?;
                let mut w = Writer::default();
                w.string(&key);
                w.byte(0);
                self.send(sb::COOKIE_RESPONSE, w)?;
            }
            cb::CUSTOM_PAYLOAD => self.custom_payload(r)?,
            cb::STORE_COOKIE => {
                r.string(32767)?;
                r.array(5120)?;
                r.done()?;
            }
            cb::UPDATE_TAGS => {
                let tags = protocol::read_registry_tags(&mut Reader::new(r.bytes))?;
                for (registry, values) in &tags {
                    self.environment.configure_tags(registry, values)?;
                }
                let content = self.content.as_mut().ok_or("content unavailable")?;
                content.set_tags(tags)?;
                self.entities.configure_interaction_tags(content)?;
            }
            // ClientboundBlockDestruction layout (26.1.2 Play id 5, verified
            // against packet 8 handler above: varint breaker-id, BlockPos i64
            // via protocol::unpack_pos, u8 stage): store 0..=9, remove on >= 10.
            cb::BLOCK_DESTRUCTION => {
                let breaker = r.var()?;
                let position = protocol::unpack_pos(r.i64()?);
                let stage = r.u8()?;
                r.done()?;
                self.receive_destruction(breaker, position, stage);
            }
            // Explicitly counted unsupported presentation variants; never gameplay facts.
            // Id 43 (ClientboundLevelParticles, layout in ClientboundLevelParticlesPacket:
            // bools, doubles/floats, count, registry particle) is counted, not
            // rendered: particle presentation is unsupported (local use/block
            // predictions already emit unsupported_local_particle_presentation).
            cb::ANIMATE | cb::AWARD_STATS | cb::BOSS_EVENT | cb::CHANGE_DIFFICULTY | cb::CHUNKS_BIOMES | cb::CLEAR_TITLES | cb::COMMAND_SUGGESTIONS | cb::COMMANDS | cb::CUSTOM_CHAT_COMPLETIONS | cb::DEBUG_BLOCK_VALUE | cb::DEBUG_CHUNK_VALUE | cb::DEBUG_ENTITY_VALUE | cb::DEBUG_EVENT | cb::DEBUG_SAMPLE | cb::DELETE_CHAT | cb::DISGUISED_CHAT | cb::ENTITY_EVENT
            | cb::INITIALIZE_BORDER | cb::PLAYER_LOOK_AT | cb::RESET_SCORE | cb::ROTATE_HEAD | cb::SELECT_ADVANCEMENTS_TAB | cb::SERVER_DATA | cb::SET_ACTION_BAR_TEXT | cb::SET_BORDER_CENTER | cb::SET_BORDER_LERP_SIZE | cb::SET_BORDER_SIZE | cb::SET_BORDER_WARNING_DELAY | cb::SET_BORDER_WARNING_DISTANCE | cb::SET_CHUNK_CACHE_CENTER
            | cb::SET_CHUNK_CACHE_RADIUS | cb::SET_DEFAULT_SPAWN_POSITION | cb::SET_DISPLAY_OBJECTIVE | cb::SET_ENTITY_DATA | cb::SET_ENTITY_LINK | cb::SET_EQUIPMENT | cb::SET_OBJECTIVE | cb::SET_PLAYER_TEAM | cb::SET_SCORE | cb::SET_SIMULATION_DISTANCE | cb::SET_SUBTITLE_TEXT | cb::SET_TITLE_TEXT | cb::SET_TITLES_ANIMATION | cb::SYSTEM_CHAT
            | cb::TAB_LIST | cb::TAG_QUERY | cb::TELEPORT_ENTITY | cb::TEST_INSTANCE_BLOCK_STATUS | cb::UPDATE_ADVANCEMENTS | cb::PROJECTILE_POWER | cb::CUSTOM_REPORT_DETAILS | cb::SERVER_LINKS | cb::WAYPOINT | cb::CLEAR_DIALOG | cb::SHOW_DIALOG | cb::ADD_TRANSIENT_BLOCK | cb::GAME_RULE_VALUES | cb::LEVEL_PARTICLES | cb::LOW_DISK_SPACE_WARNING | cb::OPEN_BOOK | cb::PONG_RESPONSE | cb::PLAYER_COMBAT_END | cb::PLAYER_COMBAT_ENTER | cb::RESOURCE_PACK_POP | cb::POST_EFFECTS | cb::SWING_ANIMATION => {
                *self.ignored_presentation.entry(id).or_default() += 1;
            }
            _ => return Err(format!("unknown mandatory Play packet {id}")),
        }
        Ok(())
    }
    fn spawn_info(&mut self, r: &mut Reader<'_>) -> Result<()> {
        let holder = r.count(65536)? as i32;
        let dimension = r.string(32767)?;
        let seed = r.i64()?;
        let mode = r.var()? as u8;
        r.var()?;
        r.bool()?;
        let flat = r.bool()?;
        if r.bool()? {
            r.string(32767)?;
            r.i64()?;
        }
        r.var()?;
        let sea_level = r.var()?;
        self.loader.clear_view(self.ui.as_mut())?;
        let content = self.content.as_ref().ok_or("content unavailable")?;
        let data = content.registry_entry("minecraft:dimension_type", holder)?;
        let min_y = data["min_y"].as_i64().ok_or("dimension min_y missing")?;
        let height = data["height"].as_i64().ok_or("dimension height missing")?;
        if min_y % 16 != 0
            || height % 16 != 0
            || !(16..=4096).contains(&height)
            || !(-2048..=2048).contains(&min_y)
        {
            return Err("unsupported dimension height/min_y bounds".into());
        }
        self.world = World::new(min_y as i32, height as i32);
        self.world.initialize_random_sources()?;
        self.world
            .configure_environment_context(&dimension, seed, sea_level)?;
        self.environment
            .activate_dimension(&dimension, data, self.client_tick)?;
        self.environment.publish_world(&mut self.world)?;
        self.player.update_environment_context(&self.world)?;
        self.player.set_mode(mode)?;
        let mut entropy = [0u8; 8];
        getrandom::fill(&mut entropy).map_err(|e| e.to_string())?;
        self.player
            .set_interaction_seed(u64::from_ne_bytes(entropy));
        self.inventory.game_mode = mode;
        self.flat_world = flat;
        self.dimension = Some(dimension.clone());
        self.far
            .enter_dimension(&dimension, self.address.as_deref());
        self.dimension_epoch += 1;
        self.entities.clear_dimension();
        self.predicted_blocks.clear();
        self.destruction.clear();
        self.block_effects.clear();
        self.block_motion.clear();
        self.received_chunks.clear();
        if let Some(terrain) = &mut self.terrain {
            terrain.reset(&mut self.scene.chunks);
            terrain.activate(
                content,
                [
                    self.connection_epoch,
                    self.dimension_epoch,
                    self.resource_epoch,
                ],
            )?;
            let constant = data["constant_ambient_light"].as_bool().unwrap_or(false);
            terrain.set_render_rules(
                if constant {
                    [0.9, 0.9, 0.9, 0.9, 0.9, 0.9]
                } else {
                    [0.5, 1., 0.8, 0.8, 0.6, 0.6]
                },
                true,
                2,
                // Pinned 26.1.2 Options default: chunkSectionFadeInTime = 0.75s.
                Some(750),
            )?;
        }
        if let Some(audio) = &self.audio {
            audio.stop();
        }
        self.event(json!({"type":"dimension_activated","dimension":dimension,"min_y":min_y,"height":height,"origin":"server_observed"}));
        Ok(())
    }
    fn read_sound(&self, r: &mut Reader<'_>) -> Result<String> {
        let id = r.var()?;
        if id == 0 {
            let name = r.string(32767)?;
            if r.bool()? {
                r.f32()?;
            }
            Ok(name)
        } else {
            self.content
                .as_ref()
                .ok_or("content unavailable")?
                .sound_names
                .get(&((id - 1) as u32))
                .cloned()
                .ok_or_else(|| format!("unknown sound registry ID {id}"))
        }
    }
    /// ClientboundBlockDestruction store. Stage 0..=9 inserts/updates
    /// (replacing on position change); stage >= 10 removes (vanilla also
    /// drops on negative stages, which the u8 wire form cannot express).
    /// Bound: 4096 entries, evict-oldest via BTreeMap pop_first.
    fn receive_destruction(&mut self, breaker: i32, position: [i32; 3], stage: u8) {
        if stage >= 10 {
            self.destruction.remove(&breaker);
            return;
        }
        if !self.destruction.contains_key(&breaker) && self.destruction.len() >= 4096 {
            self.destruction.pop_first();
        }
        self.destruction.insert(breaker, (position, stage));
        self.changed_state();
    }
    fn receive_block(&mut self, position: [i32; 3], state: u32) -> Result<()> {
        let content = self.content.as_ref().ok_or("content unavailable")?;
        content.info(state)?;
        if let Some(prediction) = self.predicted_blocks.get_mut(&position) {
            // ClientLevel.setServerVerifiedBlockState retains the local block
            // until endPredictionsUpTo acknowledges this prediction sequence.
            prediction.received = state;
            self.received_blocks.insert(position, self.packet_sequence);
            self.event(json!({"type":"block_update","position":position,"state_id":state,"origin":"server_observed","packet_sequence":self.packet_sequence.to_string()}));
            return Ok(());
        }
        let old = self.world.block(position);
        let now = (self.client_tick as f64 + f64::from(self.presentation_partial)) * 0.05;
        let new_block = content.info(state)?.block;
        if let Some(old_state) = self.world.block(position) {
            if content.info(old_state)?.block != new_block {
                self.entities.remove_block_entity(position);
            }
        }
        self.world
            .set(position, state, |id| content.block_is_air(id))?;
        self.entities
            .ensure_block_entity(position, state, content, self.client_tick)?;
        self.received_blocks.insert(position, self.packet_sequence);
        // Stale crack entries die with the block they referenced.
        self.destruction.retain(|_, (pos, _)| *pos != position);
        self.player.block_changed(position, state);
        self.entities.block_changed(position, state);
        if let Some(terrain) = &mut self.terrain {
            terrain.invalidate_block(&self.world, position)?;
            if old != Some(state) {
                self.block_motion.changed(
                    terrain,
                    &self.world,
                    content,
                    self.scene.camera_position,
                    now,
                    position,
                    old,
                    state,
                )?;
            }
        }
        self.event(json!({"type":"block_update","position":position,"state_id":state,"origin":"server_observed","packet_sequence":self.packet_sequence.to_string()}));
        Ok(())
    }
    fn reconcile_ack(&mut self, sequence: i32) -> Result<()> {
        let mut blocks = std::mem::take(&mut self.predicted_blocks);
        let content = self.content.as_ref().ok_or("content unavailable")?;
        let now = (self.client_tick as f64 + f64::from(self.presentation_partial)) * 0.05;
        for (position, block) in &blocks {
            if block.sequence <= sequence {
                let new_block = content.info(block.received)?.block;
                if let Some(old_state) = self.world.block(*position) {
                    if content.info(old_state)?.block != new_block {
                        self.entities.remove_block_entity(*position);
                    }
                }
                let old = self.world.block(*position);
                self.world.set(*position, block.received, |state| {
                    content.block_is_air(state)
                })?;
                self.entities.ensure_block_entity(
                    *position,
                    block.received,
                    content,
                    self.client_tick,
                )?;
                self.player.block_changed(*position, block.received);
                self.entities.block_changed(*position, block.received);
                if let Some(terrain) = &mut self.terrain {
                    terrain.invalidate_block(&self.world, *position)?;
                    if old != Some(block.received) {
                        self.block_motion.changed(
                            terrain,
                            &self.world,
                            content,
                            self.scene.camera_position,
                            now,
                            *position,
                            old,
                            block.received,
                        )?;
                    }
                }
            }
        }
        blocks.retain(|_, block| block.sequence > sequence);
        self.predicted_blocks = blocks;
        Ok(())
    }
}
fn chunk_key(p: [i32; 3]) -> [i32; 2] {
    [p[0].div_euclid(16), p[2].div_euclid(16)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::world::{Palette, Section, SurfaceHeightmap};

    #[test]
    fn disconnect_packet_context_keeps_component_readable() {
        use crate::protocol::nbt::Nbt;
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        for (phase, id) in [
            (Phase::Login, login::clientbound::LOGIN_DISCONNECT),
            (Phase::Configuration, configuration::clientbound::DISCONNECT),
            (Phase::Play, cb::DISCONNECT),
        ] {
            let mut session = Session::new_mode("disconnect-test".into(), false);
            session.content = Some(Content::test_cube());
            let mut packet = Writer::default();
            if phase == Phase::Login {
                packet.string(r#"{"text":"Kicked","extra":[{"text":": maintenance"}]}"#);
            } else {
                Nbt::Compound(vec![
                    ("text".into(), Nbt::String("Kicked".into())),
                    ("extra".into(), Nbt::List(10, vec![Nbt::Compound(vec![
                        ("text".into(), Nbt::String(": maintenance".into())),
                    ])])),
                ]).write_root(&mut packet).unwrap();
            }
            let error = session.apply(phase, id, &packet.0).unwrap_err();
            let mut ui = UiState::new_state(&content).unwrap();
            ui.show_disconnected(format!("{phase:?} packet 0x{id:02x} sequence 1: {error}"));
            assert_eq!(ui.title_error_text().as_deref(), Some("Kicked: maintenance"), "{phase:?}");
        }
    }

    #[test]
    fn disconnect_translation_fallback_and_unrelated_diagnostics() {
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        let mut ui = UiState::new_state(&content).unwrap();
        ui.show_disconnected(r#"Play packet 0x20 sequence 7: Play disconnect: {"translate":"missing.kick.key","fallback":"Banned: %s","with":[{"text":"rules"}],"extra":[{"text":"!"}]}"#.into());
        assert_eq!(ui.title_error_text().as_deref(), Some("Banned: rules!"));
        let diagnostic = "Play packet 0x01 sequence 8: malformed data: {\"text\":\"do not strip\"}";
        ui.show_disconnected(diagnostic.into());
        assert_eq!(ui.title_error_text().as_deref(), Some(diagnostic));
    }

    fn session_with_received_cube(position: [i32; 3]) -> Session {
        let mut session = Session::new_mode("received-prediction-test".into(), false);
        let mut content = Content::test_cube();
        for state in [0, 1] {
            content.insert_test_placement_metadata(state, json!({"block_entity_type":null}));
        }
        session.content = Some(content);
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
                chunk_key(position),
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
        session.packet_sequence = 10;
        session.received_chunks.insert(chunk_key(position), 10);
        session.receive_block(position, 1).unwrap();
        session
    }

    #[test]
    fn ticking_state_accepts_server_rate_and_preserves_it_for_interpolation() {
        let mut session = session_with_received_cube([0, 3, 0]);
        session
            .environment
            .activate_dimension(
                "minecraft:the_nether",
                &json!({"min_y":0,"height":16,"has_skylight":false,"has_ceiling":true,"skybox":"none"}),
                0,
            )
            .unwrap();
        let mut packet = Writer::default();
        packet.f32(40.);
        packet.byte(0);

        session.apply(Phase::Play, 130, &packet.0).unwrap();

        assert_eq!(session.tick_interval(), Duration::from_millis(25));
        assert!(!session.server_frozen);
    }

    #[test]
    fn block_burst_answers_keepalive_before_consumption_and_drains_in_one_frame() {
        use std::{io::{Read, Write}, net::{TcpListener, TcpStream}, sync::mpsc, thread, time::{Duration, Instant}};
        fn packet(socket: &mut TcpStream) -> (i32, Vec<u8>) {
            let mut header = Vec::new();
            loop {
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                if byte[0] & 128 == 0 { break; }
                assert!(header.len() < 5);
            }
            let mut body = vec![0; Reader::new(&header).count(1024).unwrap()];
            socket.read_exact(&mut body).unwrap();
            let mut r = Reader::new(&body);
            let id = r.var().unwrap();
            (id, body[r.offset..].to_vec())
        }
        fn send(socket: &mut TcpStream, id: i32, body: &[u8]) {
            socket.write_all(&protocol::frame(id, body, None).unwrap()).unwrap();
        }
        const COUNT: i32 = 80_000;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (answered, answer) = mpsc::channel();
        let (finish, finished) = mpsc::channel();
        let peer = thread::spawn(move || {
            let (mut login, _) = listener.accept().unwrap();
            let (mut status, _) = listener.accept().unwrap();
            login.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            login.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
            packet(&mut status);
            packet(&mut status);
            let mut response = Writer::default();
            response.string(r#"{"version":{"protocol":777}}"#);
            send(&mut status, 0, &response.0);
            packet(&mut login);
            packet(&mut login);
            let mut success = Writer(vec![0; 16]);
            success.string("BurstTest");
            success.var(0);
            send(&mut login, 2, &success.0);
            assert_eq!(packet(&mut login).0, 3);
            assert_eq!(packet(&mut login).0, 0);
            send(&mut login, 3, &[]);
            assert_eq!(packet(&mut login).0, 3);
            let mut burst = Vec::new();
            for i in 0..COUNT {
                let position = [i / 256, (i / 16) % 16, i % 16];
                burst.extend(protocol::frame(8, &block_update_bytes(position, 1), None).unwrap());
            }
            burst.extend(protocol::frame(45, &123i64.to_be_bytes(), None).unwrap());
            burst.extend(protocol::frame(62, &456i32.to_be_bytes(), None).unwrap());
            let start = Instant::now();
            login.write_all(&burst).unwrap();
            assert_eq!(packet(&mut login), (28, 123i64.to_be_bytes().to_vec()));
            assert_eq!(packet(&mut login), (45, 456i32.to_be_bytes().to_vec()));
            answered.send(start.elapsed()).unwrap();
            finished.recv().unwrap();
        });
        let mut session = session_with_received_cube([0, 0, 0]);
        let template = session.world.chunk([0, 0]).unwrap().clone();
        for x in 1..=COUNT / 4096 {
            session.world.insert([x, 0], template.clone()).unwrap();
        }
        session.network = Some(Network::connect(address.to_string(), "BurstTest".into(), 8, Arc::new(|| {})).unwrap());
        // No consumer runs until the peer observes the echo: a full world-event
        // queue must never stop the socket owner from reading the keepalive.
        let elapsed = answer.recv_timeout(Duration::from_secs(1));
        assert!(elapsed.is_ok(), "keepalive starved behind 80k block events: {elapsed:?}");
        assert!(elapsed.unwrap() < Duration::from_secs(1));
        // The fixture installed content already; start at the wire Configuration
        // boundary without running content activation a second time.
        for _ in 0..3 {
            assert!(session.network.as_ref().unwrap().try_recv().is_ok());
        }
        session.phase = "disconnected".into(); // skip scene rendering, not packet application
        let before = session.packet_sequence;
        session.process_events();
        assert!(session.error.is_none(), "{:?}", session.error);
        assert_eq!(session.packet_counts.get(&8), Some(&(COUNT as u64)));
        for i in 0..COUNT {
            let position = [i / 256, (i / 16) % 16, i % 16];
            assert_eq!(session.world.block(position), Some(1), "{position:?}");
            assert!(session.received_block_revision(position) > before);
        }
        finish.send(()).unwrap();
        peer.join().unwrap();
    }

    fn level_event_bytes(event: i32, position: [i32; 3], data: i32, global: bool) -> Vec<u8> {
        let mut writer = Writer::default();
        writer.i32(event);
        writer.i64(protocol::pack_pos(position));
        writer.i32(data);
        writer.byte(u8::from(global));
        writer.0
    }

    #[test]
    fn block_break_level_event_admits_texture_fragment_presentation() {
        let position = [0, 3, 0];
        let mut session = session_with_received_cube(position);
        session.presentation = true;
        session
            .apply(
                Phase::Play,
                47,
                &level_event_bytes(2001, position, 1, false),
            )
            .unwrap();
        assert_eq!(
            session.events.back().unwrap()["presentation_supported"],
            true
        );
        let mut geometry = crate::api::DynamicGeometry::default();
        session
            .block_effects
            .append(&session.world, [0.; 3], 0., 0., 0., &mut geometry)
            .unwrap();
        assert!(
            geometry.vertices.is_empty(),
            "unknown light must not invent full brightness"
        );
        let mut chunk = session.world.chunk(chunk_key(position)).unwrap().clone();
        chunk.sky.insert(1, Arc::new(vec![0xff; 2048]));
        chunk.block.insert(1, Arc::new(vec![0; 2048]));
        session.world.insert(chunk_key(position), chunk).unwrap();
        session
            .block_effects
            .append(&session.world, [0.; 3], 45., 30., 0., &mut geometry)
            .unwrap();
        assert_eq!(geometry.vertices.len(), 64 * 4);
        assert_eq!(
            geometry.indices[crate::api::EntityMaterial::EntityCutout as usize].len(),
            64 * 6
        );
        assert!(geometry
            .vertices
            .iter()
            .all(|vertex| vertex.light == [15., 0.]));
        let a = glam::Vec3::from_array(geometry.vertices[0].position);
        let b = glam::Vec3::from_array(geometry.vertices[1].position);
        let c = glam::Vec3::from_array(geometry.vertices[2].position);
        let normal = (b - a).cross(c - a).normalize();
        let (sy, cy) = 45f32.to_radians().sin_cos();
        let (sp, cp) = 30f32.to_radians().sin_cos();
        let view = glam::Vec3::new(-sy * cp, -sp, cy * cp);
        assert!(
            normal.dot(view).abs() > 0.9999,
            "fragment must face the pitched camera"
        );
    }

    #[test]
    fn received_level_event_decodes_and_dispatches_exact_wire_fields() {
        let position = [-33, -4, 65];
        for global in [false, true] {
            let mut session = session_with_received_cube([0, 3, 0]);
            let body = level_event_bytes(0x01020304, position, -17, global);
            assert_eq!(body.len(), 17);
            assert_eq!(&body[..4], &[1, 2, 3, 4]);
            session.apply(Phase::Play, 47, &body).unwrap();
            let event = json!({"type":"level_event","event":0x01020304,"position":position,
                "data":-17,"global":global,"origin":"server_observed",
                "packet_sequence":"10","presentation_supported":false});
            assert_eq!(session.events.back(), Some(&event));
            assert_eq!(
                session.observation().data["events"]
                    .as_array()
                    .unwrap()
                    .last(),
                Some(&event)
            );
            assert_eq!(session.ignored_presentation.get(&47), None);
        }
    }

    #[test]
    fn received_level_event_rejects_malformed_body_without_recording() {
        let position = [-33, -4, 65];
        let mut session = session_with_received_cube([0, 3, 0]);
        let body = level_event_bytes(3010, position, 7, false);
        let before_events = session.events.len();
        let before_version = session.state_version();
        for length in 0..body.len() {
            assert!(session.apply(Phase::Play, 47, &body[..length]).is_err());
            assert_eq!(session.events.len(), before_events);
            assert_eq!(session.state_version(), before_version);
        }
        let mut trailing = body;
        trailing.push(0);
        assert!(session.apply(Phase::Play, 47, &trailing).is_err());
        assert_eq!(session.events.len(), before_events);
        assert_eq!(session.state_version(), before_version);
        let error = session.apply(Phase::Play, 144, &[]).unwrap_err();
        assert_eq!(error, "unknown mandatory Play packet 144");
        assert_eq!(session.events.len(), before_events);
    }

    #[test]
    fn prediction_and_ack_cannot_confirm_received_block_removal() {
        let position = [-1, 3, -1];
        let mut session = session_with_received_cube(position);
        session.predicted_blocks.insert(
            position,
            PredictedBlock {
                received: 1,
                sequence: 7,
            },
        );
        session
            .world
            .set(position, 0, |state| Ok(state == 0))
            .unwrap();
        assert_eq!(session.world.block(position), Some(0));
        assert_eq!(session.received_block(position), Some(1));
        assert_eq!(session.received_block_revision(position), 10);
        session.reconcile_ack(6).unwrap();
        assert_eq!(session.world.block(position), Some(0));
        session.reconcile_ack(7).unwrap();
        assert_eq!(session.world.block(position), Some(1));
        assert_eq!(session.received_block(position), Some(1));
        assert_eq!(session.received_block_revision(position), 10);
        // Production bumps packet_sequence per received packet (process_events).
        session.packet_sequence = 11;
        session.receive_block(position, 0).unwrap();
        assert_eq!(session.received_block(position), Some(0));
        assert_eq!(session.received_block_revision(position), 11);
        session.reconcile_ack(7).unwrap();
        assert_eq!(session.world.block(position), Some(0));
    }
    fn destruction_bytes(breaker: i32, position: [i32; 3], stage: u8) -> Vec<u8> {
        let mut w = Writer::default();
        w.var(breaker);
        w.i64(protocol::pack_pos(position));
        w.byte(stage);
        w.0
    }

    #[test]
    fn destruction_packet_inserts_updates_and_removes() {
        let position = [-1, 3, -1];
        let mut session = session_with_received_cube(position);
        assert!(session.destruction.is_empty());
        // Play id 5 must not be counted as ignored presentation.
        let body = destruction_bytes(7, position, 3);
        let before = session.revision;
        session.apply(Phase::Play, 5, &body).unwrap();
        assert_eq!(session.destruction.get(&7), Some(&(position, 3)));
        assert_eq!(session.ignored_presentation.get(&5), None);
        assert!(session.revision > before);
        // Same breaker, new position: replace.
        let other = [2, 3, -1];
        session.receive_block(other, 1).unwrap();
        session
            .apply(Phase::Play, 5, &destruction_bytes(7, other, 5))
            .unwrap();
        assert_eq!(session.destruction.get(&7), Some(&(other, 5)));
        // Stage >= 10 removes (vanilla destroys progress at >= 10).
        session
            .apply(Phase::Play, 5, &destruction_bytes(7, other, 10))
            .unwrap();
        assert!(!session.destruction.contains_key(&7));
    }

    #[test]
    fn destruction_store_evicts_oldest_beyond_bound() {
        let position = [-1, 3, -1];
        let mut session = session_with_received_cube(position);
        for breaker in 0..4097i32 {
            session.receive_destruction(breaker, position, 1);
        }
        assert_eq!(session.destruction.len(), 4096);
        assert!(!session.destruction.contains_key(&0));
        assert!(session.destruction.contains_key(&4096));
    }

    #[test]
    fn block_update_drops_cracks_for_that_position() {
        let position = [-1, 3, -1];
        let mut session = session_with_received_cube(position);
        // Helper leaves world state at `position`; crack a neighbor so only
        // the destruction cleanup is under test.
        let cracked = [0, 3, -1];
        session.receive_block(cracked, 1).unwrap();
        session.receive_destruction(7, cracked, 4);
        session.packet_sequence = 11;
        session.receive_block(cracked, 0).unwrap();
        assert!(session.destruction.is_empty());
    }
    fn sign_session(position: [i32; 3], kind: &str) -> Session {
        let mut session = session_with_received_cube(position);
        let content = session.content.as_mut().unwrap();
        content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
        content.block_entity_names.insert(0, kind.into());
        content.insert_test_placement_metadata(0, json!({"block_entity_type":null}));
        content.insert_test_placement_metadata(1, json!({"block_entity_type":kind}));
        session.ui = Some(UiState::new_state(content).unwrap());
        session.phase = "play".into();
        session
    }

    fn open_sign_bytes(position: [i32; 3], front: bool) -> Vec<u8> {
        let mut writer = Writer::default();
        writer.i64(protocol::pack_pos(position));
        writer.var(i32::from(front));
        writer.0
    }

    #[test]
    fn disconnects_sign_opening_error_is_local_but_malformed_packet_stays_fatal() {
        let position = [-1, 3, -1];
        let mut session = sign_session(position, "minecraft:sign");
        session.receive_block(position, 1).unwrap();
        session.ui = None;
        let body = open_sign_bytes(position, true);
        session.apply(Phase::Play, cb::OPEN_SIGN_EDITOR, &body).unwrap();
        assert_eq!(session.phase, "play");
        assert!(session.error.is_none());
        let mut malformed = body;
        malformed.push(0);
        assert!(session.apply(Phase::Play, cb::OPEN_SIGN_EDITOR, &malformed).is_err());
    }

    #[test]
    fn p1_sign_block_update_then_editor_creates_both_empty_sides() {
        let position = [-1, 3, -1];
        for kind in ["minecraft:sign", "minecraft:hanging_sign"] {
            for front in [false, true] {
                let mut session = sign_session(position, kind);
                session.receive_block(position, 0).unwrap();
                let mut block = Writer::default();
                block.i64(protocol::pack_pos(position));
                block.var(1);
                session.apply(Phase::Play, 8, &block.0).unwrap();
                assert_eq!(session.entities.block_entity_name(position), Some(kind));
                assert_eq!(session.entities.block_entities[&position].state, Some(1));
                session
                    .apply(Phase::Play, 61, &open_sign_bytes(position, front))
                    .unwrap();
                let data = session.entities.block_entity_data(position).unwrap();
                for side in ["front_text", "back_text"] {
                    assert_eq!(data[side]["messages"], json!(["", "", "", ""]));
                    assert_eq!(data[side]["color"], "black");
                    assert_eq!(data[side]["has_glowing_text"], false);
                }
                // Constructor defaults are not an observed block-entity NBT packet.
                assert_eq!(session.entities.block_entity_nbt(position), None);
                let ui = session.ui.as_mut().unwrap();
                ui.synchronize(&session.inventory, session.content.as_ref().unwrap())
                    .unwrap();
                assert_eq!(ui.screen(), crate::ui::Screen::Sign);
                for line in 0..4 {
                    assert_eq!(
                        ui.title_field(&format!("sign/line/{line}")),
                        Some(String::new())
                    );
                }
                assert_eq!(session.phase, "play");
                let context = ui.context(&session.inventory);
                ui.execute(
                    crate::ui::UiIntent::EditSignLine {
                        context,
                        line: 0,
                        text: "P1 sign".into(),
                    },
                    &mut session.inventory,
                    session.content.as_ref().unwrap(),
                )
                .unwrap();
                let context = ui.context(&session.inventory);
                let effects = ui
                    .execute(
                        crate::ui::UiIntent::SubmitSign { context },
                        &mut session.inventory,
                        session.content.as_ref().unwrap(),
                    )
                    .unwrap();
                assert!(effects.commands.iter().any(|command| matches!(
                    command,
                    crate::ui::UiCommand::Sign { pos, front: side, lines }
                        if *pos == position && *side == front && lines[0] == "P1 sign"
                )));
                assert_eq!(ui.screen(), crate::ui::Screen::Game);
            }
        }
    }

    #[test]
    fn p1_sign_editor_uses_received_side_without_replacing_nbt() {
        let position = [-1, 3, -1];
        let mut session = sign_session(position, "minecraft:sign");
        let data = Arc::new(json!({
            "front_text":{"messages":["front", "", "", ""]},
            "back_text":{"messages":["back", "", "", ""]}
        }));
        session
            .entities
            .apply_block_entity(
                position,
                0,
                data,
                Arc::from([10, 0]),
                Some(1),
                session.content.as_ref().unwrap(),
                0,
            )
            .unwrap();
        session
            .apply(Phase::Play, 61, &open_sign_bytes(position, false))
            .unwrap();
        assert_eq!(
            session.ui.as_ref().unwrap().title_field("sign/line/0"),
            Some("back".into())
        );
        assert_eq!(
            session.entities.block_entity_nbt(position),
            Some(&[10, 0][..])
        );
        session
            .apply(Phase::Play, 61, &open_sign_bytes(position, true))
            .unwrap();
        let ui = session.ui.as_mut().unwrap();
        ui.synchronize(&session.inventory, session.content.as_ref().unwrap())
            .unwrap();
        assert_eq!(ui.title_field("sign/line/0"), Some("front".into()));
    }

    #[test]
    fn p1_sign_editor_on_non_sign_is_ignored() {
        let position = [-1, 3, -1];
        let mut session = sign_session(position, "minecraft:sign");
        session.receive_block(position, 0).unwrap();
        session
            .apply(Phase::Play, 61, &open_sign_bytes(position, true))
            .unwrap();
        assert_eq!(session.inventory.sign_editor, None);
        assert!(!session.inventory.screen_open);
        let ui = session.ui.as_mut().unwrap();
        ui.synchronize(&session.inventory, session.content.as_ref().unwrap())
            .unwrap();
        assert_eq!(ui.screen(), crate::ui::Screen::Game);
        assert!(session.entities.block_entity_data(position).is_none());
    }

    fn block_update_bytes(position: [i32; 3], state: i32) -> Vec<u8> {
        let mut writer = Writer::default();
        writer.i64(protocol::pack_pos(position));
        writer.var(state);
        writer.0
    }

    fn sign_data_bytes(position: [i32; 3], kind: i32) -> Vec<u8> {
        use crate::inventory::{nbt, Tag};
        let text = |line: &str| {
            Tag::record([
                (
                    "messages",
                    Tag::List([line, "", "", ""].into_iter().map(Tag::string).collect()),
                ),
                ("color", Tag::string("black")),
                ("has_glowing_text", Tag::Byte(0)),
            ])
        };
        let mut writer = Writer::default();
        writer.i64(protocol::pack_pos(position));
        writer.var(kind);
        nbt::write(
            &Tag::record([
                ("front_text", text("received front")),
                ("back_text", text("received back")),
                ("is_waxed", Tag::Byte(0)),
            ]),
            &mut writer,
        )
        .unwrap();
        writer.0
    }

    fn receive_sign_with_prediction(session: &mut Session, position: [i32; 3]) {
        session
            .apply(Phase::Play, 8, &block_update_bytes(position, 1))
            .unwrap();
        session
            .apply(Phase::Play, 6, &sign_data_bytes(position, 0))
            .unwrap();
        session
            .entities
            .predict_sign_text(
                position,
                true,
                &[
                    "predicted front".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                ],
            )
            .unwrap();
        session.entities.block_active.insert(position);
    }

    #[test]
    fn p1_review_received_sign_replacement_opens_empty_editor() {
        let position = [-1, 3, -1];
        for kind in ["minecraft:sign", "minecraft:hanging_sign"] {
            for front in [false, true] {
                let mut session = sign_session(position, kind);
                receive_sign_with_prediction(&mut session, position);
                let revision = session.entities.revision;
                session
                    .apply(Phase::Play, 8, &block_update_bytes(position, 0))
                    .unwrap();
                assert_eq!(session.entities.revision, revision + 1);
                assert_eq!(session.entities.block_entity_name(position), None);
                assert_eq!(session.entities.block_entity_data(position), None);
                assert_eq!(session.entities.block_entity_nbt(position), None);
                assert!(!session.entities.block_active.contains(&position));
                session
                    .apply(Phase::Play, 8, &block_update_bytes(position, 1))
                    .unwrap();
                assert_eq!(
                    session.entities.block_entity_data(position).unwrap()["front_text"]["messages"],
                    json!(["", "", "", ""])
                );
                session
                    .apply(Phase::Play, 61, &open_sign_bytes(position, front))
                    .unwrap();
                let ui = session.ui.as_mut().unwrap();
                ui.synchronize(&session.inventory, session.content.as_ref().unwrap())
                    .unwrap();
                assert_eq!(ui.screen(), crate::ui::Screen::Sign);
                for line in 0..4 {
                    assert_eq!(
                        ui.title_field(&format!("sign/line/{line}")),
                        Some(String::new())
                    );
                }
            }
        }
    }

    #[test]
    fn p1_review_ack_block_replacement_clears_sign_and_prediction() {
        let position = [-1, 3, -1];
        let mut session = sign_session(position, "minecraft:sign");
        receive_sign_with_prediction(&mut session, position);
        session.predicted_blocks.insert(
            position,
            PredictedBlock {
                received: 0,
                sequence: 7,
            },
        );
        session.reconcile_ack(6).unwrap();
        assert!(session.entities.block_entities[&position]
            .predicted_data
            .is_some());
        session.reconcile_ack(7).unwrap();
        assert_eq!(session.world.block(position), Some(0));
        assert_eq!(session.entities.block_entity_data(position), None);
        assert_eq!(session.entities.block_entity_nbt(position), None);
        assert!(!session.entities.block_active.contains(&position));
        session.predicted_blocks.insert(
            position,
            PredictedBlock {
                received: 1,
                sequence: 8,
            },
        );
        session.reconcile_ack(8).unwrap();
        assert_eq!(session.entities.block_entities[&position].state, Some(1));
        session
            .apply(Phase::Play, 61, &open_sign_bytes(position, true))
            .unwrap();
        assert_eq!(
            session.ui.as_ref().unwrap().title_field("sign/line/0"),
            Some(String::new())
        );
    }

    #[test]
    fn p1_review_same_block_state_and_ack_preserve_sign_text_and_prediction() {
        let position = [-1, 3, -1];
        for kind in ["minecraft:sign", "minecraft:hanging_sign"] {
            let mut session = sign_session(position, kind);
            let content = session.content.as_mut().unwrap();
            // A distinct state ID of the same block (e.g. sign rotation).
            content.states.push(content.states[1].clone());
            receive_sign_with_prediction(&mut session, position);
            let received = session.entities.block_entities[&position].data.clone();
            let predicted = session.entities.block_entities[&position]
                .predicted_data
                .clone()
                .unwrap();
            session
                .apply(Phase::Play, 8, &block_update_bytes(position, 2))
                .unwrap();
            session.predicted_blocks.insert(
                position,
                PredictedBlock {
                    received: 1,
                    sequence: 9,
                },
            );
            session.reconcile_ack(9).unwrap();
            let entity = &session.entities.block_entities[&position];
            assert_eq!(entity.state, Some(1));
            assert!(Arc::ptr_eq(&entity.data, &received));
            assert!(Arc::ptr_eq(
                entity.predicted_data.as_ref().unwrap(),
                &predicted
            ));
            assert!(session.entities.block_active.contains(&position));
            assert_eq!(
                session.entities.block_entity_data(position).unwrap()["front_text"]["messages"][0],
                "predicted front"
            );
            session
                .apply(Phase::Play, 61, &open_sign_bytes(position, true))
                .unwrap();
            assert_eq!(
                session.ui.as_ref().unwrap().title_field("sign/line/0"),
                Some("predicted front".into())
            );
        }
    }

    #[test]
    fn p1_review_block_update_then_received_sign_text_is_state_bound() {
        let position = [-1, 3, -1];
        for kind in ["minecraft:sign", "minecraft:hanging_sign"] {
            let mut session = sign_session(position, kind);
            session.receive_block(position, 0).unwrap();
            session
                .apply(Phase::Play, 8, &block_update_bytes(position, 1))
                .unwrap();
            let entity = &session.entities.block_entities[&position];
            assert_eq!(entity.state, Some(1));
            assert!(entity.raw_nbt.is_empty());
            session
                .apply(Phase::Play, 6, &sign_data_bytes(position, 0))
                .unwrap();
            let entity = &session.entities.block_entities[&position];
            assert_eq!(entity.type_name, kind);
            assert_eq!(entity.state, Some(1));
            assert_eq!(entity.data["front_text"]["messages"][0], "received front");
            assert_eq!(entity.data["back_text"]["messages"][0], "received back");
            assert!(!entity.raw_nbt.is_empty());
        }
    }

    #[test]
    fn p1_review_block_entity_data_ignores_missing_or_mismatched_entities() {
        let position = [-1, 3, -1];
        let mut session = sign_session(position, "minecraft:sign");
        session.receive_block(position, 0).unwrap();
        session
            .apply(Phase::Play, 6, &sign_data_bytes(position, 0))
            .unwrap();
        assert_eq!(session.entities.block_entity_data(position), None);
        session.receive_block(position, 1).unwrap();
        session
            .content
            .as_mut()
            .unwrap()
            .block_entity_names
            .insert(1, "minecraft:hanging_sign".into());
        let received = session.entities.block_entities[&position].data.clone();
        session
            .apply(Phase::Play, 6, &sign_data_bytes(position, 1))
            .unwrap();
        assert!(Arc::ptr_eq(
            &session.entities.block_entities[&position].data,
            &received
        ));
        assert_eq!(
            session.entities.block_entity_name(position),
            Some("minecraft:sign")
        );
        assert_eq!(session.entities.block_entity_nbt(position), None);
    }

    #[test]
    fn p1_review_non_sign_editor_preserves_open_menu_and_admission() {
        use crate::inventory::{nbt, Tag};
        let position = [-1, 3, -1];
        let mut session = sign_session(position, "minecraft:sign");
        session.receive_block(position, 0).unwrap();
        let mut menu = Writer::default();
        menu.var(7);
        menu.var(0); // Generic 9x1 menu.
        nbt::write(&Tag::string("Review menu"), &mut menu).unwrap();
        session.apply(Phase::Play, 60, &menu.0).unwrap();
        session
            .ui
            .as_mut()
            .unwrap()
            .synchronize(&session.inventory, session.content.as_ref().unwrap())
            .unwrap();
        let context = session.ui.as_ref().unwrap().context(&session.inventory);
        let inventory = session.inventory.observation();
        let active = serde_json::to_value(session.inventory.active.as_ref().unwrap()).unwrap();
        let packet_version = session.container_packet_version;
        let entity_revision = session.entities.revision;
        session.keys.insert("forward".into());
        session
            .apply(Phase::Play, 61, &open_sign_bytes(position, true))
            .unwrap();
        session
            .ui
            .as_mut()
            .unwrap()
            .synchronize(&session.inventory, session.content.as_ref().unwrap())
            .unwrap();
        assert_eq!(session.inventory.observation(), inventory);
        assert_eq!(
            serde_json::to_value(session.inventory.active.as_ref().unwrap()).unwrap(),
            active
        );
        assert_eq!(session.inventory.sign_editor, None);
        assert_eq!(session.container_packet_version, packet_version);
        assert_eq!(session.entities.revision, entity_revision);
        assert_eq!(
            session.ui.as_ref().unwrap().screen(),
            crate::ui::Screen::Menu
        );
        assert_eq!(
            session.ui.as_ref().unwrap().context(&session.inventory),
            context
        );
        session.inventory.validate(&context).unwrap();
        assert!(session.keys.contains("forward"));
        assert!(session.commands.is_empty());
    }

    #[test]
    fn p1_review_block_updates_create_native_default_block_entities() {
        let position = [-1, 3, -1];
        for kind in [
            "minecraft:chest",
            "minecraft:trapped_chest",
            "minecraft:ender_chest",
            "minecraft:shulker_box",
            "minecraft:enchanting_table",
            "minecraft:lectern",
            "minecraft:banner",
            "minecraft:decorated_pot",
            "minecraft:skull",
            "minecraft:bed",
            "minecraft:bell",
            "minecraft:conduit",
            "minecraft:campfire",
            "minecraft:brushable_block",
            "minecraft:beacon",
            "minecraft:end_portal",
            "minecraft:end_gateway",
            "minecraft:structure_block",
            "minecraft:test_instance_block",
            "minecraft:mob_spawner",
            "minecraft:trial_spawner",
            "minecraft:vault",
            "minecraft:shelf",
            "minecraft:copper_golem_statue",
            "minecraft:furnace",
            "minecraft:blast_furnace",
            "minecraft:smoker",
            "minecraft:dispenser",
            "minecraft:dropper",
            "minecraft:hopper",
            "minecraft:barrel",
            "minecraft:brewing_stand",
            "minecraft:jukebox",
            "minecraft:daylight_detector",
            "minecraft:comparator",
            "minecraft:command_block",
            "minecraft:jigsaw",
            "minecraft:beehive",
            "minecraft:creaking_heart",
            "minecraft:sculk_sensor",
            "minecraft:calibrated_sculk_sensor",
            "minecraft:sculk_catalyst",
            "minecraft:sculk_shrieker",
            "minecraft:chiseled_bookshelf",
            "minecraft:crafter",
            "minecraft:test_block",
        ] {
            let mut session = sign_session(position, kind);
            session.receive_block(position, 0).unwrap();
            session
                .apply(Phase::Play, 8, &block_update_bytes(position, 1))
                .unwrap();
            let entity = &session.entities.block_entities[&position];
            assert_eq!(entity.type_name, kind);
            assert_eq!(entity.state, Some(1));
            assert!(entity.raw_nbt.is_empty());
            assert_eq!(entity.animation.progress, 0.);
            assert_eq!(entity.animation.source.gateway_age, 0);
            assert_eq!(entity.animation.source.spawn_delay, 20);
            assert_eq!(entity.animation.signals.comparator_output, 0);
            assert!(!entity.animation.signals.test_powered);
            if kind == "minecraft:structure_block" {
                assert_eq!(entity.data["showboundingbox"], true);
            }
        }
    }

    #[test]
    fn p1_review_moving_piston_block_factory_creates_no_synthetic_entity() {
        let position = [-1, 3, -1];
        let mut session = sign_session(position, "minecraft:piston");
        session.receive_block(position, 0).unwrap();
        session
            .apply(Phase::Play, 8, &block_update_bytes(position, 1))
            .unwrap();
        assert_eq!(session.entities.block_entity_name(position), None);
        assert!(!session.entities.block_active.contains(&position));
    }

    #[test]
    fn p1_hurt_damage_event_restarts_local_tilt_and_preserves_received_direction() {
        let mut session = session_with_received_cube([4, 2, 7]);
        session.phase = "play".into();
        session.dimension = Some("minecraft:overworld".into());
        session.player.pose_known = true;
        let mut chunk = session.world.chunk([0, 0]).unwrap().clone();
        chunk.sky.insert(1, Arc::new(vec![0xff; 2048]));
        chunk.block.insert(1, Arc::new(vec![0; 2048]));
        for x in -1..=1 {
            for z in -1..=1 {
                session.world.insert([x, z], chunk.clone()).unwrap();
            }
        }
        assert!(session.play_world_presentation_ready());
        session.player.entity_id = 7;
        session.player.hurt_tick = Some(90);
        session.player.hurt_direction = 45.;
        session.player.yaw = 30.;
        session.client_tick = 100;
        for source in [false, true] {
            let mut packet = Writer::default();
            packet.var(7);
            packet.var(0);
            packet.var(0);
            packet.var(0);
            packet.byte(u8::from(source));
            if source {
                for coordinate in [1., 2., 3.] {
                    packet.f64(coordinate);
                }
            }
            session.apply(Phase::Play, 25, &packet.0).unwrap();
            assert_eq!(session.player.hurt_tick, Some(100));
            assert_eq!(session.player.hurt_direction, 45.);
            session.client_tick = 102;
            session.camera();
            let angle = session.scene.hurt_tilt_deg;
            assert!(angle < -13.);
            let observation = session.observation();
            let minecraft_player = &observation.data["minecraft"]["player"];
            assert_eq!(minecraft_player["hurt_tilt_deg"], json!(angle));
            assert_eq!(minecraft_player["hurt_direction"], json!(45.));
            assert_eq!(minecraft_player["hurt_tick"], json!("100"));
            assert_eq!(observation.data["camera"]["hurt_tilt_deg"], json!(angle));
            assert_eq!(session.player.yaw, 30.);
            session.client_tick = 110;
            session.camera();
            assert_eq!(session.scene.hurt_tilt_deg, 0.);
            assert_eq!(
                session.observation().data["camera"]["hurt_tilt_deg"],
                json!(0.)
            );
            // Other entities and malformed local events cannot restart the local timer.
            packet.0[0] = 8;
            session.apply(Phase::Play, 25, &packet.0).unwrap();
            assert_eq!(session.player.hurt_tick, Some(100));
            packet.0[0] = 7;
            packet.0.push(0);
            assert!(session.apply(Phase::Play, 25, &packet.0).is_err());
            assert_eq!(session.player.hurt_tick, Some(100));
            session.client_tick = 100;
        }
        session.disconnect();
        session.camera();
        assert_eq!(session.scene.hurt_tilt_deg, 0.);
        assert_eq!(
            session.observation().data["camera"]["hurt_tilt_deg"],
            json!(0.)
        );
    }
    #[test]
    fn chunk_without_block_entity_nbt_creates_bell_constructor_default() {
        let mut session = Session::new_mode("chunk-constructor-test".into(), false);
        let mut content = Content::test_cube();
        content.names[1] = "minecraft:bell".into();
        content
            .block_entity_names
            .insert(7, "minecraft:bell".into());
        content.insert_test_placement_metadata(0, json!({"block_entity_type":null}));
        content.insert_test_placement_metadata(1, json!({"block_entity_type":"minecraft:bell"}));
        session.content = Some(content);
        session.world = World::new(0, 16);

        // One bell at [-1,3,-1] in a received negative-coordinate chunk,
        // deliberately with no block-entity NBT entries.
        let index = 3 * 256 + 15 * 16 + 15;
        let mut section = Writer::default();
        section.0.extend(1i16.to_be_bytes());
        section.0.extend(0i16.to_be_bytes());
        section.byte(4);
        section.var(2);
        section.var(0);
        section.var(1);
        for word in 0..256 {
            section.i64(if word == index / 16 {
                1i64 << ((index % 16) * 4)
            } else {
                0
            });
        }
        section.byte(0);
        section.var(0);
        let mut packet = Writer::default();
        packet.i32(-1);
        packet.i32(-1);
        packet.var(0); // no heightmaps
        packet.var(section.0.len() as i32);
        packet.0.extend(section.0);
        packet.var(0); // no block-entity NBT
        for _ in 0..6 {
            packet.var(0); // four empty light masks and two empty array lists
        }
        session.apply(Phase::Play, 46, &packet.0).unwrap();
        let position = [-1, 3, -1];
        assert_eq!(session.world.block(position), Some(1));
        assert_eq!(
            session.entities.block_entity_name(position),
            Some("minecraft:bell")
        );
        let bell = &session.entities.block_entities[&position];
        assert_eq!(bell.state, Some(1));
        assert!(
            bell.raw_nbt.is_empty(),
            "constructor defaults are not received NBT"
        );
    }
}
