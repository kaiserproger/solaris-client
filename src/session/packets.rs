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
                0 => return Err(format!("login disconnect: {}", r.string(32767)?)),
                2 => {
                    let uuid = uuid::Uuid::from_slice(r.take(16)?).map_err(|error| error.to_string())?.to_string();
                    self.player.uuid = Some(Arc::from(uuid));
                    r.string(16)?;
                    for _ in 0..r.count(16)? { r.string(32767)?; r.string(32767)?; if r.bool()? { r.string(32767)?; } }
                    r.done()?; self.phase = "configuration".into();
                    eprintln!("session startup: configuration");
                }
                3 => { r.count(protocol::MAX_FRAME)?; r.done()?; }
                4 | 5 => {}
                _ => return Err(format!("unsupported mandatory Login packet {id}")),
            },
            Phase::Configuration => match id {
                2 => return Err(format!("configuration disconnect: {}", r.nbt()?)),
                3 => {
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
                7 => {
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
                12 => {
                    let mut features = BTreeSet::new();
                    for _ in 0..r.count(1024)? {
                        let feature = r.string(32767)?;
                        if feature != "minecraft:vanilla" { return Err(format!("unsupported mandatory feature flag {feature}")); }
                        features.insert(feature);
                    }
                    r.done()?; self.features = Arc::new(features);
                }
                13 => {
                    let tags = protocol::read_registry_tags(&mut r)?;
                    for (registry, values) in &tags { self.environment.configure_tags(registry, values)?; }
                    self.registry_tags = tags;
                }
                14 => { for _ in 0..r.count(1024)? { r.string(32767)?; r.string(32767)?; r.string(32767)?; } r.done()?; }
                4 => { r.i64()?; r.done()?; }
                5 => { r.i32()?; r.done()?; }
                0 | 6 => {}
                1 => self.custom_payload(&mut r)?,
                8 | 10 | 15 | 16 | 17 => { *self.ignored_presentation.entry(id).or_default() += 1; }
                9 => self.decline_resource_pack(&mut r, 6)?,
                11 => return Err("server transfer unsupported".into()),
                18 | 19 => return Err("configuration dialog/code-of-conduct requires unsupported human UI".into()),
                _ => return Err(format!("unknown mandatory Configuration packet {id}")),
            },
            Phase::Play => self.play(id, &mut r)?,
        }
        Ok(())
    }
    /// ClientboundResourcePackPush. Server packs are never downloaded, so this is vanilla's
    /// declined pack prompt: ServerboundResourcePack DECLINED, then a required pack disconnects.
    fn decline_resource_pack(&mut self, r: &mut Reader<'_>, response_id: i32) -> Result<()> {
        let (response, required) = resource_pack_declined(r)?;
        self.send(response_id, response)?;
        if required {
            return Err(
                "Server requires a custom resource pack (server resource packs are not supported)"
                    .into(),
            );
        }
        Ok(())
    }
    fn custom_payload(&mut self, r: &mut Reader<'_>) -> Result<()> {
        let channel = r.string(32767)?;
        if channel.starts_with("solaris:loader") {
            let payload = r.take(r.bytes.len() - r.offset)?;
            match channel.as_str() {
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
                "solaris:loader/manifest" | "solaris:loader/artifact" | "solaris:loader/view" => {
                    return Err(format!("unsupported Loader channel or phase: {channel}"));
                }
                // Unnegotiated/future plugin channels are optional custom payloads,
                // including the server's capability probe. Never infer activation.
                _ => self.event(json!({"type":"optional_custom_payload_ignored","channel":channel})),
            }
        } else if channel == crate::computer::relay::CHANNEL && self.phase == "play" {
            let payload = r.take(r.bytes.len() - r.offset)?;
            if let Err(error) = self.screen_relay.receive(payload) {
                self.event(json!({"type":"screen_payload_rejected","error":error}));
            }
            self.pump_screen_audio();
        } else if channel == crate::voice::CHANNEL && self.phase == "play" {
            let payload = r.take(r.bytes.len() - r.offset)?;
            if let Err(error) = self.receive_voice(payload) {
                self.event(json!({"type":"voice_payload_rejected","error":error}));
            }
        } else if channel != "minecraft:brand" {
            self.event(json!({"type":"optional_custom_payload_ignored","channel":channel}));
        }
        Ok(())
    }
    fn play(&mut self, id: i32, r: &mut Reader<'_>) -> Result<()> {
        if id == 60 {
            let position = protocol::unpack_pos(r.i64()?);
            let front = r.bool()?;
            r.done()?;
            // Admit the editor before inventory.apply changes screen/fence state.
            // Vanilla ignores an invalid entity without touching the current menu.
            if !self.open_sign_editor(position, front)? {
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
            self.server_frozen = self.environment.frozen;
            if (self.environment.tick_rate - 20.).abs() > 0.001 {
                return Err(format!(
                    "unsupported server simulation rate {}",
                    self.environment.tick_rate
                ));
            }
            if id == 128 {
                self.step_ticks = Reader::new(r.bytes).var()?;
            }
            handled = true;
        }
        if self
            .player
            .apply(id, &mut Reader::new(r.bytes), content, self.client_tick)?
        {
            if id == 104 || id == 68 {
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
            if id == 4 {
                self.reconcile_ack(Reader::new(r.bytes).var()?)?;
            }
            if !self.commands.is_empty() {
                self.flush_commands()?;
            }
            self.consume_ui_packet_commands()?;
            return Ok(());
        }
        match id {
            0 | 12 | 44 | 80 => {}
            32 => return Err(format!("Play disconnect: {}", r.nbt()?)),
            49 => {
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
                r.done()?;
                self.initialize_player_attributes()?;
                if let Some(uuid) = self.player.uuid.as_deref() {
                    self.entities
                        .set_local_player(self.player.entity_id, uuid)?;
                }
            }
            82 => {
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
            45 => {
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
            }
            48 => {
                let position = [r.var()?, r.var()?];
                self.world.update_light(position, r)?;
                if let Some(terrain) = &mut self.terrain {
                    terrain.invalidate_chunk(&self.world, position)?;
                }
            }
            37 => {
                let packed = r.i64()?;
                let position = [packed as i32, (packed >> 32) as i32];
                r.done()?;
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
            8 => {
                let position = protocol::unpack_pos(r.i64()?);
                let state = r.count(1_000_000)? as u32;
                r.done()?;
                self.receive_block(position, state)?;
            }
            84 => {
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
            11 => {
                r.var()?;
                r.done()?;
                let mut w = Writer::default();
                w.f32(4.);
                self.send(11, w)?;
            }
            38 => {
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
            // 26.1.2 ClientboundLevelEvent: raw i32, BlockPos, raw i32, bool.
            46 => {
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
            117 => {
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
            116 => {
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
            119 => {
                if let Some(audio) = &self.audio {
                    audio.stop();
                }
                self.event(json!({"type":"sound_stop","policy":"all_voices_stopped_bounded_C02"}));
            }
            118 => {
                self.phase = "configuration".into();
                self.loader.reset();
                self.screen_relay = Default::default();
                self.screen_audio.clear();
                self.reset_voice();
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
            81 => self.decline_resource_pack(r, 49)?,
            129 => return Err("server transfer unsupported".into()),
            21 => {
                let key = r.string(32767)?;
                r.done()?;
                let mut w = Writer::default();
                w.string(&key);
                w.byte(0);
                self.send(21, w)?;
            }
            24 => self.custom_payload(r)?,
            120 => {
                r.string(32767)?;
                r.array(5120)?;
                r.done()?;
            }
            134 => {
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
            5 => {
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
            2 | 3 | 9 | 10 | 13 | 14 | 15 | 16 | 23 | 26 | 27 | 28 | 29 | 30 | 31 | 33 | 34
            | 43 | 71 | 74 | 75 | 76 | 79 | 83 | 85 | 86 | 87 | 88 | 89 | 90 | 91 | 92 | 94
            | 95 | 97 | 98 | 99 | 100 | 102 | 106 | 109 | 110 | 111 | 112 | 114 | 115 | 121
            | 122 | 123 | 125 | 126 | 130 | 133 | 135 | 136 | 137 | 138 | 139 | 140 => {
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
        let mode = r.u8()?;
        r.u8()?;
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
        // The server announces a dimension's active screens only when the player
        // enters it, so a same-dimension respawn keeps the screen catalog.
        if self.dimension.as_ref() != Some(&dimension) {
            self.screen_relay = Default::default();
            self.screen_audio.clear();
        }
        self.dimension = Some(dimension.clone());
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
        self.predicted_blocks.remove(&position);
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
/// Reads ClientboundResourcePackPush (UUID, url, hash, required, Optional<Component> prompt)
/// and builds ServerboundResourcePack(id, DECLINED); also returns whether it was required.
fn resource_pack_declined(r: &mut Reader<'_>) -> Result<(Writer, bool)> {
    let id = r.take(16)?;
    r.string(32767)?;
    r.string(40)?;
    let required = r.bool()?;
    if r.bool()? {
        r.nbt()?;
    }
    r.done()?;
    let mut response = Writer::default();
    response.0.extend_from_slice(id);
    response.var(1); // Action.DECLINED ordinal
    Ok((response, required))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::world::{Palette, Section, SurfaceHeightmap};

    #[test]
    fn optional_resource_pack_is_declined_and_required_pack_disconnects() {
        let push = |required: bool| {
            let mut w = Writer::default();
            w.0.extend_from_slice(&[7; 16]);
            w.string("https://example.invalid/pack.zip");
            w.string("");
            w.byte(u8::from(required));
            w.byte(1);
            // Prompt component as network NBT: a root string tag.
            w.byte(8);
            w.0.extend_from_slice(&2u16.to_be_bytes());
            w.0.extend_from_slice(b"hi");
            w.0
        };
        let (response, required) = resource_pack_declined(&mut Reader::new(&push(false))).unwrap();
        assert!(!required);
        let mut expected = vec![7u8; 16];
        expected.push(1);
        assert_eq!(response.0, expected);
        for (phase, label, packet) in [
            (Phase::Configuration, "configuration", 9),
            (Phase::Play, "play", 81),
        ] {
            let mut session = Session::new_mode("resource-pack".into(), false);
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap().to_string();
            session.network = Some(
                crate::protocol::network::Network::connect(
                    address,
                    "Packs".into(),
                    8,
                    Arc::new(|| {}),
                )
                .unwrap(),
            );
            session.content = Some(Content::test_cube());
            session.phase = label.into();
            session.apply(phase, packet, &push(false)).unwrap();
            assert_eq!(
                session.phase, label,
                "an optional pack must not end the connection"
            );
            let error = session.apply(phase, packet, &push(true)).unwrap_err();
            assert!(error.contains("requires a custom resource pack"), "{error}");
        }
    }

    #[test]
    fn same_dimension_respawn_keeps_the_active_screen_catalog() {
        let mut session = Session::new_mode("respawn-screens".into(), false);
        let mut content = Content::test_cube();
        content.set_registries(BTreeMap::from([(
            "minecraft:dimension_type".to_owned(),
            vec![(
                "minecraft:overworld".to_owned(),
                json!({"min_y": 0, "height": 16, "has_skylight": true, "has_ceiling": false, "attributes": {}}),
            )],
        )]));
        session.content = Some(content);
        session.phase = "play".into();
        let respawn = |dimension: &str| {
            let mut w = Writer::default();
            w.var(0);
            w.string(dimension);
            w.0.extend_from_slice(&0i64.to_be_bytes());
            w.0.extend_from_slice(&[0, 255, 0, 0, 0]); // mode, previous, debug, flat, no death location
            w.var(0);
            w.var(63);
            w.byte(1); // keep attributes: this fixture has no entity models
            w.0
        };
        session
            .apply(Phase::Play, 82, &respawn("minecraft:overworld"))
            .unwrap();
        // ACTIVE(on) for a 1x1 monitor at the origin, overworld.
        let mut active = vec![1, 6];
        active.extend_from_slice(&[0; 12]);
        active.extend_from_slice(&[2, 1, 1, 0, 1]);
        session.screen_relay.receive(&active).unwrap();
        session
            .apply(Phase::Play, 82, &respawn("minecraft:overworld"))
            .unwrap();
        assert_eq!(
            session.screen_relay.active.len(),
            1,
            "the server will not announce it again"
        );
        session
            .apply(Phase::Play, 82, &respawn("minecraft:the_nether"))
            .unwrap();
        assert!(
            session.screen_relay.active.is_empty(),
            "a new dimension gets its own catalog"
        );
    }

    #[test]
    fn optional_loader_probe_and_unknown_channels_do_not_disconnect() {
        for (phase, label, packet) in [(Phase::Configuration, "configuration", 1), (Phase::Play, "play", 24)] {
            for channel in ["solaris:loader/probe", "solaris:loader/future", "example:optional"] {
                let mut session = Session::new_mode("optional-payload".into(), false);
                session.content = Some(Content::test_cube());
                session.phase = label.into();
                let mut writer = Writer::default();
                writer.string(channel);
                writer.0.extend_from_slice(&[1, 255, 0, 17]);
                session.apply(phase, packet, &writer.0).unwrap();
                assert_eq!(session.phase, label);
                assert_eq!(session.events.back().unwrap()["channel"], channel);
                assert_eq!(session.events.back().unwrap()["type"], "optional_custom_payload_ignored");
            }
        }
    }

    #[test]
    fn recognized_loader_channel_still_requires_its_protocol_phase() {
        let mut session = Session::new_mode("wrong-loader-phase".into(), false);
        session.content = Some(Content::test_cube());
        session.phase = "play".into();
        let mut writer = Writer::default();
        writer.string("solaris:loader/manifest");
        assert!(session.apply(Phase::Play, 24, &writer.0).unwrap_err().contains("phase"));
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
                46,
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
            session.apply(Phase::Play, 46, &body).unwrap();
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
            assert_eq!(session.ignored_presentation.get(&46), None);
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
            assert!(session.apply(Phase::Play, 46, &body[..length]).is_err());
            assert_eq!(session.events.len(), before_events);
            assert_eq!(session.state_version(), before_version);
        }
        let mut trailing = body;
        trailing.push(0);
        assert!(session.apply(Phase::Play, 46, &trailing).is_err());
        assert_eq!(session.events.len(), before_events);
        assert_eq!(session.state_version(), before_version);
        let error = session.apply(Phase::Play, 141, &[]).unwrap_err();
        assert_eq!(error, "unknown mandatory Play packet 141");
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
        writer.byte(u8::from(front));
        writer.0
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
                    .apply(Phase::Play, 60, &open_sign_bytes(position, front))
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
            .apply(Phase::Play, 60, &open_sign_bytes(position, false))
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
            .apply(Phase::Play, 60, &open_sign_bytes(position, true))
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
            .apply(Phase::Play, 60, &open_sign_bytes(position, true))
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
                    .apply(Phase::Play, 60, &open_sign_bytes(position, front))
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
            .apply(Phase::Play, 60, &open_sign_bytes(position, true))
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
                .apply(Phase::Play, 60, &open_sign_bytes(position, true))
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
        session.apply(Phase::Play, 59, &menu.0).unwrap();
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
            .apply(Phase::Play, 60, &open_sign_bytes(position, true))
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
        session.apply(Phase::Play, 45, &packet.0).unwrap();
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
