use super::*;

impl Renderer {
    /// No JSON/snapshot allocation on the frame path. A completed frame must
    /// contain the current nearby uploads after their ordinary terrain fade.
    pub fn spawn_meshes_presented(
        &mut self,
        scene: &Scene,
        epochs: (u64, u64),
        cpu_ready: bool,
    ) -> bool {
        let center = scene
            .camera_position
            .map(|coordinate| (coordinate / 16.).floor() as i32);
        let ready = cpu_ready
            && self.surface.is_some()
            && !self.gpu_failed.load(Ordering::Acquire)
            && self.frame_id > 0
            && (-1..=1).all(|dx| {
                (-1..=1).all(|dy| {
                    (-1..=1).all(|dz| {
                        let Some(x) = center[0].checked_add(dx) else {
                            return true;
                        };
                        let Some(y) = center[1].checked_add(dy) else {
                            return true;
                        };
                        let Some(z) = center[2].checked_add(dz) else {
                            return true;
                        };
                        let key = [x, y, z];
                        scene.chunks.get(&key).is_none_or(|draw| {
                            self.chunks.get(&key).is_some_and(|resident| {
                                resident.revision == draw.revision
                                    && resident.uploaded_time_ms.is_some()
                                    && resident.visibility == Some(1.)
                            })
                        })
                    })
                })
            });
        if !ready {
            self.spawn_ready_frame = None;
            return false;
        }
        let first_frame = match self.spawn_ready_frame {
            Some((connection, dimension, frame)) if (connection, dimension) == epochs => frame,
            _ => {
                self.spawn_ready_frame = Some((epochs.0, epochs.1, self.frame_id));
                self.frame_id
            }
        };
        self.completed_frame.load(Ordering::Acquire) >= first_frame
    }
    pub fn metrics(&self) -> Value {
        let mut target = json!({"adapter":self.capabilities,"display_scanout_verified":false,
            "passes":timing::PASSES,"mesh_cpu_ms":null,"mesh_cpu_status":"owned_by_session_mesher",
            "memory_measurement":"tracked_wgpu_allocations_not_physical_driver_vram","resident":{}});
        self.update_metrics(&mut target);
        target["section_upload_receipts"] = self.section_upload_receipts();
        target
    }
    /// Explicit snapshot only: normal per-frame metrics do not allocate section JSON.
    pub fn section_upload_receipts(&self) -> Value {
        let mut selected = self.visible.iter().peekable();
        let sections: Vec<Value> = self
            .chunks
            .iter()
            .take(64)
            .map(|(key, chunk)| {
                let selected_for_draw = selected.peek().is_some_and(|visible| visible.key == *key);
                if selected_for_draw {
                    selected.next();
                }
                json!({
                    "section":key,
                    "mesh_revision":chunk.revision.to_string(),
                    "uploaded_time_ms":chunk.uploaded_time_ms.map(|time| time.to_string()),
                    "fade_duration_ms":chunk.fade_duration_ms,
                    "source_visibility":chunk.visibility,
                    "selected_for_draw":selected_for_draw,
                })
            })
            .collect();
        json!({
            "clock_domain":"renderer_monotonic_elapsed_millis",
            "upload_cause":"all_present_layer_uploads_and_resident_mapping_committed",
            "visibility_time_ms":self.visibility_time_ms.to_string(),
            "frame_id":self.frame_id.to_string(),
            "generation":self.generation,
            "bounded_resident_section_limit":mesh::MAX_CHUNKS,
            "section_count":self.chunks.len(),
            "section_sample_limit":64,
            "truncated":self.chunks.len() > 64,
            "sections":sections,
        })
    }
    pub fn update_metrics(&self, target: &mut Value) {
        if !target.is_object() {
            *target = json!({});
        }
        if target.get("adapter").is_none() {
            target["adapter"] = self.capabilities.clone();
        }
        for (key, value) in [
            ("frame_id", self.frame_id),
            ("submitted_frame_id", self.frame_id),
            (
                "gpu_completed_frame_id",
                self.completed_frame.load(Ordering::Acquire),
            ),
            ("scene_revision", self.mesh_revision.unwrap_or(0)),
        ] {
            use std::fmt::Write as _;
            if let Some(Value::String(text)) = target.get_mut(key) {
                text.clear();
                let _ = write!(text, "{value}");
            } else {
                target[key] = json!(value.to_string());
            }
        }
        target["present_requested_frame_id"] = if self.surface.is_some() {
            json!(self.frame_id.to_string())
        } else {
            Value::Null
        };
        target["offscreen_only"] = json!(self.surface.is_none());
        target["render_target"] = json!(if self.surface.is_some() {
            "actual_window_scene_target"
        } else {
            "actual_offscreen_scene_target"
        });
        target["uploaded_texture_revision"] =
            json!(self.texture_revision.map(|revision| revision.to_string()));
        target["gpu_activated_texture_revision"] = json!(self
            .resource_revision()
            .map(|revision| revision.to_string()));
        target["resource_activation_frame_id"] =
            json!(self.texture_activation_frame.map(|frame| frame.to_string()));
        target["gpu_failed"] = json!(self.gpu_failed.load(Ordering::Acquire));
        let mode = if self.reference_mode {
            "reference"
        } else {
            "optimized"
        };
        if target.get("render_mode").and_then(Value::as_str) != Some(mode) {
            target["render_mode"] = json!(mode);
        }
        target["width"] = json!(self.config.width);
        target["height"] = json!(self.config.height);
        target["graphics_preset"] = json!(self.graphics.as_str());
        target["present_mode"] = json!(match self.config.present_mode {
            wgpu::PresentMode::Fifo => "fifo",
            wgpu::PresentMode::AutoNoVsync => "auto_no_vsync",
            wgpu::PresentMode::Immediate => "immediate",
            wgpu::PresentMode::Mailbox => "mailbox",
            wgpu::PresentMode::FifoRelaxed => "fifo_relaxed",
            wgpu::PresentMode::AutoVsync => "auto_vsync",
        });
        target["desired_maximum_frame_latency"] = json!(self.config.desired_maximum_frame_latency);
        target["terrain_multi_draw_indirect"] = json!(self.mdi.is_some() && !self.reference_mode);
        target["terrain_indirect_candidates"] = json!(self.mdi.as_ref().map_or(0, |mdi| mdi
            .candidates
            .iter()
            .map(Vec::len)
            .sum::<usize>()));
        target["terrain_indirect_candidates_opaque"] = json!(self.stats.terrain_candidates_opaque);
        target["terrain_indirect_candidates_cutout"] = json!(self.stats.terrain_candidates_cutout);
        target["terrain_hiz_occlusion"] =
            json!(self.mdi.as_ref().is_some_and(mdi::Indirect::hiz) && !self.reference_mode);
        target["terrain_phase2_api_calls"] = json!(self.stats.terrain_phase2_api_calls);
        // Counters of the latest frame whose GPU work completed (not necessarily
        // the frame this sample describes); captures carry the exact frame.
        if let Some(mdi) = self.mdi.as_ref().filter(|_| !self.reference_mode) {
            target["terrain_gpu_culling_latest"] = mdi.latest.clone();
        }
        target["terrain_opaque_api_calls"] = json!(self.stats.terrain_opaque_api_calls);
        target["terrain_opaque_logical_draws"] = json!(self.stats.terrain_opaque_logical_draws);
        target["terrain_opaque_indices"] = json!(self.stats.terrain_opaque_indices);
        target["terrain_cutout_api_calls"] = json!(self.stats.terrain_cutout_api_calls);
        target["terrain_cutout_logical_draws"] = json!(self.stats.terrain_cutout_logical_draws);
        target["terrain_cutout_indices"] = json!(self.stats.terrain_cutout_indices);
        target["terrain_translucent_draws"] = json!(self.stats.terrain_translucent_draws);
        target["terrain_translucent_indices"] = json!(self.stats.terrain_translucent_indices);
        target["shadow_terrain_api_calls"] = json!(self.stats.shadow_terrain_api_calls);
        target["shadow_terrain_logical_draws"] = json!(self.stats.shadow_terrain_logical_draws);
        target["shadow_terrain_indices"] = json!(self.stats.shadow_terrain_indices);
        target["shadow_indirect_reserved_bytes"] = json!(self
            .shadow
            .batch
            .as_ref()
            .map_or(0, shadow::ShadowBatch::reserved_bytes));
        target["entity_draws"] = json!(self.stats.entity_draws);
        target["entity_submitted_indices"] = json!(self.stats.entity_submitted_indices);
        // Entity/block-entity contributions share material draws.
        target["entity_geometry_indices"] = json!(self.stats.entity_geometry_indices);
        target["block_entity_geometry_indices"] = json!(self.stats.block_entity_geometry_indices);
        target["other_dynamic_indices"] = json!(self.stats.other_dynamic_indices);
        target["entity_extracted"] = json!(self.stats.entity_extracted);
        target["block_entities_extracted"] = json!(self.stats.block_entities_extracted);
        target["frustum_rejected_chunks"] = json!(self.stats.frustum_rejected_chunks);
        target["graph_rejected_chunks"] = json!(self.stats.graph_rejected_chunks);
        target["fog_rejected_chunks"] = json!(self.stats.fog_rejected_chunks);
        target["occlusion_graph_enabled"] = json!(self.stats.graph_enabled);
        target["occlusion_graph_available"] = json!(self.stats.graph_available);
        target["occlusion_graph_reached_sections"] = json!(self.stats.graph_reached_sections);
        // `draw_calls` counts wgpu draw invocations: one MDI call serves many
        // logical terrain draws. `index_count` sums every visible chunk range
        // regardless of path, including MDI candidates the GPU may still cull.
        target["index_count"] = json!(self.stats.submitted_indices);
        target["draw_calls"] = json!(self.stats.draw_calls);
        target["frame_wall_ms"] = json!(self.last_frame_ms);
        target["cpu_prepare_submit_ms"] = json!((self.last_frame_ms
            - self.stats.surface_acquire_ms
            - self.stats.present_call_ms)
            .max(0.));
        target["cpu_upload_ms"] = json!(self.stats.upload_ms);
        target["cpu_visibility_sort_ms"] = json!(self.stats.visibility_ms);
        target["cpu_draw_encode_ms"] = json!(self.stats.draw_encode_ms);
        target["surface_acquire_ms"] = json!(self.stats.surface_acquire_ms);
        target["present_call_ms"] = json!(self.stats.present_call_ms);
        target["present_thread_cpu_ms"] = json!(self.stats.present_thread_cpu_ms);
        target["submitted_frame_interval_ms"] = json!(self.frame_interval_ms);
        target["terrain_upload_bytes"] = json!(self.stats.terrain_upload_bytes);
        target["terrain_uploaded_chunks"] = json!(self.stats.terrain_uploaded_chunks);
        target["terrain_freed_chunks"] = json!(self.stats.terrain_freed_chunks);
        target["texture_upload_bytes"] = json!(self.stats.texture_upload_bytes);
        target["dynamic_upload_bytes"] = json!(self.dynamic.uploaded_bytes);
        target["ui_upload_bytes"] = json!(self.overlay.uploaded_bytes);
        target["upload_bytes"] = json!(
            self.stats.terrain_upload_bytes
                + self.stats.texture_upload_bytes
                + self.dynamic.uploaded_bytes
                + self.overlay.uploaded_bytes
                + self.sunrise.uploaded_bytes
                + self.instance_data.len() as u64 * 16
                + std::mem::size_of::<CameraUniform>() as u64
                + if self.reference_mode {
                    0
                } else {
                    self.mdi.as_ref().map_or(0, mdi::Indirect::upload_bytes)
                }
        );
        target["visible_chunks"] = json!(self.stats.visible_chunks);
        target["culled_chunks"] = json!(self.stats.culled_chunks);
        target["resident_chunks"] = json!(self.chunks.len());
        target["unknown_section_fade_policy_count"] =
            json!(self.stats.unknown_fade_policy_sections);
        target["texture_layers"] = json!(self.texture_layers);
        target["timestamp_missed_samples"] = json!(self.timing.missed_samples);
        if target.get("gpu_timestamps").and_then(|v| v.get("frame_id"))
            != self.timing.latest.get("frame_id")
            || target.get("gpu_timestamps").and_then(|v| v.get("status"))
                != self.timing.latest.get("status")
        {
            target["gpu_timestamps"] = self.timing.latest.clone();
            target["gpu_time_ms"] = self.timing.latest["total_ms"].clone();
            target["gpu_time_status"] = self.timing.latest["status"].clone();
        }
        if !target.get("resident").is_some_and(Value::is_object) {
            target["resident"] = json!({});
        }
        let resident = &mut target["resident"];
        resident["vertex_reserved_bytes"] = json!(self.vertex_arena.reserved());
        resident["vertex_used_bytes"] = json!(self.vertex_arena.used());
        resident["index_reserved_bytes"] = json!(self.index_arena.reserved());
        resident["index_used_bytes"] = json!(self.index_arena.used());
        resident["vertex_largest_free_bytes"] = json!(self.vertex_arena.largest_free());
        resident["index_largest_free_bytes"] = json!(self.index_arena.largest_free());
        resident["retired_mesh_bytes"] =
            json!(self.vertex_arena.retired_bytes() + self.index_arena.retired_bytes());
        resident["texture_active_bytes"] = json!(self.texture_bytes + 2048);
        resident["lightmap_reserved_bytes"] = json!(1024);
        resident["entity_overlay_reserved_bytes"] = json!(1024);
        resident["sunrise_reserved_bytes"] = json!(self.sunrise.reserved());
        resident["star_reserved_bytes"] = json!(u64::from(self.star_vertices) * 12);
        resident["instance_reserved_bytes"] = json!(((mesh::MAX_CHUNKS + 2) * 16) as u64);
        resident["terrain_indirect_reserved_bytes"] =
            json!(self.mdi.as_ref().map_or(0, mdi::Indirect::reserved_bytes));
        resident["camera_reserved_bytes"] = json!(std::mem::size_of::<CameraUniform>() as u64);
        resident["lightmap_revision"] =
            json!(self.lightmap_revision.map(|revision| revision.to_string()));
        resident["texture_retired_bytes"] =
            json!(self.retired_textures.iter().map(|t| t.bytes).sum::<u64>());
        resident["dynamic_reserved_bytes"] = json!(self.dynamic.reserved());
        resident["ui_reserved_bytes"] = json!(self.overlay.reserved());
        resident["scene_target_active_bytes"] =
            json!(u64::from(self.config.width) * u64::from(self.config.height) * 8);
        resident["outline_target_active_bytes"] =
            json!(u64::from(self.config.width) * u64::from(self.config.height) * 12);
        // Shadow cascades + opaque depth/colour copies + bloom, god-ray and TAA
        // history targets; the scene HDR target is reported separately above.
        resident["fancy_target_active_bytes"] = json!((self.shadow.target_bytes()
            + self.post.target_bytes())
        .saturating_sub(u64::from(self.config.width) * u64::from(self.config.height) * 8));
        resident["shadow_instance_reserved_bytes"] = json!(((mesh::MAX_CHUNKS + 2) * 16) as u64);
        resident["terrain_vertex_budget_bytes"] = json!(mesh::VERTEX_BUDGET);
        resident["terrain_index_budget_bytes"] = json!(mesh::INDEX_BUDGET);
    }
}
