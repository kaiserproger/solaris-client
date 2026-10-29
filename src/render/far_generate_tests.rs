use super::*;
use crate::render::Renderer;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The compute pass against the CPU reference (itself bit-exact with the
/// server, `far/program_tests.rs`) on tiles near and 8 km from the origin, in
/// every quadrant and at fine and coarse levels. The GPU works in f32, so
/// heights may differ by a block or two where the density crosses zero
/// slowly; the test reports and bounds that.
#[test]
fn gpu_columns_match_the_cpu_reference() {
    let _gpu_guard = crate::render::gpu_test_lock();
    let renderer = match pollster::block_on(Renderer::new_offscreen(
        32,
        32,
        PathBuf::from(".analysis/clientcpu-regression"),
    )) {
        Ok(renderer) => renderer,
        Err(error) => {
            eprintln!("SKIP: offscreen GPU unavailable: {error}");
            return;
        }
    };
    if !renderer.far.generator.supported {
        eprintln!("SKIP: no compute shaders on this adapter");
        return;
    }
    let (bytes, _) = crate::far::program::tests::fixture();
    let program = Arc::new(Program::decode(&bytes).unwrap());
    let keys = [
        TileKey {
            level: 1,
            x: 0,
            z: 0,
        },
        TileKey {
            level: 2,
            x: -3,
            z: 5,
        },
        TileKey {
            level: 4,
            x: 7,
            z: -2,
        },
        TileKey {
            level: 6,
            x: -8,
            z: -8,
        },
        TileKey {
            level: 6,
            x: 7,
            z: 7,
        },
        TileKey {
            level: 5,
            x: -15,
            z: 12,
        },
    ];
    let work = FarGeneration {
        program: Some(Arc::clone(&program)),
        tiles: keys.to_vec(),
        revision: 1,
    };
    let timing = Timing::new(&renderer.device, &renderer.queue, false);
    let mut generator = FarGenerator::new(true);
    let mut gpu = HashMap::new();
    let started = Instant::now();
    let mut dispatch_ms = Vec::new();
    while gpu.len() < keys.len() {
        assert!(
            started.elapsed() < Duration::from_secs(300),
            "GPU generation stalled"
        );
        let mut encoder = renderer.device.create_command_encoder(&Default::default());
        generator.encode(
            &renderer.device,
            &renderer.queue,
            &mut encoder,
            &work,
            &timing,
        );
        assert_eq!(generator.error, None);
        let dispatched = Instant::now();
        let submission = renderer.queue.submit([encoder.finish()]);
        generator.submitted();
        renderer
            .device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: Some(Duration::from_secs(60)),
            })
            .unwrap();
        dispatch_ms.push(dispatched.elapsed().as_secs_f64() * 1000.);
        gpu.extend(generator.drain());
    }
    let mut regs = Vec::new();
    let (mut total, mut exact, mut within_one, mut within_two, mut biomes) = (0, 0, 0, 0, 0);
    let mut worst = 0;
    let cpu_started = Instant::now();
    for key in keys {
        let origin = key.origin();
        let spacing = key.spacing();
        for (index, column) in gpu[&key].iter().enumerate() {
            let x = (origin[0] + (index % 16) as i64 * spacing) as i32;
            let z = (origin[1] + (index / 16) as i64 * spacing) as i32;
            let cpu = program.column(x, z, &mut regs);
            let diff = (cpu.height - column.height).abs();
            worst = worst.max(diff);
            total += 1;
            exact += usize::from(diff == 0);
            within_one += usize::from(diff <= 1);
            within_two += usize::from(diff <= 2);
            biomes += usize::from(cpu.biome == column.biome);
        }
    }
    let cpu_ms_per_tile = cpu_started.elapsed().as_secs_f64() * 1000. / keys.len() as f64;
    eprintln!(
        "far gpu parity: {total} columns, height exact {exact}, <=1 {within_one}, <=2 \
         {within_two}, worst {worst}; biome equal {biomes}; wall ms per dispatch (1 tile, \
         includes readback) {dispatch_ms:?}; cpu ms per tile {cpu_ms_per_tile:.1}"
    );
    assert!(
        within_two * 100 >= total * 95,
        "{within_two}/{total} within 2 blocks"
    );
    assert!(biomes * 100 >= total * 95, "{biomes}/{total} biomes equal");
}
