use super::*;

impl Renderer {
    pub(super) fn collect_completed(&mut self) {
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.timing.collect();
        if let Some(mdi) = &mut self.mdi {
            mdi.collect();
        }
        let completed = self.completed_frame.load(Ordering::Acquire);
        self.vertex_arena.reclaim(completed);
        self.index_arena.reclaim(completed);
        self.retired_textures.retain(|t| t.frame > completed);
    }

    pub(super) fn sync_scene(
        &mut self,
        scene: &Scene,
        observation: &Observation,
    ) -> Result<(), String> {
        if scene.chunks.len() > crate::terrain::section_limit(scene.render_distance) {
            return Err("terrain_chunk_residency_limit".into());
        }
        for chunk in scene.chunks.values() {
            if chunk
                .fade_duration_ms
                .is_some_and(|duration| duration > 2000)
            {
                return Err("invalid_source_section_fade_duration".into());
            }
        }
        let generation = [
            &observation.connection_epoch,
            &observation.dimension_epoch,
            &observation.resource_epoch,
        ];
        let changed_generation = self
            .generation
            .as_ref()
            .is_none_or(|old| (0..3).any(|i| &old[i] != generation[i]));
        let texture_changed =
            changed_generation || self.texture_revision != Some(scene.texture_revision);
        let layers = if scene.texture_rgba.is_some() {
            scene.texture_layers.max(1)
        } else {
            1
        };
        let force = changed_generation || texture_changed;
        let same_world = self.generation.as_ref().is_some_and(|old| {
            old[0] == observation.connection_epoch && old[1] == observation.dimension_epoch
        });
        let limits = self.device.limits();
        let (pixels, width, height) = if let Some(pixels) = &scene.texture_rgba {
            (pixels.as_slice(), scene.texture_width, scene.texture_height)
        } else {
            (&[255; 4][..], 1, 1)
        };
        let mut texture_bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(u64::from(layers)))
            .and_then(|n| n.checked_mul(4))
            .ok_or("texture_size_overflow")?;
        if width == 0
            || height == 0
            || width > limits.max_texture_dimension_2d
            || height > limits.max_texture_dimension_2d
            || layers > limits.max_texture_array_layers
            || pixels.len() as u64 != texture_bytes
        {
            return Err("invalid_scene_texture_array_or_adapter_limit".into());
        }
        let mips = scene
            .texture_mips
            .as_deref()
            .map_or(&[][..], |mips| mips.as_slice());
        if mips.len() > 4 {
            return Err("texture_mipmap_count_limit".into());
        }
        for (index, mip) in mips.iter().enumerate() {
            if mip.level != index as u32 + 1
                || mip.level > width.max(height).ilog2()
                || mip.width != (width >> mip.level).max(1)
                || mip.height != (height >> mip.level).max(1)
                || mip.rgba.len() as u64
                    != u64::from(mip.width) * u64::from(mip.height) * u64::from(layers) * 4
            {
                return Err("invalid_texture_mipmap_dimensions_or_sequence".into());
            }
            texture_bytes = texture_bytes
                .checked_add(mip.rgba.len() as u64)
                .ok_or("texture_size_overflow")?;
        }
        let retired = self.retired_textures.iter().map(|t| t.bytes).sum::<u64>();
        if texture_bytes > 256 * 1024 * 1024
            || (texture_changed && texture_bytes + self.texture_bytes + retired > 512 * 1024 * 1024)
        {
            return Err(format!(
                "texture_reload_memory_budget_exceeded active={} retired={retired} staging={texture_bytes}",
                self.texture_bytes
            ));
        }
        if scene.texture_patches.len() > 16384 {
            return Err("texture_animation_patch_count_limit".into());
        }
        let mut patch_bytes = 0u64;
        for patch in &scene.texture_patches {
            if patch.mip_level > mips.len() as u32 {
                return Err("invalid_texture_animation_mip_level".into());
            }
            let bytes = u64::from(patch.size[0])
                .checked_mul(u64::from(patch.size[1]))
                .and_then(|n| n.checked_mul(4))
                .ok_or("texture_animation_size_overflow")?;
            if patch.size.contains(&0)
                || patch.layer >= layers
                || patch.origin[0]
                    .checked_add(patch.size[0])
                    .is_none_or(|end| end > (width >> patch.mip_level).max(1))
                || patch.origin[1]
                    .checked_add(patch.size[1])
                    .is_none_or(|end| end > (height >> patch.mip_level).max(1))
                || bytes != patch.rgba.len() as u64
            {
                return Err("invalid_texture_animation_patch".into());
            }
            patch_bytes = patch_bytes
                .checked_add(bytes)
                .ok_or("texture_animation_size_overflow")?;
        }
        if patch_bytes > 64 * 1024 * 1024 {
            return Err("texture_animation_patch_byte_limit".into());
        }
        for texture in [
            scene.environment.sun_texture,
            scene.environment.moon_texture,
            scene.environment.end_sky_texture,
            scene.environment.end_flash_texture,
        ]
        .into_iter()
        .flatten()
        {
            if texture.layer >= layers
                || !texture
                    .uv
                    .iter()
                    .all(|v| v.is_finite() && *v >= 0. && *v <= 1.)
            {
                return Err("invalid_celestial_texture_region".into());
            }
        }
        let env = &scene.environment;
        if ![
            env.fog_start,
            env.fog_end,
            env.sky_fog_end,
            env.render_fog_start,
            env.render_fog_end,
            env.fog_alpha,
            env.sun_angle,
            env.moon_angle,
            env.star_angle,
            env.star_brightness,
            env.rain,
            env.thunder,
            env.end_flash_intensity,
            env.end_flash_x_angle,
            env.end_flash_y_angle,
        ]
        .iter()
        .chain(env.sky_color.iter())
        .chain(env.fog_color.iter())
        .chain(env.sunrise_color.iter())
        .all(|v| v.is_finite())
            || !scene
                .camera_position
                .iter()
                .chain(scene.origin.iter())
                .all(|v| v.is_finite())
            || !scene.yaw.is_finite()
            || !scene.pitch.is_finite()
        {
            return Err("non_finite_world_environment_or_camera".into());
        }
        if env.sky_kind == SkyKind::Overworld
            && (env.sun_texture.is_none() || env.moon_texture.is_none())
        {
            return Err("missing_required_overworld_celestial_texture".into());
        }
        if env.sky_kind == SkyKind::End
            && (env.end_sky_texture.is_none()
                || (env.end_flash_intensity > 0. && env.end_flash_texture.is_none()))
        {
            return Err("missing_required_end_sky_texture".into());
        }
        let directional = scene
            .dynamic
            .indices
            .iter()
            .enumerate()
            .any(|(family, indices)| {
                !indices.is_empty() && material::MATERIALS[family].shader.cardinal
            });
        if directional && env.light_directions.is_none() {
            return Err("missing_required_source_cardinal_light_directions".into());
        }
        if env
            .light_directions
            .is_some_and(|directions| directions.iter().flatten().any(|v| !v.is_finite()))
        {
            return Err("non_finite_source_cardinal_light_directions".into());
        }
        let portal = scene
            .dynamic
            .indices
            .iter()
            .enumerate()
            .any(|(family, indices)| {
                !indices.is_empty() && material::MATERIALS[family].shader.kind == 12
            });
        if portal && (scene.portal_sprites.is_none() || scene.shader_game_time.is_none()) {
            return Err("missing_required_source_portal_sprites_or_game_time".into());
        }
        if scene.shader_game_time.is_some_and(|time| !time.is_finite()) {
            return Err("non_finite_source_shader_game_time".into());
        }
        if let Some(sprites) = scene.portal_sprites {
            for sprite in sprites {
                if sprite.layer >= layers
                    || !sprite
                        .uv
                        .iter()
                        .all(|v| v.is_finite() && *v >= 0. && *v <= 1.)
                    || sprite.uv[0] >= sprite.uv[2]
                    || sprite.uv[1] >= sprite.uv[3]
                {
                    return Err("invalid_source_portal_sprite_region".into());
                }
            }
        }
        // Validate independent presentation streams before any resident replacement.
        Stream::validate_size(&scene.dynamic.vertices, &scene.dynamic.indices, layers)?;
        Stream::validate_size(&scene.overlay.vertices, &scene.overlay.indices, layers)?;
        if scene.lightmap.is_none()
            && scene
                .vertices
                .iter()
                .chain(scene.dynamic.vertices.iter())
                .chain(
                    scene
                        .chunks
                        .values()
                        .flat_map(|chunk| chunk.geometry.iter().flat_map(|g| g.vertices.iter())),
                )
                .any(|vertex| vertex.light.iter().all(|level| *level >= 0.))
        {
            return Err("missing_required_causal_world_lightmap".into());
        }
        let mut staged = std::mem::take(&mut self.staged_chunks);
        let mut staged_fixture = None;
        let preparation = (|| -> Result<(), String> {
            let mut transparent_count = scene
                .dynamic
                .indices
                .iter()
                .enumerate()
                .filter(|(i, _)| material::MATERIALS[*i].sort)
                .map(|(_, indices)| indices.len() / 3)
                .sum::<usize>();
            // Both maps are ordered by key: one merged walk finds each
            // section's resident counterpart without a per-section lookup
            // (a 32-chunk view keeps ~45k sections resident every frame).
            let mut residents = self.chunks.iter().peekable();
            for (&key, chunk) in &scene.chunks {
                if !chunk.origin.iter().all(|v| v.is_finite()) {
                    return Err("non_finite_chunk_origin".into());
                }
                while residents.next_if(|(resident, _)| **resident < key).is_some() {}
                let resident = residents
                    .next_if(|(resident, _)| **resident == key)
                    .map(|(_, chunk)| chunk);
                if let Some(old) = resident.filter(|old| old.revision == chunk.revision) {
                    // Atlas-only changes preserve an already-uploaded section;
                    // connection/dimension changes never reuse a released payload.
                    if !force || (same_world && chunk.geometry.is_none()) {
                        transparent_count += old.mesh.transparent.len();
                        continue;
                    }
                }
                let Some(geometry) = chunk.geometry.as_ref() else {
                    return Err(format!(
                        "terrain_cpu_payload_missing key={key:?} revision={} resident={} same_world={same_world}",
                        chunk.revision,
                        resident.map_or_else(|| "absent".to_string(), |old| old.revision.to_string()),
                    ));
                };
                mesh::validate(
                    &geometry.vertices,
                    &[
                        &geometry.indices[0],
                        &geometry.indices[1],
                        &geometry.indices[2],
                    ],
                    layers,
                )?;
                let packed = packed::pack_vertices(&geometry.vertices, &mut self.tiles)?;
                let mesh = ResidentMesh::reserve(
                    &self.device,
                    &self.queue,
                    &mut self.vertex_arena,
                    &mut self.index_arena,
                    &geometry.vertices,
                    [
                        &geometry.indices[0],
                        &geometry.indices[1],
                        &geometry.indices[2],
                    ],
                    geometry.faces,
                )?;
                transparent_count += mesh.transparent.len();
                staged.push((
                    key,
                    ResidentChunk {
                        revision: chunk.revision,
                        origin: chunk.origin,
                        bounds_min: chunk.bounds_min,
                        bounds_max: chunk.bounds_max,
                        visibility_mask: chunk.visibility,
                        fade_duration_ms: chunk.fade_duration_ms,
                        uploaded_time_ms: if changed_generation {
                            None
                        } else {
                            resident.and_then(|old| old.uploaded_time_ms)
                        },
                        visibility: None,
                        mesh,
                    },
                    packed,
                ));
            }
            if transparent_count > mesh::MAX_TRANSPARENT_PRIMITIVES {
                return Err("transparent_scene_primitive_budget_exceeded".into());
            }
            if (force || self.mesh_revision != Some(scene.revision))
                && (!scene.vertices.is_empty() || !scene.indices.is_empty())
            {
                mesh::validate(&scene.vertices, &[&scene.indices], layers)?;
                let packed = packed::pack_vertices(&scene.vertices, &mut self.tiles)?;
                let mesh = ResidentMesh::reserve(
                    &self.device,
                    &self.queue,
                    &mut self.vertex_arena,
                    &mut self.index_arena,
                    &scene.vertices,
                    [&scene.indices, &[], &[]],
                    [None, None],
                )?;
                staged_fixture = Some((mesh, packed));
            }
            Ok(())
        })();
        if let Err(error) = preparation {
            for (_, chunk, _) in staged.drain(..) {
                chunk
                    .mesh
                    .release(&mut self.vertex_arena, &mut self.index_arena);
            }
            if let Some((mesh, _)) = staged_fixture {
                mesh.release(&mut self.vertex_arena, &mut self.index_arena);
            }
            self.staged_chunks = staged;
            return Err(error);
        }
        let staged_texture = if texture_changed {
            Some(Self::atlas(
                &self.device,
                &self.queue,
                &self.pipelines.atlas_layout,
                &self.lightmap_texture,
                &self.entity_overlay_texture,
                &self.tile_texture,
                pixels,
                mips,
                width,
                height,
                layers,
            ))
        } else {
            None
        };
        self.dynamic.upload(
            &self.device,
            &self.queue,
            &scene.dynamic.vertices,
            &scene.dynamic.indices,
            material::MATERIALS.map(|m| m.sort),
        )?;
        self.overlay.upload(
            &self.device,
            &self.queue,
            &scene.overlay.vertices,
            &scene.overlay.indices,
            [false; 3],
        )?;
        sky::sunrise(&mut self.sunrise_geometry, &scene.environment);
        self.sunrise.upload(
            &self.device,
            &self.queue,
            &self.sunrise_geometry.vertices,
            &self.sunrise_geometry.indices,
            [false; 3],
        )?;
        self.stats.terrain_upload_bytes += self.tiles.flush(&self.queue, &self.tile_texture);
        if self.reference_mode && !force {
            // The reference path submits full geometry bytes each frame. Queue
            // writes execute after prior submissions and before this frame;
            // unchanged topology does not need three copies of the same scene.
            for (key, source) in &scene.chunks {
                let Some(old) = self
                    .chunks
                    .get(key)
                    .filter(|old| old.revision == source.revision)
                else {
                    continue;
                };
                // Released payloads (None) are already resident at this revision;
                // reference mode keeps deliberate full-upload bytes only while the
                // parent retains the source.
                let Some(geometry) = source.geometry.as_ref() else {
                    continue;
                };
                let packed = packed::pack_vertices(&geometry.vertices, &mut self.tiles)?;
                old.mesh.upload_geometry(
                    &self.queue,
                    &self.vertex_arena,
                    &self.index_arena,
                    &packed,
                    [
                        &geometry.indices[0],
                        &geometry.indices[1],
                        &geometry.indices[2],
                    ],
                );
                self.stats.terrain_upload_bytes += old.mesh.bytes();
                self.stats.terrain_uploaded_chunks += 1;
            }
            if self.mesh_revision == Some(scene.revision) {
                if let Some(mesh) = &self.fixture {
                    let packed = packed::pack_vertices(&scene.vertices, &mut self.tiles)?;
                    mesh.upload_geometry(
                        &self.queue,
                        &self.vertex_arena,
                        &self.index_arena,
                        &packed,
                        [&scene.indices, &[], &[]],
                    );
                    self.stats.terrain_upload_bytes += mesh.bytes();
                }
            }
        }
        let mut removed = false;
        for (key, chunk, packed) in staged.drain(..) {
            let ack = (key, chunk.revision);
            let Some(geometry) = scene
                .chunks
                .get(&key)
                .and_then(|draw| draw.geometry.as_ref())
            else {
                return Err("terrain_cpu_payload_vanished_during_commit".into());
            };
            chunk.mesh.upload_geometry(
                &self.queue,
                &self.vertex_arena,
                &self.index_arena,
                &packed,
                [
                    &geometry.indices[0],
                    &geometry.indices[1],
                    &geometry.indices[2],
                ],
            );
            self.stats.terrain_upload_bytes += chunk.mesh.bytes();
            self.stats.terrain_uploaded_chunks += 1;
            let resident = match self.chunks.entry(key) {
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    let old = entry.insert(chunk);
                    old.mesh
                        .retire(&mut self.vertex_arena, &mut self.index_arena, self.frame_id);
                    entry.into_mut()
                }
                std::collections::btree_map::Entry::Vacant(entry) => {
                    let (min, max) = self.chunk_extents.get_or_insert((key, key));
                    for axis in 0..3 {
                        min[axis] = min[axis].min(key[axis]);
                        max[axis] = max[axis].max(key[axis]);
                    }
                    entry.insert(chunk)
                }
            };
            // The all-layer upload and resident mapping have now committed. Source
            // starts here once per section reset, never at worker/fence completion.
            if resident.uploaded_time_ms.is_none() {
                resident.uploaded_time_ms =
                    Some(self.monotonic_origin.elapsed().as_millis() as u64);
            }

            // Record the exact ack only after bytes + mapping committed.
            // Reference mode omits acks so its deliberate full-upload source
            // stays retained for per-frame rewrites.
            if !self.reference_mode {
                self.uploaded_meshes.push(ack);
            }
        }
        if let Some((mesh, packed)) = staged_fixture {
            mesh.upload_geometry(
                &self.queue,
                &self.vertex_arena,
                &self.index_arena,
                &packed,
                [&scene.indices, &[], &[]],
            );
            self.stats.terrain_upload_bytes += mesh.bytes();
            if let Some(old) = self.fixture.replace(mesh) {
                old.retire(&mut self.vertex_arena, &mut self.index_arena, self.frame_id);
            }
        }
        let vertex_arena = &mut self.vertex_arena;
        let index_arena = &mut self.index_arena;
        let stats = &mut self.stats;
        let frame = self.frame_id;
        // `retain` visits keys in ascending order: walk the sources alongside.
        let mut sources = scene.chunks.iter().peekable();
        self.chunks.retain(|key, chunk| {
            while sources.next_if(|(source, _)| *source < key).is_some() {}
            if let Some((_, source)) = sources.next_if(|(source, _)| *source == key) {
                chunk.fade_duration_ms = source.fade_duration_ms;
                chunk.visibility_mask = source.visibility;
                return true;
            }
            let empty = ResidentMesh {
                faces: [None, None],
                vertices: None,
                indices: None,
                ranges: std::array::from_fn(|_| 0..0),
                transparent: Default::default(),
            };
            std::mem::replace(&mut chunk.mesh, empty).retire(vertex_arena, index_arena, frame);
            stats.terrain_freed_chunks += 1;
            removed = true;
            false
        });
        if removed {
            // Removal can shrink the extents; insertion only widened them.
            self.chunk_extents = visibility::extents(self.chunks.keys().copied());
        }
        if let Some((texture, bind)) = staged_texture {
            self.retired_textures.push(RetiredTexture {
                frame: self.frame_id,
                bytes: self.texture_bytes,
                _texture: std::mem::replace(&mut self.atlas_texture, texture),
                _bind: std::mem::replace(&mut self.atlas_bind, bind),
            });
            self.texture_bytes = texture_bytes;
            self.texture_layers = layers;
            self.stats.texture_upload_bytes = texture_bytes;
            self.texture_revision = Some(scene.texture_revision);
            self.texture_activation_frame = None;
            self.applied_patches.clear();
        }
        for patch in &scene.texture_patches {
            let key = (patch.mip_level, patch.layer, patch.origin, patch.size);
            if self.applied_patches.get(&key) == Some(&patch.sequence) {
                continue;
            }
            self.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.atlas_texture,
                    mip_level: patch.mip_level,
                    origin: wgpu::Origin3d {
                        x: patch.origin[0],
                        y: patch.origin[1],
                        z: patch.layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &patch.rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(patch.size[0] * 4),
                    rows_per_image: Some(patch.size[1]),
                },
                wgpu::Extent3d {
                    width: patch.size[0],
                    height: patch.size[1],
                    depth_or_array_layers: 1,
                },
            );
            self.stats.texture_upload_bytes += patch.rgba.len() as u64;
            self.applied_patches.insert(key, patch.sequence);
        }
        if let Some(lightmap) = &scene.lightmap {
            if changed_generation || self.lightmap_revision != Some(lightmap.revision) {
                self.queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.lightmap_texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    lightmap.rgba.as_slice(),
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(64),
                        rows_per_image: Some(16),
                    },
                    wgpu::Extent3d {
                        width: 16,
                        height: 16,
                        depth_or_array_layers: 1,
                    },
                );
                self.stats.texture_upload_bytes += 1024;
                self.lightmap_revision = Some(lightmap.revision);
            }
        } else {
            self.lightmap_revision = None;
        }
        if changed_generation {
            self.generation = Some(generation.map(|s| s.clone()));
        }
        self.mesh_revision = Some(scene.revision);
        self.staged_chunks = staged;
        if scene.vertices.is_empty() && scene.indices.is_empty() {
            if let Some(mesh) = self.fixture.take() {
                mesh.retire(&mut self.vertex_arena, &mut self.index_arena, self.frame_id);
            }
        }
        Ok(())
    }
}
