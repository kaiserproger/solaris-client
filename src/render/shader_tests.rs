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
