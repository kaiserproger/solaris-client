//! GPU regressions for the shader design pass: atmosphere sky/fog, god rays,
//! cascaded shadows, High water, TAA and preset switching. Every test renders
//! on a real device (no mocks); frames are 64x64.

use super::graphics::{jitter_pixels, Features};
use super::shader_tests::pixels;
use super::*;
use bytemuck::Zeroable;

const CENTER: usize = (32 * 64 + 32) * 4;

struct LockedRenderer {
    renderer: Renderer,
    _guard: std::sync::MutexGuard<'static, ()>,
}
impl std::ops::Deref for LockedRenderer {
    type Target = Renderer;
    fn deref(&self) -> &Renderer { &self.renderer }
}
impl std::ops::DerefMut for LockedRenderer {
    fn deref_mut(&mut self) -> &mut Renderer { &mut self.renderer }
}
fn offscreen(label: &str) -> LockedRenderer {
    let guard = gpu_test_lock();
    let renderer = pollster::block_on(Renderer::new_offscreen(
        64,
        64,
        PathBuf::from(format!(".analysis/design-regression-{label}")),
    ))
    .unwrap();
    LockedRenderer { renderer, _guard: guard }
}

fn overworld(sun_angle: f32, moon_angle: f32) -> crate::api::WorldEnvironment {
    crate::api::WorldEnvironment {
        display_encoded: true,
        sky_kind: SkyKind::Overworld,
        sky_color: [0.47, 0.65, 1.0],
        fog_color: [0.75, 0.85, 1.0],
        sun_angle,
        moon_angle,
        sun_texture: Some(SPRITE),
        moon_texture: Some(SPRITE),
        ..Default::default()
    }
}

const SPRITE: crate::api::SkyTexture = crate::api::SkyTexture {
    uv: [0., 0., 1., 1.],
    layer: 0,
};

/// A fully transparent 1x1 atlas: the required sun/moon sprites exist but add
/// no light, so tests observe the atmosphere alone.
fn transparent_atlas() -> Scene {
    Scene {
        texture_rgba: Some(Arc::new(vec![0, 0, 0, 0])),
        texture_width: 1,
        texture_height: 1,
        texture_layers: 1,
        texture_revision: 1,
        ..Default::default()
    }
}

fn quad(
    geometry: &mut crate::api::MeshGeometry,
    material: usize,
    corners: [[f32; 3]; 4],
    color: [f32; 3],
    normal: [f32; 3],
    alpha: f32,
) {
    let first = geometry.vertices.len() as u32;
    for position in corners {
        geometry.vertices.push(crate::api::Vertex {
            position,
            color,
            normal,
            alpha,
            uv: [-1.; 2],
            overlay_uv: [-1.; 2],
            overlay_tint: [1.; 3],
            light: [-1.; 2],
            ..crate::api::Vertex::zeroed()
        });
    }
    geometry.indices[material].extend([first, first + 1, first + 2, first, first + 2, first + 3]);
}

fn chunk(
    scene: &mut Scene,
    key: [i32; 3],
    origin: [f64; 3],
    min: [f32; 3],
    max: [f32; 3],
    geometry: crate::api::MeshGeometry,
) {
    scene.chunks.insert(
        key,
        crate::api::ChunkDraw {
            revision: 1,
            origin,
            bounds_min: min,
            bounds_max: max,
            visibility: (1u64 << 36) - 1,
            geometry: Some(Arc::new(geometry)),
            fade_duration_ms: Some(0),
        },
    );
    scene.revision += 1;
}

fn rgb(frame: &[u8], offset: usize) -> [f32; 3] {
    [
        f32::from(frame[offset]),
        f32::from(frame[offset + 1]),
        f32::from(frame[offset + 2]),
    ]
}

fn mean_abs_diff(a: &[u8], b: &[u8]) -> f32 {
    let total: f32 = a
        .chunks_exact(4)
        .zip(b.chunks_exact(4))
        .map(|(p, q)| {
            (0..3)
                .map(|c| (f32::from(p[c]) - f32::from(q[c])).abs())
                .sum::<f32>()
        })
        .sum();
    total / (a.len() / 4 * 3) as f32
}

/// Looks along `yaw` at the horizon (yaw 90 faces -X, the sun's side for small angles).
fn sky_at(
    renderer: &mut Renderer,
    sun_angle: f32,
    moon_angle: f32,
    yaw: f32,
    pitch: f32,
) -> [f32; 3] {
    let scene = Scene {
        yaw,
        pitch,
        environment: overworld(sun_angle, moon_angle),
        ..transparent_atlas()
    };
    let frame = pixels(renderer, &scene);
    rgb(&frame, CENTER)
}

#[test]
fn sunset_sky_warms_toward_the_sun_and_noon_stays_blue() {
    let mut renderer = offscreen("sky");
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    let warmth = |c: [f32; 3]| c[0] / c[2].max(1.);
    let noon = sky_at(&mut renderer, 0., 180., 90., 0.);
    let toward = sky_at(&mut renderer, 84., 264., 90., 0.);
    let away = sky_at(&mut renderer, 84., 264., -90., 0.);
    // Looking 45 degrees off the zenith: straight up would stare into the noon sun.
    let zenith = sky_at(&mut renderer, 0., 180., 0., -45.);
    assert!(
        noon[2] > noon[0],
        "noon horizon must stay blue-white: {noon:?}"
    );
    assert!(
        zenith[2] > zenith[0] * 1.3,
        "noon zenith must be clearly blue: {zenith:?}"
    );
    assert!(
        warmth(toward) > warmth(noon) * 1.25,
        "sunset horizon toward the sun must be warmer than noon: {toward:?} vs {noon:?}"
    );
    assert!(
        warmth(toward) > warmth(away) * 1.1,
        "sun side must be warmer than the anti-solar side: {toward:?} vs {away:?}"
    );
    // Sun below the horizon, moon up, vanilla night colours: dark but not black,
    // and never dimmer than the vanilla gradient it replaces.
    let night_scene = Scene {
        yaw: 90.,
        environment: crate::api::WorldEnvironment {
            sky_color: [0.0, 0.0, 0.02],
            fog_color: [0.043, 0.035, 0.067],
            ..overworld(180., 0.)
        },
        ..transparent_atlas()
    };
    let night = rgb(&pixels(&mut renderer, &night_scene), CENTER);
    renderer.set_graphics_preset(GraphicsPreset::Low);
    let vanilla_night = rgb(&pixels(&mut renderer, &night_scene), CENTER);
    assert!(
        night[2] < noon[2] * 0.5 && night[2] > 1. && night[2] >= vanilla_night[2] * 0.8,
        "night sky {night:?} vs vanilla {vanilla_night:?}"
    );
}

#[test]
fn twilight_sky_is_orange_at_the_sun_and_blue_overhead() {
    let mut renderer = offscreen("twilight");
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    // Sun ~4 degrees above the horizon: single-scatter with a red-only
    // transmittance would paint the whole dome one orange.
    let toward = sky_at(&mut renderer, 86., 266., 90., 0.);
    let overhead = sky_at(&mut renderer, 86., 266., 0., -60.);
    assert!(
        toward[0] > toward[2] * 1.1,
        "horizon at the sun must be orange: {toward:?}"
    );
    assert!(
        overhead[2] > overhead[0] * 1.5,
        "twilight zenith must stay blue: {overhead:?}"
    );
}

#[test]
fn low_sky_keeps_the_vanilla_gradient_untouched() {
    let mut renderer = offscreen("low-sky");
    renderer.set_graphics_preset(GraphicsPreset::Low);
    let toward = sky_at(&mut renderer, 84., 264., 90., 0.);
    let away = sky_at(&mut renderer, 84., 264., -90., 0.);
    // The vanilla sky has no sun-side scattering: both sides of the horizon match.
    assert_eq!(toward, away, "Low must not add atmosphere");
}

#[test]
fn terrain_fog_ends_exactly_on_the_sky_pixel_behind_it() {
    let mut renderer = offscreen("fog");
    renderer.set_reference_mode(true); // keep fully fogged sections (no fog cull)
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    let mut scene = Scene {
        environment: crate::api::WorldEnvironment {
            fog_start: 0.,
            fog_end: 10.,
            ..overworld(60., 240.)
        },
        ..transparent_atlas()
    };
    let sky = pixels(&mut renderer, &scene);
    let mut geometry = crate::api::MeshGeometry::default();
    // Packed terrain positions span [-8, 24) around the chunk origin: a 16-block
    // wall 12 blocks ahead covers the pixels probed below and is past fog_end 10.
    quad(
        &mut geometry,
        0,
        [[0., 0., 0.], [0., 16., 0.], [16., 16., 0.], [16., 0., 0.]],
        [0.7, 0.3, 0.2],
        [0., 0., -1.],
        1.,
    );
    chunk(
        &mut scene,
        [0; 3],
        [-8., -8., 12.],
        [0., 0., 0.],
        [16., 16., 0.],
        geometry,
    );
    let fogged = pixels(&mut renderer, &scene);
    for pixel in [(8, 8), (32, 32), (55, 20), (20, 55)] {
        let offset = (pixel.1 * 64 + pixel.0) * 4;
        for channel in 0..3 {
            let (a, b) = (sky[offset + channel], fogged[offset + channel]);
            assert!(
                a.abs_diff(b) <= 2,
                "fogged terrain at {pixel:?} must equal the sky ({a} vs {b})"
            );
        }
    }
    // With far terrain, the loaded terrain must use the far edge fog curve,
    // not disappear at the old near fog boundary.
    scene.far.distance = 1000.;
    scene.environment.far_fog = true;
    let with_far = pixels(&mut renderer, &scene);
    assert!(
        mean_abs_diff(&sky, &with_far) > 20.,
        "loaded terrain must remain visible when far terrain is enabled"
    );
    scene.far.distance = 0.;
    // Control: without fog the quad is visible, so the match above is not vacuous.
    scene.environment.fog_start = 5000.;
    scene.environment.fog_end = 6000.;
    scene.environment.render_fog_start = 5000.;
    scene.environment.render_fog_end = 6000.;
    let visible = pixels(&mut renderer, &scene);
    assert!(
        mean_abs_diff(&sky, &visible) > 20.,
        "unfogged quad must differ from the sky"
    );
}

/// Left-half depth occluder + sky elsewhere, run through the real post chain.
fn god_ray_frame(renderer: &Renderer, ray_color: [f32; 3]) -> Vec<u8> {
    let device = &renderer.device;
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("god ray test depth"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: DEPTH,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let depth_view = depth.create_view(&Default::default());
    let features = Features {
        hdr: true,
        cascades: 1,
        god_ray_taps: 16,
        water_ssr: false,
        taa: false,
    };
    let mut post = post::Post::new(device, 64, 64, &depth_view, &features);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("left half occluder"),
        source: wgpu::ShaderSource::Wgsl(
            "@vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                var p = array<vec2<f32>, 6>(vec2(-1.0, -1.0), vec2(0.0, -1.0), vec2(-1.0, 1.0),
                    vec2(0.0, -1.0), vec2(0.0, 1.0), vec2(-1.0, 1.0));
                return vec4<f32>(p[i], 0.2, 1.0);
            }"
            .into(),
        ),
    });
    let occluder = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("left half occluder"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        primitive: Default::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: DEPTH,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        fragment: None,
        multiview: None,
        cache: None,
    });
    let output = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("god ray test output"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: COLOR,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    drop(encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("black scene"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &post.scene_view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
    }));
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("occluder depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_pipeline(&occluder);
        pass.draw(0..6, 0..1);
    }
    post.update(
        &renderer.queue,
        &post::PostFrame {
            inverse_view_projection: Mat4::IDENTITY,
            previous_view_projection: Mat4::IDENTITY,
            camera_delta: Vec3::ZERO,
            history_valid: false,
            ray_color,
            sun_uv: [0.5, 0.5],
            sun_in_front: true,
            jitter_ndc: [0.; 2],
        },
    );
    post.encode(&mut encoder, &output_view, true, &renderer.timing);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("god ray readback"),
        size: 64 * 256,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &output,
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
        .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: Some(std::time::Duration::from_secs(30)),
        })
        .unwrap();
    rx.recv().unwrap().unwrap();
    let data = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    assert!(
        !renderer.gpu_failed.load(Ordering::Acquire),
        "{:?}",
        renderer.gpu_errors.try_recv()
    );
    data
}

#[test]
fn god_rays_light_open_sky_toward_the_sun_and_stop_at_occluders() {
    let renderer = offscreen("rays");
    let off = god_ray_frame(&renderer, [0.; 3]);
    assert!(
        off.chunks_exact(4)
            .all(|p| p[0] <= 1 && p[1] <= 1 && p[2] <= 1),
        "no shafts without strength"
    );
    let lit = god_ray_frame(&renderer, [1., 1., 1.]);
    let at = |frame: &[u8], x: usize, y: usize| rgb(frame, (y * 64 + x) * 4);
    // Right half: every tap toward the sun at screen centre sees open sky.
    assert!(
        at(&lit, 60, 32)[0] > 60.,
        "open sky near the sun must glow: {:?}",
        at(&lit, 60, 32)
    );
    // Far left: the whole line to the sun lies behind the occluder.
    assert!(
        at(&lit, 2, 32)[0] <= 1.,
        "occluded line of sight must stay dark: {:?}",
        at(&lit, 2, 32)
    );
    // Shafts fall off with distance from the sun.
    assert!(
        at(&lit, 36, 32)[0] > at(&lit, 62, 32)[0],
        "glow must fade away from the sun"
    );
}

fn lit_ground_and_caster(scene: &mut Scene) {
    // Ground at y=0 in front of the camera; a plate hovers off to the side so
    // the 45 degree sun throws its shadow onto the visible ground.
    let mut ground = crate::api::MeshGeometry::default();
    quad(
        &mut ground,
        0,
        [[-4., 0., 1.], [-4., 0., 8.], [4., 0., 8.], [4., 0., 1.]],
        [0.7, 0.6, 0.4],
        [0., 1., 0.],
        1.,
    );
    chunk(scene, [0; 3], [0.; 3], [-4., 0., 1.], [4., 0., 8.], ground);
    let mut caster = crate::api::MeshGeometry::default();
    quad(
        &mut caster,
        0,
        [[-4., 0., 1.], [-4., 0., 8.], [4., 0., 8.], [4., 0., 1.]],
        [0.7, 0.6, 0.4],
        [0., 1., 0.],
        1.,
    );
    chunk(
        scene,
        [2, 0, 0],
        [-8., 8., 0.],
        [-4., 0., 1.],
        [4., 0., 8.],
        caster,
    );
    scene.camera_position = [0., 2., 0.];
    scene.pitch = 20.;
}

#[test]
fn every_cascade_count_shadows_the_visible_ground() {
    for preset in [GraphicsPreset::Medium, GraphicsPreset::High] {
        let mut renderer = offscreen("cascades");
        renderer.set_graphics_preset(preset);
        let mut scene = Scene {
            environment: overworld(45., 225.),
            ..transparent_atlas()
        };
        lit_ground_and_caster(&mut scene);
        let shadowed = pixels(&mut renderer, &scene);
        assert_eq!(
            renderer.shadow.layers.len() as u32,
            preset.features().cascades,
            "{preset:?} cascade layers"
        );
        assert!(
            renderer.stats.shadow_terrain_indices >= 6,
            "{preset:?} must submit casters"
        );
        scene.chunks.remove(&[2, 0, 0]);
        scene.revision += 1;
        let open = pixels(&mut renderer, &scene);
        assert!(
            mean_abs_diff(&shadowed, &open) > 0.5,
            "{preset:?}: the caster must darken visible ground ({})",
            mean_abs_diff(&shadowed, &open)
        );
        let darker = shadowed
            .chunks_exact(4)
            .zip(open.chunks_exact(4))
            .filter(|(s, o)| u16::from(s[0]) + 12 < u16::from(o[0]))
            .count();
        assert!(darker > 40, "{preset:?}: only {darker} pixels are shadowed");
    }
}

fn water_scene() -> Scene {
    let mut scene = Scene {
        environment: overworld(45., 225.),
        ..transparent_atlas()
    };
    let mut geometry = crate::api::MeshGeometry::default();
    quad(
        &mut geometry,
        0,
        [
            [-6., -3., 1.],
            [-6., -3., 12.],
            [6., -3., 12.],
            [6., -3., 1.],
        ],
        [0.7, 0.6, 0.4],
        [0., 1., 0.],
        1.,
    );
    // Lake bed raised on one side so the water column varies.
    quad(
        &mut geometry,
        0,
        [
            [-6., -1., 6.],
            [-6., -1., 12.],
            [0., -1., 12.],
            [0., -1., 6.],
        ],
        [0.55, 0.5, 0.35],
        [0., 1., 0.],
        1.,
    );
    quad(
        &mut geometry,
        2,
        [[-6., 0., 1.], [-6., 0., 12.], [6., 0., 12.], [6., 0., 1.]],
        [0.15, 0.4, 0.6],
        [0., 4., 0.],
        0.7,
    );
    chunk(
        &mut scene,
        [0; 3],
        [0.; 3],
        [-6., -3., 1.],
        [6., 0., 12.],
        geometry,
    );
    scene.camera_position = [0., 2., 0.];
    scene.pitch = 18.;
    scene
}

#[test]
fn high_water_refracts_reflects_and_differs_from_medium() {
    let mut renderer = offscreen("water");
    let scene = water_scene();
    renderer.set_graphics_preset(GraphicsPreset::Medium);
    let medium = pixels(&mut renderer, &scene);
    renderer.set_graphics_preset(GraphicsPreset::High);
    let high = pixels(&mut renderer, &scene);
    assert!(
        renderer.shadow.scene_copy.width() == 64,
        "High keeps a full-size opaque scene copy"
    );
    assert!(
        mean_abs_diff(&medium, &high) > 1.,
        "High water must change the image ({})",
        mean_abs_diff(&medium, &high)
    );
    assert!(high.chunks_exact(4).all(|p| p[3] == 255), "opaque output");
    // The left half of the lake (x < 0) has its bed raised to 1 block deep; the
    // right half is 3 blocks deep. Row 26 looks at z ~ 10, inside both beds.
    let shallow = rgb(&high, (26 * 64 + 14) * 4);
    let deep = rgb(&high, (26 * 64 + 50) * 4);
    assert!(
        (shallow[0] - deep[0]).abs() + (shallow[1] - deep[1]).abs() > 3.,
        "water depth must change the tint: {shallow:?} vs {deep:?}"
    );
}

#[test]
fn jitter_stays_under_half_a_pixel_and_covers_the_pixel() {
    let samples: Vec<[f32; 2]> = (0..8).map(jitter_pixels).collect();
    assert!(
        samples.iter().all(|j| j[0].abs() < 0.5 && j[1].abs() < 0.5),
        "{samples:?}"
    );
    let mean = samples
        .iter()
        .fold([0.; 2], |a, j| [a[0] + j[0], a[1] + j[1]]);
    assert!(
        mean[0].abs() < 0.5 && mean[1].abs() < 0.5,
        "jitter must be centred: {mean:?}"
    );
    for (i, a) in samples.iter().enumerate() {
        for b in &samples[i + 1..] {
            assert_ne!(a, b, "jitter samples must be distinct");
        }
    }
    assert_eq!(
        jitter_pixels(3),
        jitter_pixels(11),
        "sequence repeats every 8 frames"
    );
}

#[test]
fn taa_converges_on_static_scenes_and_reprojects_camera_motion() {
    let mut renderer = offscreen("taa");
    renderer.set_graphics_preset(GraphicsPreset::High);
    assert!(renderer.post.taa());
    let mut scene = Scene {
        environment: overworld(45., 225.),
        ..transparent_atlas()
    };
    lit_ground_and_caster(&mut scene);
    let mut previous = pixels(&mut renderer, &scene);
    let mut latest = previous.clone();
    for _ in 0..10 {
        latest = pixels(&mut renderer, &scene);
    }
    assert!(renderer.temporal.history_valid);
    let settled = mean_abs_diff(&previous, &latest);
    previous = latest.clone();
    latest = pixels(&mut renderer, &scene);
    assert!(
        mean_abs_diff(&previous, &latest) < 1.5,
        "a static scene must be temporally stable (first diff {settled}, last {})",
        mean_abs_diff(&previous, &latest)
    );
    // Move the camera: the resolved frame must track a history-free render of
    // the new pose (reprojection, not smearing).
    scene.camera_position[0] += 0.4;
    let moved = pixels(&mut renderer, &scene);
    renderer.temporal.history_valid = false;
    let fresh = pixels(&mut renderer, &scene);
    assert!(
        mean_abs_diff(&moved, &fresh) < 8.,
        "TAA after motion must match a fresh frame ({})",
        mean_abs_diff(&moved, &fresh)
    );
}

#[test]
fn switching_presets_at_runtime_keeps_rendering_and_low_returns_to_vanilla_pixels() {
    let mut renderer = offscreen("switch");
    let mut scene = water_scene();
    renderer.set_graphics_preset(GraphicsPreset::Low);
    let low = pixels(&mut renderer, &scene);
    for preset in [
        GraphicsPreset::High,
        GraphicsPreset::Medium,
        GraphicsPreset::High,
        GraphicsPreset::Low,
    ] {
        renderer.set_graphics_preset(preset);
        let frame = pixels(&mut renderer, &scene);
        assert!(
            frame
                .chunks_exact(4)
                .any(|p| p[0] != 0 || p[1] != 0 || p[2] != 0),
            "{preset:?}"
        );
        assert_eq!(
            renderer.shadow.layers.len() as u32,
            preset.features().cascades.max(1)
        );
        assert_eq!(renderer.post.taa(), preset.features().taa);
    }
    assert_eq!(
        pixels(&mut renderer, &scene),
        low,
        "Low must return to identical vanilla pixels"
    );
    // Resize reallocates TAA history, cascade array and scene copy together.
    renderer.set_graphics_preset(GraphicsPreset::High);
    renderer.resize(PhysicalSize::new(96, 80));
    assert_eq!(renderer.shadow.opaque_depth.width(), 96);
    assert_eq!(renderer.shadow.scene_copy.height(), 80);
    assert_eq!(renderer.shadow.layers.len(), 3);
    scene.revision += 1;
    renderer.stats = FrameStats::default();
    renderer
        .encode_scene(&scene, &super::shader_tests::observation(), None)
        .unwrap();
}
