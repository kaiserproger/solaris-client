use super::weather::append;
use crate::{api::{DynamicGeometry, EntityMaterial}, protocol::{content::{Content, TextureRegion}, world::{World, Chunk, Section, Palette, SurfaceHeightmap, empty_block_entities}}};
use std::{collections::BTreeMap, sync::Arc};
use serde_json::json;

fn fixture(temperature: f32) -> (World, Content) {
    let mut content = Content::test_cube();
    for (state, blocks_motion) in [(0, false), (1, true)] {
        content.insert_test_placement_state(state, json!({"is_solid":blocks_motion,"blocks_motion":blocks_motion,"liquid":false,"can_burn":false,"instrument":"harp","instrument_above":false,"static_support_cache":false}));
    }
    for (name, layer) in [("rain", 1), ("snow", 2)] {
        content.insert_test_texture(&format!("minecraft:environment/{name}"), TextureRegion { uv: [0., 0., 1., 1.], layer, width: 16, height: 16 });
    }
    content.set_registries(BTreeMap::from([("minecraft:worldgen/biome".into(), vec![("test:biome".into(), json!({"temperature":temperature,"has_precipitation":true}))])]));
    let uniform = |size| Palette { bits: 0, values: vec![0], words: Vec::new(), size };
    let mut world = World::new(0, 16);
    world.configure_environment_context("minecraft:overworld", 0, 63).unwrap();
    world.insert([0, 0], Chunk {
        sections: vec![Arc::new(Section { blocks: uniform(4096), biomes: uniform(64), non_empty_block_count: 0 })],
        sky: BTreeMap::new(), block: BTreeMap::new(), block_entities: empty_block_entities(),
        surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()), surface_heightmap_received: false,
    }).unwrap();
    (world, content)
}

#[test]
fn weather_mesh_respects_roofs_and_biome_temperature() {
    let (mut world, content) = fixture(0.8);
    let mut mesh = DynamicGeometry::default();
    let camera = [8.5, 8., 8.5];
    append(&world, &content, camera, [0.; 3], 1., 0., &mut mesh).unwrap();
    let rain_count = mesh.indices[EntityMaterial::Precipitation as usize].len();
    assert!(rain_count > 0);
    assert!(mesh.vertices.iter().all(|v| v.texture_layer == 1));
    world.set([8, 12, 8], 1, |state| Ok(state == 0)).unwrap();
    let full_alpha = mesh.vertices[0].alpha;
    mesh.vertices.clear();
    mesh.indices[EntityMaterial::Precipitation as usize].clear();
    append(&world, &content, camera, [0.; 3], 0.5, 0., &mut mesh).unwrap();
    assert!((mesh.vertices[0].alpha * 2. - full_alpha).abs() < 0.00001);
    assert_eq!(world.precipitation_height(8, 8, &content).unwrap(), Some(13));
    assert_eq!(world.precipitation_height(9, 8, &content).unwrap(), Some(0));
    assert_eq!(world.precipitation_height(32, 8, &content).unwrap(), None);
    mesh.vertices.clear();
    mesh.indices[EntityMaterial::Precipitation as usize].clear();
    append(&world, &content, camera, [0.; 3], 1., 0., &mut mesh).unwrap();
    assert!(mesh.indices[EntityMaterial::Precipitation as usize].len() < rain_count);
    let (world, content) = fixture(-0.5);
    mesh.vertices.clear();
    mesh.indices[EntityMaterial::Precipitation as usize].clear();
    append(&world, &content, camera, [0.; 3], 1., 0., &mut mesh).unwrap();
    assert!(!mesh.vertices.is_empty());
    assert!(mesh.vertices.iter().all(|v| v.texture_layer == 2));
    mesh.vertices.clear();
    mesh.indices[EntityMaterial::Precipitation as usize].clear();
    append(&world, &content, camera, [0.; 3], 0., 0., &mut mesh).unwrap();
    assert!(mesh.vertices.is_empty());
}

#[test]
fn weather_high_altitude_turns_marginal_biome_to_snow() {
    let (mut world, content) = fixture(0.2);
    assert_eq!(world.precipitation_is_snow([8, 8, 8], &content).unwrap(), Some(false));
    world.configure_environment_context("minecraft:overworld", 0, -100).unwrap();
    assert_eq!(world.precipitation_is_snow([8, 8, 8], &content).unwrap(), Some(true));
}
