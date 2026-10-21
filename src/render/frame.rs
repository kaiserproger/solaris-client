use super::*;

/// Plane-test margin for terrain facing runs: shader waving displaces
/// vegetation vertices by at most 0.09 blocks.
const FACING_SLACK: f32 = 0.125;
/// Rooted waving vegetation (grass, flowers, crops) is drawn only in sections
/// whose nearest point is within this many blocks of the camera. Beyond it a
/// tuft covers a few pixels and its quads are the largest cutout cost per
/// visible pixel.
const VEGETATION_RANGE: f32 = 128.;
/// Blocks past a fog end a section's nearest point must lie before it is
/// skipped as fully fogged (covers shader wave displacement and rounding).
const FOG_MARGIN: f32 = 1.;

impl Renderer {
    fn prepare_visibility(&mut self, scene: &Scene, view_projection: Mat4) -> Result<(), String> {
        let start = Instant::now();
        self.visible.clear();
        for runs in &mut self.terrain_runs {
            runs.clear();
        }
        self.instance_data.clear();
        if let Some(mdi) = &mut self.mdi {
            for candidates in &mut mdi.candidates {
                candidates.clear();
            }
        }
        self.instance_data.push([0., 0., 0., 1.]);
        let presentation_origin = relative(scene.origin, scene.camera_position);
        self.instance_data.push([
            presentation_origin.x,
            presentation_origin.y,
            presentation_origin.z,
            1.,
        ]);
        let frustum = Frustum::new(view_projection);
        self.transparent.clear();
        self.visibility_time_ms = self.monotonic_origin.elapsed().as_millis() as u64;
        self.stats.unknown_fade_policy_sections = 0;
        // Cull outcomes belong to this visibility pass: reset per call so a
        // repeated pose reproduces identical counts. Upload bytes below keep
        // accumulating: they measure work done, not this pass's decision.
        self.stats.culled_chunks = 0;
        self.stats.frustum_rejected_chunks = 0;
        self.stats.graph_rejected_chunks = 0;
        self.stats.fog_rejected_chunks = 0;
        let graph_enabled = !self.reference_mode && visibility::occlusion_enabled();
        self.stats.graph_enabled = graph_enabled;
        let graph_available = graph_enabled
            && visibility::extents(self.chunks.keys().copied()).is_some_and(|bounds| {
                visibility::flood(
                    scene.camera_position.map(|v| (v / 16.).floor() as i32),
                    bounds,
                    |key| self.chunks.get(&key).map(|chunk| chunk.visibility_mask),
                    |key| {
                        if let Some(chunk) = self.chunks.get(&key) {
                            let offset = relative(chunk.origin, scene.camera_position);
                            frustum.intersects(
                                Vec3::from_array(chunk.bounds_min) + offset,
                                Vec3::from_array(chunk.bounds_max) + offset,
                            )
                        } else {
                            let origin = key.map(|v| f64::from(v) * 16.);
                            let min = relative(origin, scene.camera_position);
                            frustum.intersects(min, min + Vec3::splat(16.))
                        }
                    },
                    &mut self.occlusion_reached,
                    &mut self.occlusion_queue,
                )
            });
        self.stats.graph_available = graph_available;
        self.stats.graph_reached_sections = if graph_available {
            self.occlusion_reached.len() as u64
        } else {
            0
        };
        let occlusion = graph_available;
        // Fog distances after which `fogged()` returns exactly the sky colour
        // (full strength, ramp present). Only the Fancy overworld shades far
        // terrain with the sky ray; elsewhere fog colour differs from the sky.
        let env = &scene.environment;
        let fully_fogged = !self.reference_mode
            && self.fancy_shaders
            && env.sky_kind == SkyKind::Overworld
            && env.fog_alpha >= 1.;
        let fog_sphere = (env.fog_start < env.fog_end).then_some(env.fog_end + FOG_MARGIN);
        let fog_cylinder = (env.render_fog_start < env.render_fog_end)
            .then_some(env.render_fog_end + FOG_MARGIN);
        for (&key, chunk) in &mut self.chunks {
            if chunk.fade_duration_ms.is_none() {
                self.stats.unknown_fade_policy_sections += 1;
            }
            chunk.visibility = match (chunk.uploaded_time_ms, chunk.fade_duration_ms) {
                (Some(uploaded), Some(duration)) => {
                    let delta = self.visibility_time_ms - uploaded;
                    Some(if delta >= duration {
                        1.
                    } else {
                        delta as f32 / duration as f32
                    })
                }
                _ => None,
            };
            if chunk.mesh.indices.is_none() {
                continue;
            }
            let Some(visibility) = chunk.visibility else {
                // Unknown compile policy can become GPU-resident but cannot draw.
                continue;
            };
            let offset = relative(chunk.origin, scene.camera_position);
            // shader::waving displaces Fancy cutout vegetation at most
            // 0.09 blocks in X and 0.0495 in Z; preserve border silhouettes.
            let wind = if self.fancy_shaders
                && scene.environment.sky_kind == SkyKind::Overworld
                && !chunk.mesh.ranges[1].is_empty()
            {
                Vec3::new(0.09, 0., 0.05)
            } else {
                Vec3::ZERO
            };
            let bounds_min = Vec3::from_array(chunk.bounds_min) - wind;
            let bounds_max = Vec3::from_array(chunk.bounds_max) + wind;
            let in_frustum = self.reference_mode
                || frustum.intersects(bounds_min + offset, bounds_max + offset);
            if !self.reference_mode
                && occlusion
                && !visibility::reaches_bounds(key, bounds_min, bounds_max, &self.occlusion_reached)
            {
                self.stats.culled_chunks += 1;
                self.stats.graph_rejected_chunks += 1;
                continue;
            }
            // Frustum rejection precedes every per-frame submission: sections
            // outside the frustum take no instance slot, no indirect candidate
            // and no transparent sort. The GPU cull pass stays the backstop for
            // stale CPU planes, but it must not be the only filter.
            if !in_frustum {
                self.stats.culled_chunks += 1;
                self.stats.frustum_rejected_chunks += 1;
                continue;
            }
            // Nearest point of the section bounds to the camera (the origin).
            let nearest = Vec3::ZERO
                .max(bounds_min + offset)
                .min(bounds_max + offset);
            // A section wholly past either fog end (spherical or the render
            // distance cylinder) is painted pure sky by the Fancy shader, so
            // drawing it only overpaints identical sky pixels. This is the
            // square-chunk-window corners at the horizon.
            if fully_fogged
                && ((fog_sphere.is_some_and(|end| nearest.length() >= end))
                    || fog_cylinder
                        .is_some_and(|end| nearest.x.hypot(nearest.z).max(nearest.y.abs()) >= end))
            {
                self.stats.culled_chunks += 1;
                self.stats.fog_rejected_chunks += 1;
                continue;
            }
            let instance = self.instance_data.len() as u32;
            self.instance_data
                .push([offset.x, offset.y, offset.z, visibility]);
            self.visible.push(Visible { key, instance });
            let local: [f32; 3] =
                std::array::from_fn(|axis| (scene.camera_position[axis] - chunk.origin[axis]) as f32);
            let vegetation = nearest.length_squared() <= VEGETATION_RANGE * VEGETATION_RANGE;
            for material in 0..2 {
                let runs = &mut self.terrain_runs[material];
                let first = runs.len();
                if self.reference_mode {
                    // Reference path: whole layers, no facing partition.
                    let range = chunk.mesh.ranges[material].clone();
                    if !range.is_empty() {
                        runs.push(TerrainRun { key, instance, range });
                    }
                } else {
                    chunk
                        .mesh
                        .facing_runs(material, local, FACING_SLACK, vegetation, |range| {
                            runs.push(TerrainRun { key, instance, range })
                        });
                    if runs.len() > mesh::MAX_CHUNKS {
                        // Candidate buffers are bounded: fall back to the whole layer.
                        runs.truncate(first);
                        runs.push(TerrainRun {
                            key,
                            instance,
                            range: chunk.mesh.ranges[material].clone(),
                        });
                    }
                }
                if !self.reference_mode
                    && let Some(mdi) = &mut self.mdi
                {
                    for run in &self.terrain_runs[material][first..] {
                        if let Some(candidate) = mdi::Candidate::new(
                            &chunk.mesh,
                            run.range.clone(),
                            instance,
                            bounds_min + offset,
                            bounds_max + offset,
                            mdi::section_slot(key),
                        ) {
                            mdi.candidates[material].push(candidate);
                        }
                    }
                }
            }
            let range = chunk.mesh.ranges[2].clone();
            if !range.is_empty() {
                self.stats.terrain_upload_bytes += chunk.mesh.sort_transparent(
                    &self.queue,
                    &self.index_arena,
                    std::array::from_fn(|axis| scene.camera_position[axis] - chunk.origin[axis]),
                );
                let center =
                    (Vec3::from_array(chunk.bounds_min) + Vec3::from_array(chunk.bounds_max)) * 0.5
                        + offset;
                self.transparent.push(TransparentDraw {
                    source: Source::Chunk(key),
                    instance,
                    first: range.start,
                    count: range.end - range.start,
                    distance: center.length_squared(),
                    order: self.transparent.len() as u32,
                });
            }
        }
        for (material, primitives) in self.dynamic.transparent.iter().enumerate() {
            for primitive in primitives {
                self.transparent.push(TransparentDraw {
                    source: Source::Dynamic(material),
                    instance: 1,
                    first: primitive.first,
                    count: primitive.count,
                    distance: (Vec3::from_array(primitive.center) + presentation_origin)
                        .length_squared(),
                    order: self.transparent.len() as u32,
                });
            }
        }
        if self.transparent.len() > mesh::MAX_TRANSPARENT_PRIMITIVES {
            return Err("transparent_draw_descriptor_budget_exceeded".into());
        }
        // Terrain sections sort far-to-near, with their quads sorted in-place
        // on camera movement. Dynamic RenderType sorting remains independent.
        self.transparent.sort_unstable_by(|a, b| {
            a.source
                .bucket()
                .cmp(&b.source.bucket())
                .then_with(|| b.distance.total_cmp(&a.distance))
                .then_with(|| a.order.cmp(&b.order))
        });
        let mut cursor = 0;
        for (bucket, range) in self.transparent_ranges.iter_mut().enumerate() {
            range.start = cursor;
            while cursor < self.transparent.len()
                && self.transparent[cursor].source.bucket() == bucket
            {
                cursor += 1;
            }
            range.end = cursor;
        }
        self.stats.visible_chunks = self.visible.len() as u64;
        if let Some(mdi) = &self.mdi {
            self.stats.terrain_candidates_opaque = mdi.candidates[0].len() as u64;
            self.stats.terrain_candidates_cutout = mdi.candidates[1].len() as u64;
        } else {
            self.stats.terrain_candidates_opaque = 0;
            self.stats.terrain_candidates_cutout = 0;
        }
        self.queue.write_buffer(
            &self.instances,
            0,
            bytemuck::cast_slice(&self.instance_data),
        );
        self.stats.visibility_ms = start.elapsed().as_secs_f64() * 1000.;
        Ok(())
    }
    fn draw_resident(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        mesh: &ResidentMesh,
        range: std::ops::Range<u32>,
        instance: u32,
    ) {
        if range.is_empty() {
            return;
        }
        if let (Some(vertices), Some(indices)) = (mesh.vertices, mesh.indices) {
            pass.set_vertex_buffer(0, self.vertex_arena.slice(vertices));
            pass.set_index_buffer(self.index_arena.slice(indices), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(range, 0, instance..instance + 1);
        }
    }

    pub(super) fn encode_scene(
        &mut self,
        scene: &Scene,
        observation: &Observation,
        frame: Option<&wgpu::SurfaceTexture>,
    ) -> Result<wgpu::CommandEncoder, String> {
        let upload = Instant::now();
        self.sync_scene(scene, observation)?;
        self.stats.upload_ms = upload.elapsed().as_secs_f64() * 1000.;
        // Dynamic categories share material draws, but their extraction ranges
        // are disjoint and every dynamic index is submitted once.
        self.stats.entity_geometry_indices = scene.entity_presentation.entity_indices as u64;
        self.stats.block_entity_geometry_indices =
            scene.entity_presentation.block_entity_indices as u64;
        self.stats.other_dynamic_indices = scene.dynamic.indices.iter().map(Vec::len).sum::<usize>()
            as u64
            - self.stats.entity_geometry_indices
            - self.stats.block_entity_geometry_indices;
        self.stats.entity_extracted = scene.entity_presentation.submitted as u64;
        self.stats.block_entities_extracted = scene.entity_presentation.block_entities as u64;
        let yaw = scene.yaw.to_radians();
        let pitch = scene.pitch.to_radians();
        let forward = Vec3::new(
            -yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        );
        let fov = if scene.fov_deg > 0. {
            scene.fov_deg.clamp(30., 110.)
        } else {
            70.
        };
        let projection = Mat4::perspective_rh(
            fov.to_radians(),
            self.config.width as f32 / self.config.height as f32,
            0.05,
            2048.,
        );
        let view_projection = projection
            * hurt_camera_transform(scene)
            * Mat4::look_to_rh(Vec3::ZERO, forward, Vec3::Y);
        self.prepare_visibility(scene, view_projection)?;
        let env = &scene.environment;
        let fancy = self.fancy_shaders && env.sky_kind == SkyKind::Overworld;
        let (sun_sine, sun_cosine) = if fancy {
            env.sun_angle.to_radians().sin_cos()
        } else {
            (0., 1.)
        };
        let (shadow_projection, sunlight, light_strength) = if fancy {
            shadow::projection(env.sun_angle, env.moon_angle)
        } else {
            (Mat4::IDENTITY, Vec3::Y, 0.)
        };
        if fancy {
            self.shadow.prepare(
                &self.queue,
                &self.chunks,
                scene,
                self.fixture.as_ref(),
                shadow_projection,
                sunlight,
            );
        }
        let uv = |texture: Option<crate::api::SkyTexture>| texture.map_or([0.; 4], |t| t.uv);
        let layer =
            |texture: Option<crate::api::SkyTexture>| texture.map_or(-1., |t| t.layer as f32);
        let direction = |index: usize| {
            env.light_directions
                .map_or([0.; 4], |d| [d[index][0], d[index][1], d[index][2], 0.])
        };
        let portal_uv = |index: usize| scene.portal_sprites.map_or([0.; 4], |p| p[index].uv);
        let portal_layer =
            |index: usize| scene.portal_sprites.map_or(-1., |p| p[index].layer as f32);
        let uniform = CameraUniform {
            view_projection: view_projection.to_cols_array(),
            inverse_view_projection: view_projection.inverse().to_cols_array(),
            fog: [
                env.fog_color[0],
                env.fog_color[1],
                env.fog_color[2],
                env.fog_start,
            ],
            sky: [
                env.sky_color[0],
                env.sky_color[1],
                env.sky_color[2],
                env.fog_end,
            ],
            viewport: [
                self.config.width as f32,
                self.config.height as f32,
                env.sky_fog_end,
                match env.sky_kind {
                    SkyKind::None => 0.,
                    SkyKind::Overworld => 1.,
                    SkyKind::End => 2.,
                },
            ],
            celestial: [
                env.sun_angle,
                env.moon_angle,
                1. - env.rain.clamp(0., 1.),
                env.star_brightness.clamp(0., 1.),
            ],
            sun_uv: uv(env.sun_texture),
            moon_uv: uv(env.moon_texture),
            end_uv: uv(env.end_sky_texture),
            layers: [
                layer(env.sun_texture),
                layer(env.moon_texture),
                layer(env.end_sky_texture),
                f32::from(u8::from(env.dark_sky_disc)),
            ],
            sunrise: env.sunrise_color,
            star: [
                env.star_angle,
                env.render_fog_start,
                env.render_fog_end,
                env.fog_alpha,
            ],
            end_flash: [
                env.end_flash_intensity,
                env.end_flash_x_angle,
                env.end_flash_y_angle,
                layer(env.end_flash_texture),
            ],
            end_flash_uv: uv(env.end_flash_texture),
            color_space: [
                f32::from(u8::from(env.display_encoded)),
                (-sun_cosine * 4.).clamp(0., 1.),
                -sun_sine,
                sun_cosine,
            ],
            light_direction_0: direction(0),
            light_direction_1: direction(1),
            portal_uv_0: portal_uv(0),
            portal_uv_1: portal_uv(1),
            material_globals: [
                portal_layer(0),
                portal_layer(1),
                scene.shader_game_time.unwrap_or(0.),
                0.,
            ],
            // Source beacon_beam.fsh reconstructs eye depth using the OpenGL
            // projection's row 3 coefficients, not WebGPU's zero-to-one matrix.
            projection_fog: [
                -2. * 2048. * 0.05 / (2048. - 0.05),
                -(2048. + 0.05) / (2048. - 0.05),
                0.,
                0.,
            ],
            shadow_projection: shadow_projection.to_cols_array(),
            fancy: [
                f32::from(u8::from(fancy)),
                self.monotonic_origin.elapsed().as_secs_f32(),
                ((scene.camera_position[0] * 0.57 + scene.camera_position[2] * 0.39)
                    .rem_euclid(std::f64::consts::TAU)) as f32,
                ((scene.camera_position[0] * 0.57 + scene.camera_position[2] * 0.39) * 1.7)
                    .rem_euclid(std::f64::consts::TAU) as f32,
            ],
            sun_direction: [
                sunlight.x,
                sunlight.y,
                sunlight.z,
                light_strength * (1. - env.rain * 0.8),
            ],
            // Reduce phases in f64, not positions in f32: no long-world wind jitter.
            world_origin: [
                (scene.camera_position[0] * 1.4 + scene.camera_position[2] * 0.6)
                    .rem_euclid(std::f64::consts::TAU) as f32,
                (scene.camera_position[2] * 1.7 - scene.camera_position[0] * 0.5)
                    .rem_euclid(std::f64::consts::TAU) as f32,
                0.,
                0.,
            ],
        };
        self.queue
            .write_buffer(&self.camera, 0, bytemuck::bytes_of(&uniform));
        let drawing = Instant::now();
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Solaris complete frame"),
            });
        self.timing.begin();
        let hiz = !self.reference_mode && self.mdi.as_ref().is_some_and(mdi::Indirect::hiz);
        if !self.reference_mode
            && let Some(mdi) = &mut self.mdi
        {
            mdi.begin_frame();
            mdi.upload(&self.queue, view_projection);
            // Hi-Z: phase 1 keeps last frame's visible sections inside the
            // frustum; the rest is decided after the pyramid exists (phase 2).
            if hiz {
                mdi.encode_phase1(&mut encoder, self.timing.compute_writes(16));
            } else {
                mdi.encode_frustum(&mut encoder, self.timing.compute_writes(16));
            }
        }
        if fancy {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Fancy sun moon directional shadows"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow.view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timing.writes(12),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.shadow.terrain_pipeline);
            pass.set_bind_group(0, &self.camera_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_vertex_buffer(1, self.shadow.instances.slice(..));
            // Both terrain materials share the alpha-tested shadow pipeline.
            // Preserve caster/instance order in one native indirect batch.
            let shadow_mdi = !self.reference_mode && self.mdi.is_some() && self.shadow.batch.is_some();
            if shadow_mdi {
                let batch = self.shadow.batch.as_ref().expect("shadow batch armed");
                let issued = u64::from(!batch.draws.is_empty());
                if issued != 0 {
                    pass.set_vertex_buffer(0, self.vertex_arena.whole());
                    pass.set_index_buffer(self.index_arena.whole(), wgpu::IndexFormat::Uint32);
                    pass.multi_draw_indexed_indirect(&batch.buffer, 0, batch.draws.len() as u32);
                }
                self.stats.draw_calls += issued;
                self.stats.shadow_terrain_api_calls = issued;
                self.stats.shadow_terrain_logical_draws = batch.logical_draws;
                self.stats.shadow_terrain_indices = batch.submitted_indices;
            } else {
                for run in &self.shadow.runs {
                    let mesh = match run.key {
                        Some(key) => &self.chunks[&key].mesh,
                        None => self.fixture.as_ref().expect("fixture shadow run"),
                    };
                    let indices = u64::from(run.range.end - run.range.start);
                    self.draw_resident(&mut pass, mesh, run.range.clone(), run.instance);
                    self.stats.draw_calls += 1;
                    self.stats.shadow_terrain_api_calls += 1;
                    self.stats.shadow_terrain_logical_draws[run.material] += 1;
                    self.stats.shadow_terrain_indices += indices;
                }
            }
            pass.set_pipeline(&self.shadow.pipeline);
            pass.set_vertex_buffer(0, self.dynamic.vertices.slice(..));
            pass.set_index_buffer(self.dynamic.indices.slice(..), wgpu::IndexFormat::Uint32);
            for (family, range) in self.dynamic.ranges.iter().enumerate() {
                let descriptor = material::MATERIALS[family];
                if !range.is_empty() && descriptor.write_depth && descriptor.shader.kind <= 2 {
                    pass.draw_indexed(range.clone(), 0, 1..2);
                    self.stats.draw_calls += 1;
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("actual dimension sky"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post.scene_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: self.timing.writes(0),
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.camera_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_bind_group(2, &self.shadow.bind, &[]);
            pass.set_pipeline(&self.pipelines.sky);
            pass.draw(0..3, 0..1);
            self.stats.draw_calls += 1;
            if env.sky_kind == SkyKind::Overworld {
                if !self.sunrise.ranges[0].is_empty() {
                    pass.set_pipeline(&self.pipelines.sunrise);
                    pass.set_vertex_buffer(0, self.sunrise.vertices.slice(..));
                    pass.set_vertex_buffer(1, self.instances.slice(..));
                    pass.set_index_buffer(
                        self.sunrise.indices.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(self.sunrise.ranges[0].clone(), 0, 0..1);
                    self.stats.draw_calls += 1;
                }
                pass.set_pipeline(&self.pipelines.celestials);
                pass.draw(0..3, 0..1);
                self.stats.draw_calls += 1;
                if env.star_brightness > 0. {
                    pass.set_pipeline(&self.pipelines.stars);
                    pass.set_vertex_buffer(0, self.stars.slice(..));
                    pass.draw(0..self.star_vertices, 0..1);
                    self.stats.draw_calls += 1;
                }
                if env.dark_sky_disc {
                    pass.set_pipeline(&self.pipelines.dark_disc);
                    pass.draw(0..3, 0..1);
                    self.stats.draw_calls += 1;
                }
            }
            if env.sky_kind == SkyKind::End && env.end_flash_intensity > 0. {
                pass.set_pipeline(&self.pipelines.celestials);
                pass.draw(0..3, 0..1);
                self.stats.draw_calls += 1;
            }
        }
        // Hi-Z splits the cutout pass so phase-2 terrain lands before any
        // blended entity geometry, which must still draw over all terrain.
        // (material, terrain, dynamic geometry)
        let stages: &[(usize, bool, bool)] = if hiz {
            &[(0, true, false), (1, true, false), (1, false, true), (2, true, true)]
        } else {
            &[(0, true, true), (1, true, true), (2, true, true)]
        };
        for (stage, &(material, terrain, dynamic)) in stages.iter().enumerate() {
            let slot = if terrain { material + 1 } else { 20 };
            if material == 2 && fancy {
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.targets._depth,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::DepthOnly,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &self.shadow.opaque_depth,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::DepthOnly,
                    },
                    wgpu::Extent3d {
                        width: self.config.width,
                        height: self.config.height,
                        depth_or_array_layers: 1,
                    },
                );
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(timing::PASSES[slot]),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.post.scene_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.targets.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: if stage == 0 {
                            wgpu::LoadOp::Clear(1.)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: self.timing.writes(slot),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipelines.world[material]);
            pass.set_bind_group(0, &self.camera_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_bind_group(2, &self.shadow.bind, &[]);
            pass.set_vertex_buffer(1, self.instances.slice(..));
            let mut calls = 0;
            let mut indices = 0;
            let mut terrain_api = 0;
            let mut terrain_logical = 0;
            let mut terrain_indices = 0;
            let mut entity_calls = 0;
            if material != 2 {
                if terrain {
                    if let Some(mdi) = self.mdi.as_ref().filter(|_| !self.reference_mode) {
                        // One API submission per terrain material. GPU writes zero
                        // instance_count for coarse candidates outside the frustum
                        // (and, under Hi-Z, for those not decided by this phase).
                        if !mdi.candidates[material].is_empty() {
                            pass.set_vertex_buffer(0, self.vertex_arena.whole());
                            pass.set_index_buffer(
                                self.index_arena.whole(),
                                wgpu::IndexFormat::Uint32,
                            );
                            terrain_api += u64::from(mdi.draw(&mut pass, material));
                            calls += terrain_api;
                        }
                        for run in &self.terrain_runs[material] {
                            let count = u64::from(run.range.end - run.range.start);
                            indices += count;
                            terrain_indices += count;
                            terrain_logical += 1;
                        }
                    } else {
                        for run in &self.terrain_runs[material] {
                            let mesh = &self.chunks[&run.key].mesh;
                            let count = u64::from(run.range.end - run.range.start);
                            indices += count;
                            terrain_indices += count;
                            terrain_api += 1;
                            terrain_logical += 1;
                            calls += 1;
                            self.draw_resident(&mut pass, mesh, run.range.clone(), run.instance);
                        }
                    }
                    if let Some(mesh) = &self.fixture {
                        let range = mesh.ranges[material].clone();
                        if !range.is_empty() {
                            indices += u64::from(range.end - range.start);
                            terrain_indices += u64::from(range.end - range.start);
                            terrain_api += 1;
                            terrain_logical += 1;
                            calls += 1;
                            self.draw_resident(&mut pass, mesh, range, 1);
                        }
                    }
                }
                if material == 1 && dynamic {
                    // Source RenderType ordering is independent of opaque/cutout
                    // terrain bins. Distinct shader/blend/depth families stay intact.
                    pass.set_vertex_buffer(0, self.dynamic.vertices.slice(..));
                    pass.set_index_buffer(
                        self.dynamic.indices.slice(..),
                        wgpu::IndexFormat::Uint32,
                    );
                    for &family in &self.pipelines.dynamic_order {
                        let range = self.dynamic.ranges[family].clone();
                        if range.is_empty() || material::MATERIALS[family].shader.kind == 11 {
                            continue;
                        }
                        pass.set_pipeline(
                            &self.pipelines.dynamic[self.pipelines.dynamic_index[family]],
                        );
                        if !material::MATERIALS[family].sort {
                            indices += u64::from(range.end - range.start);
                            self.stats.entity_submitted_indices +=
                                u64::from(range.end - range.start);
                            calls += 1;
                            entity_calls += 1;
                            pass.draw_indexed(range, 0, 1..2);
                        } else {
                            let sorted = self.transparent_ranges[family].clone();
                            let mut i = sorted.start;
                            while i < sorted.end {
                                let first = self.transparent[i];
                                let mut count = first.count;
                                i += 1;
                                while i < sorted.end
                                    && self.transparent[i].first == first.first + count
                                {
                                    count += self.transparent[i].count;
                                    i += 1;
                                }
                                pass.draw_indexed(first.first..first.first + count, 0, 1..2);
                                indices += u64::from(count);
                                self.stats.entity_submitted_indices += u64::from(count);
                                calls += 1;
                                entity_calls += 1;
                            }
                        }
                    }
                }
            } else {
                let sorted = self.transparent_ranges[EntityMaterial::COUNT].clone();
                for draw in &self.transparent[sorted] {
                    let Source::Chunk(key) = draw.source else {
                        unreachable!()
                    };
                    self.draw_resident(
                        &mut pass,
                        &self.chunks[&key].mesh,
                        draw.first..draw.first + draw.count,
                        draw.instance,
                    );
                    indices += u64::from(draw.count);
                    calls += 1;
                }
                self.stats.terrain_translucent_draws += calls;
                self.stats.terrain_translucent_indices += indices;
            }
            if material == 0 {
                self.stats.terrain_opaque_api_calls += terrain_api;
                self.stats.terrain_opaque_logical_draws += terrain_logical;
                self.stats.terrain_opaque_indices += terrain_indices;
            } else if material == 1 {
                self.stats.terrain_cutout_api_calls += terrain_api;
                self.stats.terrain_cutout_logical_draws += terrain_logical;
                self.stats.terrain_cutout_indices += terrain_indices;
                self.stats.entity_draws += entity_calls;
            }
            drop(pass);
            self.stats.draw_calls += calls;
            self.stats.submitted_indices += indices;
            if hiz && material == 1 && terrain {
                // Phase 2: pyramid from the phase-1 depth, then draw the
                // sections it proves visible that phase 1 did not draw.
                let mdi = self.mdi.as_ref().expect("hi-z requires indirect culling");
                mdi.encode_pyramid(&mut encoder, self.timing.compute_writes(17));
                mdi.encode_phase2(&mut encoder, self.timing.compute_writes(18));
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(timing::PASSES[19]),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.post.scene_view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.targets.depth_view,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: self.timing.writes(19),
                    occlusion_query_set: None,
                });
                pass.set_bind_group(0, &self.camera_bind, &[]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_bind_group(2, &self.shadow.bind, &[]);
                pass.set_vertex_buffer(1, self.instances.slice(..));
                if mdi.candidates.iter().any(|candidates| !candidates.is_empty()) {
                    // Arenas exist only once something is resident.
                    pass.set_vertex_buffer(0, self.vertex_arena.whole());
                    pass.set_index_buffer(self.index_arena.whole(), wgpu::IndexFormat::Uint32);
                }
                let mut phase2_calls = 0;
                for material in 0..2 {
                    if !mdi.candidates[material].is_empty() {
                        pass.set_pipeline(&self.pipelines.world[material]);
                        phase2_calls += u64::from(mdi.draw(&mut pass, material));
                    }
                }
                drop(pass);
                self.stats.draw_calls += phase2_calls;
                self.stats.terrain_phase2_api_calls += phase2_calls;
            }
        }
        self.post
            .encode(&mut encoder, &self.targets.color_view, fancy, &self.timing);
        self.stats.draw_calls += if fancy { 3 } else { 1 };
        if self
            .dynamic
            .ranges
            .iter()
            .enumerate()
            .any(|(family, range)| {
                !range.is_empty() && material::MATERIALS[family].shader.kind == 11
            })
        {
            let mut pass = self.outline.mask_pass(&mut encoder, &self.timing);
            pass.set_bind_group(0, &self.camera_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_bind_group(2, &self.shadow.bind, &[]);
            pass.set_vertex_buffer(0, self.dynamic.vertices.slice(..));
            pass.set_vertex_buffer(1, self.instances.slice(..));
            pass.set_index_buffer(self.dynamic.indices.slice(..), wgpu::IndexFormat::Uint32);
            for &family in &self.pipelines.dynamic_order {
                let range = self.dynamic.ranges[family].clone();
                if !range.is_empty() && material::MATERIALS[family].shader.kind == 11 {
                    pass.set_pipeline(
                        &self.pipelines.dynamic[self.pipelines.dynamic_index[family]],
                    );
                    self.stats.submitted_indices += u64::from(range.end - range.start);
                    self.stats.entity_submitted_indices += u64::from(range.end - range.start);
                    pass.draw_indexed(range, 0, 1..2);
                    self.stats.draw_calls += 1;
                    self.stats.entity_draws += 1;
                }
            }
            drop(pass);
            self.outline
                .post(&mut encoder, &self.targets.color_view, &self.timing);
            self.stats.draw_calls += 5;
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("actual retained UI overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.color_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: self.timing.writes(10),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipelines.overlay);
            pass.set_bind_group(0, &self.camera_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_bind_group(2, &self.shadow.bind, &[]);
            pass.set_vertex_buffer(0, self.overlay.vertices.slice(..));
            pass.set_index_buffer(self.overlay.indices.slice(..), wgpu::IndexFormat::Uint32);
            for range in &self.overlay.ranges {
                if !range.is_empty() {
                    pass.draw_indexed(range.clone(), 0, 0..1);
                    self.stats.draw_calls += 1;
                    self.stats.submitted_indices += u64::from(range.end - range.start);
                }
            }
        }
        if let Some(frame) = frame {
            let surface_view = frame.texture.create_view(&Default::default());
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present same world UI target"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: self.timing.writes(11),
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.pipelines.blit);
            pass.set_bind_group(0, &self.targets.blit, &[]);
            pass.draw(0..3, 0..1);
            self.stats.draw_calls += 1;
        }
        self.timing.resolve(&mut encoder);
        self.stats.draw_encode_ms = drawing.elapsed().as_secs_f64() * 1000.;
        Ok(encoder)
    }
}

/// Vanilla post-multiplies the projection by bobHurt before the ordinary view.
/// Keep the three rotations in camera space; a world-space roll changes the attack axis.
fn hurt_camera_transform(scene: &Scene) -> Mat4 {
    let roll = if scene.roll_deg == 0. {
        Mat4::IDENTITY
    } else {
        Mat4::from_rotation_z(scene.roll_deg.to_radians())
    };
    if scene.hurt_tilt_deg == 0. {
        return roll;
    }
    let direction = scene.hurt_direction.to_radians();
    roll * Mat4::from_rotation_y(-direction)
        * Mat4::from_rotation_z(scene.hurt_tilt_deg.to_radians())
        * Mat4::from_rotation_y(direction)
}

#[cfg(test)]
mod hurt_camera_tests {
    use super::*;

    fn close(actual: Vec3, expected: Vec3) {
        assert!(
            (actual - expected).length() < 0.000001,
            "{actual:?} != {expected:?}"
        );
    }

    #[test]
    fn p1_hurt_camera_zero_tilt_is_identity_for_every_direction() {
        for direction in [0., 45., 90., 180., -270.] {
            let mut scene = Scene::default();
            scene.hurt_direction = direction;
            assert_eq!(hurt_camera_transform(&scene), Mat4::IDENTITY);
        }
    }

    #[test]
    fn p1_hurt_camera_zero_direction_rolls_in_vanilla_negative_z() {
        let mut scene = Scene::default();
        scene.hurt_tilt_deg = -14.;
        let matrix = hurt_camera_transform(&scene);
        let angle = (-14_f32).to_radians();
        close(
            matrix.transform_vector3(Vec3::X),
            Vec3::new(angle.cos(), angle.sin(), 0.),
        );
        close(matrix.transform_vector3(Vec3::Z), Vec3::Z);
    }

    #[test]
    fn p1_hurt_camera_conjugates_roll_by_received_direction_before_view() {
        let mut scene = Scene::default();
        scene.hurt_tilt_deg = -14.;
        scene.hurt_direction = 90.;
        let matrix = hurt_camera_transform(&scene);
        let angle = (-14_f32).to_radians();
        // At +90 hurt yaw the roll axis is -X; it is not a yaw change.
        close(matrix.transform_vector3(Vec3::X), Vec3::X);
        close(
            matrix.transform_vector3(Vec3::Y),
            Vec3::new(0., angle.cos(), -angle.sin()),
        );
        scene.hurt_direction = 180.;
        let opposite = hurt_camera_transform(&scene);
        close(
            opposite.transform_vector3(Vec3::X),
            Vec3::new(angle.cos(), -angle.sin(), 0.),
        );
        close(
            matrix
                .inverse()
                .transform_vector3(matrix.transform_vector3(Vec3::Y)),
            Vec3::Y,
        );
    }
}

#[cfg(test)]
mod terrain_translucency_tests {
    use super::*;

    #[test]
    fn terrain_translucency_has_one_descriptor_per_section_not_per_quad() {
        let mut renderer = pollster::block_on(Renderer::new_offscreen(
            32,
            32,
            PathBuf::from(".analysis/drawcall-regression"),
        ))
        .unwrap();
        renderer.reference_mode = true;
        let mut vertices = super::packed::blank_vertices(16);
        let mut indices = Vec::new();
        for quad in 0..4 {
            let base = quad * 4;
            for corner in 0..4 {
                vertices[base + corner].position = [quad as f32, corner as f32, 0.];
            }
            let base = base as u32;
            indices.extend([base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let packed = super::packed::pack_vertices(&vertices, &mut Default::default()).unwrap();
        for (key, origin) in [([0, 0, 0], [0.; 3]), ([2, 0, 0], [32., 0., 0.])] {
            let mesh = ResidentMesh::reserve(
                &renderer.device,
                &renderer.queue,
                &mut renderer.vertex_arena,
                &mut renderer.index_arena,
                &vertices,
                [&indices[..6], &indices[..6], &indices],
                [None, None],
            )
            .unwrap();
            mesh.upload_geometry(
                &renderer.queue,
                &renderer.vertex_arena,
                &renderer.index_arena,
                &packed,
                [&indices[..6], &indices[..6], &indices],
            );
            renderer.chunks.insert(
                key,
                ResidentChunk {
                    revision: 1,
                    origin,
                    bounds_min: [0.; 3],
                    bounds_max: [16.; 3],
                    // All 36 face pairs open; matches both visibility mask domains.
                    visibility_mask: (1u64 << 36) - 1,
                    fade_duration_ms: Some(0),
                    uploaded_time_ms: Some(0),
                    visibility: None,
                    mesh,
                },
            );
        }
        renderer.dynamic.transparent[0] = mesh::primitives(&vertices, &indices, 0).unwrap();
        let allocations: Vec<_> = renderer
            .chunks
            .values()
            .map(|chunk| chunk.mesh.indices)
            .collect();
        let scene = Scene::default();
        renderer.prepare_visibility(&scene, Mat4::IDENTITY).unwrap();
        let draws =
            &renderer.transparent[renderer.transparent_ranges[EntityMaterial::COUNT].clone()];
        assert_eq!(renderer.stats.visible_chunks, 2);
        assert_eq!(
            draws.len(),
            2,
            "terrain submission must scale with sections"
        );
        assert!(draws[0].source == Source::Chunk([2, 0, 0]));
        assert!(draws[1].source == Source::Chunk([0, 0, 0]));
        assert!(draws
            .iter()
            .all(|draw| draw.first == 12 && draw.count == 24));
        assert_eq!(draws.iter().map(|draw| draw.count).sum::<u32>(), 48);
        let dynamic = &renderer.transparent[renderer.transparent_ranges[0].clone()];
        assert_eq!(
            dynamic.len(),
            4,
            "dynamic primitives keep their per-quad path"
        );
        assert_eq!(dynamic[0].first, 18);
        assert_eq!(dynamic[3].first, 0);
        assert!(dynamic.iter().all(|draw| draw.count == 6));
        assert_eq!(renderer.stats.terrain_upload_bytes, 192);
        renderer.prepare_visibility(&scene, Mat4::IDENTITY).unwrap();
        assert_eq!(
            renderer.stats.terrain_upload_bytes, 192,
            "stationary camera must not rewrite"
        );
        let mut moved = scene;
        moved.camera_position = [17., 0., 0.];
        renderer.prepare_visibility(&moved, Mat4::IDENTITY).unwrap();
        assert_eq!(renderer.stats.terrain_upload_bytes, 384);
        assert_eq!(
            renderer
                .chunks
                .values()
                .map(|chunk| chunk.mesh.indices)
                .collect::<Vec<_>>(),
            allocations,
            "resorting must retain the arena allocations"
        );
        for chunk in renderer.chunks.values() {
            assert_eq!(chunk.mesh.ranges, [0..6, 6..12, 12..36]);
        }
    }
    #[test]
    fn frustum_rejection_precedes_instance_and_indirect_submission() {
        let mut renderer = pollster::block_on(Renderer::new_offscreen(
            32,
            32,
            PathBuf::from(".analysis/frustum-rejection"),
        ))
        .unwrap();
        // Optimized mode (reference_mode off) so the real frustum gate runs.
        let mut vertices = super::packed::blank_vertices(8);
        for (index, vertex) in vertices.iter_mut().enumerate() {
            vertex.position = [index as f32, 0., 4.];
        }
        let indices = vec![0, 1, 2, 0, 2, 3];
        // Two residents far apart: the sparse-extents volume guard trips, so
        // the occlusion flood is unavailable and every section falls through
        // to the plain frustum test. The origin section overlaps the identity
        // NDC cube; the distant one does not.
        let packed = super::packed::pack_vertices(&vertices, &mut Default::default()).unwrap();
        for (key, origin) in [([0, 0, 0], [0.; 3]), ([60, 60, 60], [960., 960., 960.])] {
            let mesh = ResidentMesh::reserve(
                &renderer.device,
                &renderer.queue,
                &mut renderer.vertex_arena,
                &mut renderer.index_arena,
                &vertices,
                [&indices, &[], &[]],
                [None, None],
            )
            .unwrap();
            mesh.upload_geometry(
                &renderer.queue,
                &renderer.vertex_arena,
                &renderer.index_arena,
                &packed,
                [&indices, &[], &[]],
            );
            renderer.chunks.insert(
                key,
                ResidentChunk {
                    revision: 1,
                    origin,
                    bounds_min: [0.; 3],
                    bounds_max: [16.; 3],
                    visibility_mask: (1u64 << 36) - 1,
                    fade_duration_ms: Some(0),
                    uploaded_time_ms: Some(0),
                    visibility: None,
                    mesh,
                },
            );
        }
        let scene = Scene::default();
        renderer.prepare_visibility(&scene, Mat4::IDENTITY).unwrap();
        assert_eq!(renderer.stats.visible_chunks, 1);
        assert_eq!(renderer.stats.culled_chunks, 1);
        assert_eq!(renderer.stats.frustum_rejected_chunks, 1);
        assert_eq!(renderer.stats.graph_rejected_chunks, 0);
        assert!(!renderer.stats.graph_available);
        assert_eq!(renderer.stats.graph_reached_sections, 0);
        // Candidate counts need MDI hardware; the fallback path submits zero.
        let expected_candidates = u64::from(renderer.mdi.is_some());
        assert_eq!(renderer.stats.terrain_candidates_opaque, expected_candidates);
        assert_eq!(renderer.stats.terrain_candidates_cutout, 0);
        // Two base instance slots (null + presentation origin) plus exactly
        // the one visible section: the culled section takes no slot.
        assert_eq!(renderer.instance_data.len(), 3);
        // Same for indirect candidates: only the visible section submits one.
        if let Some(mdi) = &renderer.mdi {
            assert_eq!(
                mdi.candidates.iter().map(Vec::len).sum::<usize>(),
                1,
                "off-frustum sections must not emit indirect candidates"
            );
        }
        assert!(renderer.transparent.is_empty());
        // Stale-visibility hypothesis: swinging to an empty view and back
        // must reproduce the same pose exactly, with no duplicated keys.
        let snapshot = (
            renderer.stats.visible_chunks,
            renderer.stats.culled_chunks,
            renderer.stats.frustum_rejected_chunks,
            renderer.stats.terrain_candidates_opaque,
            renderer.stats.terrain_candidates_cutout,
            renderer.instance_data.len(),
            renderer.mdi.as_ref().map(|mdi| {
                mdi.candidates.iter().map(Vec::len).sum::<usize>()
            }),
            renderer.visible.iter().map(|visible| visible.key).collect::<Vec<_>>(),
        );
        let empty_view = Mat4::from_translation(Vec3::new(10_000., 0., 0.));
        renderer.prepare_visibility(&scene, empty_view).unwrap();
        assert_eq!(renderer.stats.visible_chunks, 0);
        assert_eq!(renderer.visible.len(), 0);
        renderer.prepare_visibility(&scene, Mat4::IDENTITY).unwrap();
        let restored = (
            renderer.stats.visible_chunks,
            renderer.stats.culled_chunks,
            renderer.stats.frustum_rejected_chunks,
            renderer.stats.terrain_candidates_opaque,
            renderer.stats.terrain_candidates_cutout,
            renderer.instance_data.len(),
            renderer.mdi.as_ref().map(|mdi| {
                mdi.candidates.iter().map(Vec::len).sum::<usize>()
            }),
            renderer.visible.iter().map(|visible| visible.key).collect::<Vec<_>>(),
        );
        assert_eq!(restored, snapshot, "A-B-A view must restore identical submission");
        let mut keys = renderer.visible.iter().map(|visible| visible.key).collect::<Vec<_>>();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), renderer.visible.len(), "visible keys must not duplicate");
    }

    #[test]
    fn sections_wholly_past_the_fog_end_are_not_submitted() {
        let mut renderer = pollster::block_on(Renderer::new_offscreen(
            32,
            32,
            PathBuf::from(".analysis/fog-cull-regression"),
        ))
        .unwrap();
        let mut vertices = super::packed::blank_vertices(4);
        for (corner, vertex) in vertices.iter_mut().enumerate() {
            vertex.position = [(corner / 2) as f32, 0., (corner % 2) as f32];
        }
        let indices = [0u32, 1, 2, 0, 2, 3];
        // Section keys 0 and 18 start 0 and 288 blocks east of the camera.
        let packed = super::packed::pack_vertices(&vertices, &mut Default::default()).unwrap();
        for (key, origin) in [([0, 0, 0], [0.; 3]), ([18, 0, 0], [288., 0., 0.])] {
            let mesh = ResidentMesh::reserve(
                &renderer.device,
                &renderer.queue,
                &mut renderer.vertex_arena,
                &mut renderer.index_arena,
                &vertices,
                [&indices, &[], &[]],
                [None, None],
            )
            .unwrap();
            mesh.upload_geometry(
                &renderer.queue,
                &renderer.vertex_arena,
                &renderer.index_arena,
                &packed,
                [&indices, &[], &[]],
            );
            renderer.chunks.insert(
                key,
                ResidentChunk {
                    revision: 1,
                    origin,
                    bounds_min: [0.; 3],
                    bounds_max: [16.; 3],
                    visibility_mask: visibility::ALL_OPEN,
                    fade_duration_ms: Some(0),
                    uploaded_time_ms: Some(0),
                    visibility: None,
                    mesh,
                },
            );
        }
        let mut scene = Scene::default();
        scene.environment.sky_kind = SkyKind::Overworld;
        scene.environment.fog_start = 100.;
        scene.environment.fog_end = 4000.;
        scene.environment.render_fog_start = 240.;
        scene.environment.render_fog_end = 256.;
        let view = Mat4::from_scale(Vec3::splat(0.0005));
        renderer.set_fancy_shaders(true);
        renderer.prepare_visibility(&scene, view).unwrap();
        assert_eq!(renderer.stats.visible_chunks, 1);
        assert_eq!(renderer.stats.fog_rejected_chunks, 1);
        // Reference rendering and non-Fancy shading keep every section.
        renderer.set_fancy_shaders(false);
        renderer.prepare_visibility(&scene, view).unwrap();
        assert_eq!(renderer.stats.visible_chunks, 2);
        assert_eq!(renderer.stats.fog_rejected_chunks, 0);
        // Fog that has not reached full strength (alpha < 1) never culls.
        renderer.set_fancy_shaders(true);
        scene.environment.fog_alpha = 0.5;
        renderer.prepare_visibility(&scene, view).unwrap();
        assert_eq!(renderer.stats.visible_chunks, 2);
    }
}
