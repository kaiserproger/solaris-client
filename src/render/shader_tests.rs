use super::*;
use bytemuck::Zeroable;

fn observation() -> Observation {
    Observation {
        instance_id: "shader-test".into(),
        connection_epoch: "1".into(),
        dimension_epoch: "1".into(),
        state_revision: "1".into(),
        last_applied_packet_sequence: "0".into(),
        client_tick: "1".into(),
        server_tick: None,
        resource_epoch: "1".into(),
        captured_frame_id: None,
        origin: "authored-render-regression".into(),
        truncated: false,
        next_cursor: None,
        data: json!({}),
    }
}
fn pixels(renderer: &mut Renderer, scene: &Scene) -> Vec<u8> {
    renderer.stats = FrameStats::default();
    let mut encoder = renderer.encode_scene(scene, &observation(), None).unwrap();
    let buffer = renderer.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("shader regression pixels"),
        size: 64 * 256,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &renderer.targets.color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(256),
                rows_per_image: Some(64),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    let index = renderer.queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    renderer
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let pixels = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    assert!(
        !renderer.gpu_failed.load(Ordering::Acquire),
        "{:?}",
        renderer.gpu_errors.try_recv()
    );
    pixels
}
#[test]
fn fancy_shader_gpu_toggle_preserves_ui_and_off_pixels() {
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64,
        64,
        PathBuf::from(".analysis/shaders-regression"),
    ))
    .unwrap();
    let mut scene = Scene {
        environment: crate::api::WorldEnvironment {
            sky_kind: SkyKind::Overworld,
            display_encoded: true,
            sky_color: [0.3, 0.5, 0.8],
            fog_color: [0.6, 0.7, 0.9],
            sun_angle: 45.,
            sun_texture: Some(crate::api::SkyTexture {
                uv: [0., 0., 1., 1.],
                layer: 0,
            }),
            moon_texture: Some(crate::api::SkyTexture {
                uv: [0., 0., 1., 1.],
                layer: 0,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    // Saturated UI patch must bypass HDR, bloom, lighting and tonemapping.
    for position in [[0., 0., 0.], [16., 0., 0.], [16., 16., 0.], [0., 16., 0.]] {
        scene.overlay.vertices.push(crate::api::Vertex {
            position,
            color: [0.25, 0.5, 0.75],
            alpha: 1.,
            uv: [-1.; 2],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            ..crate::api::Vertex::zeroed()
        });
    }
    scene.overlay.indices[0] = vec![0, 1, 2, 0, 2, 3];
    renderer.set_fancy_shaders(false);
    let off = pixels(&mut renderer, &scene);
    renderer.set_fancy_shaders(true);
    let fancy = pixels(&mut renderer, &scene);
    renderer.set_fancy_shaders(false);
    let off_again = pixels(&mut renderer, &scene);
    assert_eq!(
        off, off_again,
        "Off must return to the same vanilla-like pixels"
    );
    let ui_pixel = (8 * 64 + 8) * 4;
    assert_eq!(&off[ui_pixel..ui_pixel + 4], &fancy[ui_pixel..ui_pixel + 4]);
    let sky_pixel = (32 * 64 + 32) * 4;
    assert_ne!(
        &off[sky_pixel..sky_pixel + 3],
        &fancy[sky_pixel..sky_pixel + 3]
    );
    // Execute the depth-tinted water shader over actual opaque depth, not just sky.
    let mut geometry = crate::api::MeshGeometry::default();
    for (height, color, normal, layer) in [
        (-2., [0.7, 0.6, 0.4], [0., 1., 0.], 0usize),
        (0., [0.15, 0.4, 0.6], [0., 4., 0.], 2usize),
    ] {
        let first = geometry.vertices.len() as u32;
        for position in [
            [-4., height, 1.],
            [-4., height, 8.],
            [4., height, 8.],
            [4., height, 1.],
        ] {
            geometry.vertices.push(crate::api::Vertex {
                position,
                color,
                normal,
                alpha: if layer == 2 { 0.7 } else { 1. },
                uv: [-1.; 2],
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                light: [-1.; 2],
                ..crate::api::Vertex::zeroed()
            });
        }
        geometry.indices[layer].extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    scene.chunks.insert(
        [0; 3],
        crate::api::ChunkDraw {
            revision: 1,
            origin: [0.; 3],
            bounds_min: [-4., -2., 1.],
            bounds_max: [4., 0., 8.],
            visibility: (1u64 << 36) - 1,
            geometry: Some(Arc::new(geometry)),
            fade_duration_ms: Some(0),
        },
    );
    scene.revision += 1;
    scene.camera_position = [0., 2., 0.];
    scene.pitch = 20.;
    renderer.set_fancy_shaders(false);
    let water_off = pixels(&mut renderer, &scene);
    renderer.set_fancy_shaders(true);
    let water_fancy = pixels(&mut renderer, &scene);
    assert!(
        renderer.stats.submitted_indices >= 18,
        "opaque ground, water and UI must all submit"
    );
    assert_ne!(
        &water_off[sky_pixel..sky_pixel + 3],
        &water_fancy[sky_pixel..sky_pixel + 3]
    );
    assert_eq!(
        &water_off[ui_pixel..ui_pixel + 4],
        &water_fancy[ui_pixel..ui_pixel + 4]
    );
    scene.chunks.clear();
    scene.revision += 1;
    scene.camera_position = [0.; 3];
    scene.pitch = 0.;
    scene.environment.sky_color = [0.; 3];
    scene.environment.fog_color = [0.043, 0.035, 0.067];
    scene.environment.sun_angle = 180.;
    scene.environment.moon_angle = 0.;
    renderer.set_fancy_shaders(false);
    let night_off = pixels(&mut renderer, &scene);
    renderer.set_fancy_shaders(true);
    let night_fancy = pixels(&mut renderer, &scene);
    assert!(
        f32::from(night_fancy[sky_pixel + 2]) >= f32::from(night_off[sky_pixel + 2]) * 0.8,
        "Fancy tonemap must not crush the vanilla night horizon to black: Off={} Fancy={}",
        night_off[sky_pixel + 2],
        night_fancy[sky_pixel + 2]
    );
    renderer.resize(PhysicalSize::new(128, 96));
    assert_eq!(renderer.shadow.opaque_depth.width(), 128);
    assert_eq!(renderer.shadow.opaque_depth.height(), 96);
}

#[test]
fn terrain_mdi_compute_matches_fallback_pixels_across_arena_growth() {
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64,
        64,
        PathBuf::from(".analysis/clientcpu-regression"),
    ))
    .unwrap();
    if renderer.mdi.is_none() {
        eprintln!("MDI hardware unavailable; fallback covered by remaining render tests");
        return;
    }
    renderer.set_fancy_shaders(false);
    let mut scene = Scene::default();
    scene.camera_position = [0., 2., 0.];
    scene.pitch = 20.;
    // Force several growth copies with live allocations at nonzero offsets.
    renderer.vertex_arena = Arena::new(
        mesh::VERTEX_BUDGET,
        320,
        wgpu::BufferUsages::VERTEX,
        "test vertices",
    );
    renderer.index_arena = Arena::new(
        mesh::INDEX_BUDGET,
        32,
        wgpu::BufferUsages::INDEX,
        "test indices",
    );
    for (i, material) in [0, 1, 2, 0, 1, 0].into_iter().enumerate() {
        let mut geometry = crate::api::MeshGeometry::default();
        for position in [[-1., 0., 1.], [-1., 0., 4.], [1., 0., 4.], [1., 0., 1.]] {
            geometry.vertices.push(crate::api::Vertex {
                position,
                color: [0.2 + i as f32 * 0.1, 0.6, 0.3],
                normal: [0., 1., 0.],
                alpha: 1.,
                uv: [-1.; 2],
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                light: [-1.; 2],
                ..crate::api::Vertex::zeroed()
            });
        }
        geometry.indices[material] = vec![0, 1, 2, 0, 2, 3];
        scene.chunks.insert(
            [i as i32, 0, 0],
            crate::api::ChunkDraw {
                revision: 1,
                origin: [i as f64 * 2. - 4., 0., if i == 5 { -32. } else { 0. }],
                bounds_min: [-1., 0., 1.],
                bounds_max: [1., 0., 4.],
                visibility: visibility::ALL_OPEN,
                geometry: Some(Arc::new(geometry)),
                fade_duration_ms: Some(0),
            },
        );
    }
    scene.revision = 1;
    let fast = pixels(&mut renderer, &scene);
    let fast_calls = renderer.stats.draw_calls;
    let mdi = renderer.mdi.take().unwrap();
    let fallback = pixels(&mut renderer, &scene);
    assert_eq!(
        fast, fallback,
        "MDI must preserve opaque/cutout/translucent terrain"
    );
    assert!(fast_calls < renderer.stats.draw_calls);
    assert!(
        fast.chunks_exact(4).any(|p| p[1] > p[0]),
        "fixture must actually render"
    );
    renderer.mdi = Some(mdi);
    // Moving from a front-facing to a behind-camera candidate changes GPU culling.
    scene.yaw = 180.;
    let turned_fast = pixels(&mut renderer, &scene);
    let mdi = renderer.mdi.take().unwrap();
    let turned_fallback = pixels(&mut renderer, &scene);
    assert_eq!(
        turned_fast, turned_fallback,
        "fast turns cannot retain stale indirect args"
    );
    renderer.mdi = Some(mdi);
}
#[test]
fn fancy_shadow_indirect_matches_direct_pixels_with_fewer_calls() {
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64,
        64,
        PathBuf::from(".analysis/shadow-batch-regression"),
    ))
    .unwrap();
    if renderer.shadow.batch.is_none() {
        eprintln!("shadow MDI unavailable; direct fallback covered by other render tests");
        return;
    }
    let mut scene = Scene {
        environment: crate::api::WorldEnvironment {
            sky_kind: SkyKind::Overworld,
            display_encoded: true,
            sky_color: [0.3, 0.5, 0.8],
            fog_color: [0.6, 0.7, 0.9],
            sun_angle: 45.,
            sun_texture: Some(crate::api::SkyTexture {
                uv: [0., 0., 1., 1.],
                layer: 0,
            }),
            moon_texture: Some(crate::api::SkyTexture {
                uv: [0., 0., 1., 1.],
                layer: 0,
            }),
            ..Default::default()
        },
        ..Default::default()
    };
    // The elevated caster is outside the camera to its left; the 45-degree
    // sun projects its shadow onto the visible ground quad at x=0.
    for (i, material, origin) in [
        (0, 0, [0.; 3]),
        (1, 1, [32., 0., 0.]),
        (2, 0, [-8., 8., 0.]),
    ] {
        let mut geometry = crate::api::MeshGeometry::default();
        for position in [[-4., 0., 1.], [-4., 0., 8.], [4., 0., 8.], [4., 0., 1.]] {
            geometry.vertices.push(crate::api::Vertex {
                position,
                color: [0.7, 0.6, 0.4],
                normal: [0., 1., 0.],
                alpha: 1.,
                uv: if material == 1 { [0., 0.] } else { [-1., -1.] },
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                light: [-1.; 2],
                ..crate::api::Vertex::zeroed()
            });
        }
        geometry.indices[material] = vec![0, 1, 2, 0, 2, 3];
        scene.chunks.insert(
            [i, 0, 0],
            crate::api::ChunkDraw {
                revision: 1,
                origin,
                bounds_min: [-4., 0., 1.],
                bounds_max: [4., 0., 8.],
                visibility: (1u64 << 36) - 1,
                geometry: Some(Arc::new(geometry)),
                fade_duration_ms: Some(0),
            },
        );
    }
    scene.revision = 1;
    scene.camera_position = [0., 2., 0.];
    scene.pitch = 20.;
    let fast = pixels(&mut renderer, &scene);
    let fast_calls = renderer.stats.draw_calls;
    let fast_shadow_api = renderer.stats.shadow_terrain_api_calls;
    let fast_shadow_logical: u64 = renderer.stats.shadow_terrain_logical_draws.iter().sum();
    let fast_shadow_indices = renderer.stats.shadow_terrain_indices;
    // Reference mode disables every MDI path, including the shadow batch, so
    // the fallback issues one direct draw per caster range.
    renderer.set_reference_mode(true);
    let reference = pixels(&mut renderer, &scene);
    let reference_calls = renderer.stats.draw_calls;
    assert_eq!(renderer.stats.shadow_terrain_api_calls, 3);
    assert_eq!(renderer.stats.shadow_terrain_indices, fast_shadow_indices);
    renderer.set_reference_mode(false);
    assert_eq!(
        fast, reference,
        "shadow batching must preserve exact GPU pixels, including off-camera casters"
    );
    // Three casters: two opaque ranges plus one cutout range.
    assert_eq!(
        fast_shadow_logical, 3,
        "logical shadow draws must cover every selected caster range"
    );
    assert_eq!(
        fast_shadow_api, 1,
        "terrain shadows share one pipeline and batch"
    );
    assert!(
        fast_calls < reference_calls,
        "shadow batching must reduce native draw calls ({fast_calls} vs reference {reference_calls})"
    );
    assert!(
        fast_shadow_indices >= 18,
        "opaque and cutout casters must submit indices"
    );
    assert!(
        fast.chunks_exact(4).any(|p| p[1] > p[0]),
        "fixture must actually render"
    );
    scene.chunks.remove(&[2, 0, 0]);
    scene.revision += 1;
    let without_off_camera_caster = pixels(&mut renderer, &scene);
    assert_ne!(
        fast, without_off_camera_caster,
        "the off-camera caster must actually shadow visible pixels"
    );
}

#[test]
fn present_setting_selects_supported_uncapped_modes_and_fifo() {
    use wgpu::PresentMode::*;
    assert_eq!(present_mode(true, &[Mailbox, Immediate, Fifo]), Fifo);
    assert_eq!(present_mode(false, &[Mailbox, Immediate, Fifo]), Mailbox);
    assert_eq!(present_mode(false, &[Immediate, Fifo]), Immediate);
    assert_eq!(present_mode(false, &[Fifo]), AutoNoVsync);
}

#[test]
fn fancy_emission_enters_hdr_in_linear_space() {
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64,
        64,
        PathBuf::from(".analysis/shaders-hdr-regression"),
    ))
    .unwrap();
    let sprite = Some(crate::api::SkyTexture {
        uv: [0., 0., 1., 1.],
        layer: 0,
    });
    let mut scene = Scene {
        yaw: 90.,
        pitch: -45.,
        texture_rgba: Some(Arc::new(vec![128, 128, 128, 255])),
        texture_width: 1,
        texture_height: 1,
        texture_layers: 1,
        texture_revision: 1,
        environment: crate::api::WorldEnvironment {
            display_encoded: true,
            sky_kind: SkyKind::Overworld,
            sky_color: [0.; 3],
            fog_color: [0.; 3],
            sun_angle: 45.,
            moon_angle: 225.,
            sun_texture: sprite,
            moon_texture: sprite,
            ..Default::default()
        },
        ..Default::default()
    };
    let center = (32 * 64 + 32) * 4;
    let close = |actual: u8, expected: u8| {
        assert!(
            (i16::from(actual) - i16::from(expected)).abs() <= 2,
            "HDR pixel {actual}, expected {expected} ±2"
        )
    };
    renderer.set_fancy_shaders(false);
    close(pixels(&mut renderer, &scene)[center], 128);
    renderer.set_fancy_shaders(true);
    // sRGB(128) -> linear 0.21586 -> exposure 3.2 -> ACES -> sRGB ~=220.
    close(pixels(&mut renderer, &scene)[center], 220);

    scene.yaw = 0.;
    scene.pitch = 90.;
    scene.environment.dark_sky_disc = true;
    scene.environment.sky_color = [0.18; 3];
    scene.environment.fog_color = [0.18; 3];
    // The dark disc decodes fog before applying its radial visibility, not after.
    close(pixels(&mut renderer, &scene)[center], 3);

    scene.pitch = 0.;
    scene.environment.dark_sky_disc = false;
    scene.environment.sky_color = [0.; 3];
    scene.environment.fog_color = [0.; 3];
    scene.environment.sun_angle = 0.;
    scene.environment.moon_angle = 180.;
    for position in [[-2., -2., 4.], [-2., 2., 4.], [2., 2., 4.], [2., -2., 4.]] {
        scene.dynamic.vertices.push(crate::api::Vertex {
            position,
            color: [0.5; 3],
            normal: [0., 0., -1.],
            alpha: 1.,
            uv: [-1.; 2],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            light: [-1.; 2],
            ..crate::api::Vertex::zeroed()
        });
    }
    scene.dynamic.indices[EntityMaterial::Lightning as usize] = vec![0, 1, 2, 0, 2, 3];
    scene.revision += 1;
    renderer.set_fancy_shaders(false);
    close(pixels(&mut renderer, &scene)[center], 128);
    renderer.set_fancy_shaders(true);
    // Unlit source kind 4: linear(0.5)=0.214 -> ACES -> display ~=154.
    close(pixels(&mut renderer, &scene)[center], 154);

    scene.dynamic.indices[EntityMaterial::Lightning as usize].clear();
    scene.dynamic.indices[EntityMaterial::Glint as usize] = vec![0, 1, 2, 0, 2, 3];
    for vertex in &mut scene.dynamic.vertices {
        vertex.color = [1.; 3];
        vertex.uv = [0.5; 2];
        vertex.overlay_uv = [0.; 2];
    }
    // Glint uses depth-equal and source-color blending: black geometry supplies
    // its exact depth, so the expected contribution is linear(128/255)^2.
    let mut backing = crate::api::MeshGeometry::default();
    backing.vertices = scene
        .dynamic
        .vertices
        .iter()
        .map(|vertex| crate::api::Vertex {
            color: [0.; 3],
            uv: [-1.; 2],
            overlay_uv: [-1.; 2],
            ..*vertex
        })
        .collect();
    backing.indices[0] = vec![0, 1, 2, 0, 2, 3];
    scene.chunks.insert(
        [0; 3],
        crate::api::ChunkDraw {
            revision: 1,
            origin: [0.; 3],
            bounds_min: [-2., -2., 4.],
            bounds_max: [2., 2., 4.],
            visibility: visibility::ALL_OPEN,
            geometry: Some(Arc::new(backing)),
            fade_duration_ms: Some(0),
        },
    );
    scene.revision += 1;
    renderer.set_fancy_shaders(false);
    close(pixels(&mut renderer, &scene)[center], 64);
    renderer.set_fancy_shaders(true);
    close(pixels(&mut renderer, &scene)[center], 56);
}

fn terrain_release_quad() -> crate::api::MeshGeometry {
    let mut geometry = crate::api::MeshGeometry::default();
    for position in [[-1., 0., 1.], [-1., 0., 4.], [1., 0., 4.], [1., 0., 1.]] {
        geometry.vertices.push(crate::api::Vertex {
            position,
            color: [0.4, 0.6, 0.3],
            normal: [0., 1., 0.],
            alpha: 1.,
            uv: [-1.; 2],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            light: [-1.; 2],
            ..crate::api::Vertex::zeroed()
        });
    }
    geometry.indices[0] = vec![0, 1, 2, 0, 2, 3];
    geometry
}

#[test]
fn terrain_cpu_release_keeps_gpu_geometry_and_requires_fresh_payload() {
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64,
        64,
        PathBuf::from(".analysis/clientcpu-release-regression"),
    ))
    .unwrap();
    renderer.set_fancy_shaders(false);
    let mut scene = Scene {
        camera_position: [0., 2., 0.],
        pitch: 20.,
        ..Scene::default()
    };
    scene.chunks.insert(
        [0; 3],
        crate::api::ChunkDraw {
            revision: 1,
            origin: [0.; 3],
            bounds_min: [-1., 0., 1.],
            bounds_max: [1., 0., 4.],
            visibility: visibility::ALL_OPEN,
            geometry: Some(Arc::new(terrain_release_quad())),
            fade_duration_ms: Some(0),
        },
    );
    scene.revision = 1;
    let first = pixels(&mut renderer, &scene);
    let first_indices = renderer.stats.submitted_indices;
    assert!(first_indices >= 6, "terrain quad must submit");
    // Exact ack only for the committed upload; the follow-up drain is empty.
    let acks: Vec<_> = renderer.drain_uploaded_meshes().collect();
    assert_eq!(acks, [([0; 3], 1)]);
    assert_eq!(renderer.drain_uploaded_meshes().count(), 0);
    // Parent released the CPU payload after the successful submission.
    scene.chunks.get_mut(&[0; 3]).unwrap().geometry = None;
    let second = pixels(&mut renderer, &scene);
    assert_eq!(
        first, second,
        "GPU geometry must keep rendering after the CPU drop"
    );
    assert_eq!(
        renderer.stats.submitted_indices, first_indices,
        "resident mesh must submit identical indices without reupload"
    );
    assert_eq!(renderer.drain_uploaded_meshes().count(), 0);
    // A resource-only (atlas) change reuses the exact resident mesh.
    scene.texture_revision = 2;
    let reused = pixels(&mut renderer, &scene);
    assert_eq!(
        first, reused,
        "resource-only change must reuse the resident mesh"
    );
    assert_eq!(renderer.drain_uploaded_meshes().count(), 0);
    scene.texture_revision = 1;
    // A replacement revision without payload must not render stale mesh.
    scene.chunks.get_mut(&[0; 3]).unwrap().revision = 2;
    let Err(err) = renderer.encode_scene(&scene, &observation(), None) else {
        panic!("replacement without payload must fail");
    };
    assert!(
        err.contains("terrain_cpu_payload_missing"),
        "mismatched resident revision without payload must request remesh, got: {err}"
    );
    // Fresh payload for the replacement commits and acks exactly once.
    scene.chunks.get_mut(&[0; 3]).unwrap().geometry = Some(Arc::new(terrain_release_quad()));
    let third = pixels(&mut renderer, &scene);
    assert_eq!(first, third, "identical replacement must restore pixels");
    let acks: Vec<_> = renderer.drain_uploaded_meshes().collect();
    assert_eq!(acks, [([0; 3], 2)]);
}

/// Eight-corner-free axis-aligned box faces in `FaceGroups` order
/// (`-Y,+Y,-Z,+Z,-X,+X`), wound CCW seen from outside, plus a diagonal quad.
fn facing_box_geometry(partitioned: bool) -> crate::api::MeshGeometry {
    let (x0, x1, y0, y1, z0, z1) = (-2., 2., 0.5, 4.5, 6., 10.);
    let faces: [[[f32; 3]; 4]; 6] = [
        [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
        [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        [[x0, y0, z0], [x0, y1, z0], [x1, y1, z0], [x1, y0, z0]],
        [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
        [[x0, y0, z0], [x0, y0, z1], [x0, y1, z1], [x0, y1, z0]],
        [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
    ];
    let diagonal = [[-6., 0.5, 7.], [-5., 0.5, 8.], [-5., 3., 8.], [-6., 3., 7.]];
    let mut geometry = crate::api::MeshGeometry::default();
    let mut quads: Vec<[[f32; 3]; 4]> = faces.to_vec();
    quads.push(diagonal);
    for (i, corners) in quads.iter().enumerate() {
        let first = geometry.vertices.len() as u32;
        for position in corners {
            geometry.vertices.push(crate::api::Vertex {
                position: *position,
                color: [0.2 + 0.12 * i as f32, 0.8 - 0.1 * i as f32, 0.3],
                normal: [0., 1., 0.],
                alpha: 1.,
                uv: [-1.; 2],
                overlay_uv: [-1.; 2],
                overlay_tint: [1.; 3],
                light: [-1.; 2],
                ..crate::api::Vertex::zeroed()
            });
        }
        geometry.indices[0].extend([first, first + 1, first + 2, first, first + 2, first + 3]);
    }
    if partitioned {
        geometry.faces[0] = Some(crate::api::FaceGroups {
            ends: [6, 12, 18, 24, 30, 36, 36, 42],
            // -Y group: highest plane; +Y: lowest; others analogous.
            planes: [y0, y1, z0, z1, x0, x1],
        });
    }
    geometry
}

#[test]
fn terrain_facing_groups_preserve_pixels_and_drop_back_faces() {
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        64,
        64,
        PathBuf::from(".analysis/facing-groups-regression"),
    ))
    .unwrap();
    renderer.set_fancy_shaders(true);
    let environment = crate::api::WorldEnvironment {
        sky_kind: SkyKind::Overworld,
        display_encoded: true,
        sky_color: [0.3, 0.5, 0.8],
        fog_color: [0.6, 0.7, 0.9],
        sun_angle: 45.,
        sun_texture: Some(crate::api::SkyTexture {
            uv: [0., 0., 1., 1.],
            layer: 0,
        }),
        moon_texture: Some(crate::api::SkyTexture {
            uv: [0., 0., 1., 1.],
            layer: 0,
        }),
        ..Default::default()
    };
    let scene_for = |partitioned: bool| {
        let revision = 1 + u64::from(partitioned);
        let mut scene = Scene {
            environment: environment.clone(),
            camera_position: [3., 3., 0.],
            yaw: 0.,
            pitch: 0.,
            ..Default::default()
        };
        scene.chunks.insert(
            [0, 0, 0],
            crate::api::ChunkDraw {
                revision,
                origin: [0.; 3],
                bounds_min: [-6., 0.5, 6.],
                bounds_max: [2., 4.5, 10.],
                visibility: visibility::ALL_OPEN,
                geometry: Some(Arc::new(facing_box_geometry(partitioned))),
                fade_duration_ms: Some(0),
            },
        );
        scene.revision = revision;
        scene
    };
    let whole_scene = scene_for(false);
    let whole = pixels(&mut renderer, &whole_scene);
    let whole_main = renderer.stats.terrain_opaque_indices;
    let whole_shadow = renderer.stats.shadow_terrain_indices;
    let split_scene = scene_for(true);
    let split = pixels(&mut renderer, &split_scene);
    let split_main = renderer.stats.terrain_opaque_indices;
    let split_shadow = renderer.stats.shadow_terrain_indices;
    assert_eq!(whole_main, 42);
    assert_eq!(whole_shadow, 42);
    // Camera at x=3,y=3 sees only -Z and +X of the box; the diagonal quad stays.
    assert_eq!(split_main, 18, "front faces plus the non-planar quad");
    assert!(
        split_shadow < whole_shadow,
        "light-facing partition must drop faces turned away from the sun"
    );
    assert_eq!(
        whole, split,
        "dropping back faces must not change a single pixel"
    );
    assert!(
        whole.chunks_exact(4).any(|p| p[1] > p[0]),
        "fixture must actually render"
    );
}

/// Runs the terrain vertex decode (`decode_terrain`, the exact function both
/// terrain vertex stages call) on the GPU over raw packed bytes and returns 24
/// floats per vertex: position, color, alpha, uv, layer, light, class, weight,
/// normal, tile origin, tile size/marker.
fn gpu_decode(renderer: &Renderer, packed: &[packed::TerrainVertex]) -> Vec<f32> {
    const PROBE: &str = r#"
@group(2) @binding(0) var<storage, read> raw: array<u32>;
@group(2) @binding(1) var<storage, read_write> decoded: array<f32>;
@compute @workgroup_size(1)
fn probe(@builtin(global_invocation_id) id: vec3<u32>) {
    let w = id.x * 5u;
    let a = raw[w]; let b = raw[w + 1u]; let c = raw[w + 2u]; let d = raw[w + 3u]; let e = raw[w + 4u];
    let t = decode_terrain(vec4<u32>(a & 0xffffu, a >> 16u, b & 0xffffu, b >> 16u), unpack4x8unorm(c),
        vec4<u32>(d & 255u, (d >> 8u) & 255u, (d >> 16u) & 255u, d >> 24u),
        vec2<u32>(e & 0xffffu, e >> 16u));
    let o = id.x * 24u;
    decoded[o] = t.position.x; decoded[o + 1u] = t.position.y; decoded[o + 2u] = t.position.z;
    decoded[o + 3u] = t.color.x; decoded[o + 4u] = t.color.y; decoded[o + 5u] = t.color.z;
    decoded[o + 6u] = t.alpha; decoded[o + 7u] = t.uv.x; decoded[o + 8u] = t.uv.y;
    decoded[o + 9u] = f32(t.layer); decoded[o + 10u] = t.light.x; decoded[o + 11u] = t.light.y;
    decoded[o + 12u] = f32(t.class_kind); decoded[o + 13u] = t.weight;
    decoded[o + 14u] = t.normal.x; decoded[o + 15u] = t.normal.y; decoded[o + 16u] = t.normal.z;
    decoded[o + 17u] = t.tile_origin.x; decoded[o + 18u] = t.tile_origin.y;
    decoded[o + 19u] = t.tile.x; decoded[o + 20u] = t.tile.y; decoded[o + 21u] = t.tile.z;
}
"#;
    let device = &renderer.device;
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("packed terrain decode probe"),
        source: wgpu::ShaderSource::Wgsl(
            format!("{}{PROBE}", include_str!("../shader.wgsl")).into(),
        ),
    });
    let layout_entry = |binding, ty| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty,
        count: None,
    };
    let storage = |read_only| wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let empty = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[],
    });
    let tiles = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[layout_entry(
            6,
            wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        )],
    });
    let buffers = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            layout_entry(0, storage(true)),
            layout_entry(1, storage(false)),
        ],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&empty, &tiles, &buffers],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("packed terrain decode probe"),
        layout: Some(&pipeline_layout),
        module: &module,
        entry_point: Some("probe"),
        compilation_options: Default::default(),
        cache: None,
    });
    let raw = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: std::mem::size_of_val(packed) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    renderer
        .queue
        .write_buffer(&raw, 0, bytemuck::cast_slice(packed));
    let out_bytes = (packed.len() * 24 * 4) as u64;
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: out_bytes,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: out_bytes,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let view = renderer.tile_texture.create_view(&Default::default());
    let group = |layout, entries: &[wgpu::BindGroupEntry]| {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries,
        })
    };
    let g0 = group(&empty, &[]);
    let g1 = group(
        &tiles,
        &[wgpu::BindGroupEntry {
            binding: 6,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    );
    let g2 = group(
        &buffers,
        &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: raw.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: out.as_entire_binding(),
            },
        ],
    );
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &g0, &[]);
        pass.set_bind_group(1, &g1, &[]);
        pass.set_bind_group(2, &g2, &[]);
        pass.dispatch_workgroups(packed.len() as u32, 1, 1);
    }
    encoder.copy_buffer_to_buffer(&out, 0, &read, 0, out_bytes);
    let index = renderer.queue.submit([encoder.finish()]);
    let (tx, rx) = mpsc::sync_channel(1);
    read.slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            tx.send(result).unwrap();
        });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let decoded = bytemuck::cast_slice(&read.slice(..).get_mapped_range()).to_vec();
    read.unmap();
    decoded
}

#[test]
fn packed_terrain_shader_decode_matches_mesher_vertices() {
    use crate::api::Vertex;
    let mut renderer = pollster::block_on(Renderer::new_offscreen(
        32,
        32,
        PathBuf::from(".analysis/packed-decode"),
    ))
    .unwrap();
    let base = Vertex {
        position: [3.0625, 15.9375, -0.5],
        color: [61. / 255., 140. / 255., 1.],
        uv: [0.25, 0.8125],
        overlay_uv: [-1.; 2],
        overlay_tint: [1.; 3],
        alpha: 1.,
        texture_layer: 7,
        light: [13.25, 4.0625],
        normal: [0., 1., 0.],
    };
    let tiled = Vertex {
        uv: [16., -15.],
        overlay_uv: [0.125, 0.40625],
        overlay_tint: [0.0625, 0.03125, -1.],
        texture_layer: 5,
        normal: [0., 0., -4.],
        ..base
    };
    let mut vertices = vec![
        base,
        Vertex {
            light: [-1.; 2],
            uv: [-1.; 2],
            alpha: 0.5,
            ..base
        },
        Vertex {
            normal: [3., 0., 0.],
            light: [0., 15.],
            ..base
        },
        tiled,
        Vertex {
            uv: [0., 0.],
            texture_layer: 5,
            ..tiled
        },
        packed::blank_vertices(1)[0],
    ];
    for face in 0..6 {
        let direction = packed::FACE_NORMALS[face];
        vertices.push(Vertex {
            normal: direction.map(|c| c * 4.),
            ..base
        });
    }
    for height in [0f32, 0.0625, 0.5, 0.9375, 1.] {
        vertices.push(Vertex {
            normal: [0., 2. + 0.2 * height, 0.],
            ..base
        });
    }
    let packed = packed::pack_vertices(&vertices, &mut renderer.tiles).unwrap();
    renderer
        .tiles
        .flush(&renderer.queue, &renderer.tile_texture);
    let decoded = gpu_decode(&renderer, &packed);
    for (i, vertex) in vertices.iter().enumerate() {
        let got = &decoded[i * 24..i * 24 + 22];
        let near = |actual: f32, expected: f32, tolerance: f32, what: &str| {
            assert!(
                (actual - expected).abs() <= tolerance,
                "vertex {i} {what}: gpu {actual} vs mesher {expected}"
            );
        };
        for axis in 0..3 {
            near(
                got[axis],
                vertex.position[axis],
                1. / 4096. + 1e-6,
                "position",
            );
            near(got[3 + axis], vertex.color[axis], 0.6 / 255., "color");
        }
        near(got[6], vertex.alpha, 0.6 / 255., "alpha");
        let tiled = vertex.overlay_tint[2] < 0.;
        let (uv_tolerance, layer) = if tiled {
            (0., 5.)
        } else {
            (1. / 32768., vertex.texture_layer as f32)
        };
        for axis in 0..2 {
            if vertex.uv[axis] < 0. && !tiled {
                near(got[7 + axis], -1., 0., "untextured uv");
            } else {
                near(got[7 + axis], vertex.uv[axis], uv_tolerance, "uv");
            }
        }
        near(got[9], layer, 0., "layer");
        let lit = vertex.light.iter().all(|l| *l >= 0.);
        for channel in 0..2 {
            near(
                got[10 + channel],
                if lit { vertex.light[channel] } else { -1. },
                0.,
                "light",
            );
        }
        let length = vertex.normal.iter().map(|c| c * c).sum::<f32>().sqrt();
        near(got[12], length.round(), 0., "class");
        if length.round() == 2. {
            near(
                got[13],
                ((length - 2.) * 5.).clamp(0., 1.),
                1. / 256.,
                "rooted weight",
            );
        }
        for axis in 0..3 {
            let unit = if length > 0. {
                vertex.normal[axis] / length
            } else {
                0.
            };
            near(got[14 + axis], unit, 1e-6, "normal");
        }
        if tiled {
            near(got[17], vertex.overlay_uv[0], 0., "tile origin");
            near(got[18], vertex.overlay_uv[1], 0., "tile origin");
            near(got[19], vertex.overlay_tint[0], 0., "tile size");
            near(got[20], vertex.overlay_tint[1], 0., "tile size");
            near(got[21], -1., 0., "tile marker");
        } else {
            near(got[17], -1., 0., "no tile origin");
            near(got[21], 1., 0., "no tile marker");
        }
    }
}
