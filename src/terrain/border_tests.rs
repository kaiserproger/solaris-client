//! Chunk/section border face-culling and greedy-merge regression fixtures.
//!
//! Fixtures reuse the repo-owned [`Content::test_cube`] baked cube (no Mojang
//! bytes) with a real world snapshot so culling reads cross real chunk
//! handles. Rules are deterministic: flat directional shade, AO off,
//! blend radius 0, full-bright constant light, tint-free quads. An isolated
//! full cube is unaffected by greedy merging, so its six-face counts hold both
//! before and after the parent's mesher edits.

use super::catalog::Catalog;
use super::mesher::{self, BuiltSection, Rules, NORMALS};
use crate::api::{MeshGeometry, RenderLayer};
use crate::protocol::content::{Content, TextureRegion};
use crate::protocol::world::{
    empty_block_entities, Chunk, Palette, Section, SurfaceHeightmap, World,
};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{atomic::AtomicU64, Arc};

const AIR: u32 = 0;
const CUBE: u32 = 1;
const BYTE_LIMIT: usize = 8 * 1024 * 1024;

fn uniform(size: usize, value: u32) -> Palette {
    Palette {
        bits: 0,
        values: vec![value],
        words: Vec::new(),
        size,
    }
}

fn full_light_layer() -> Arc<Vec<u8>> {
    Arc::new(vec![0xFF; 2048])
}

fn block_index(p: [i32; 3]) -> usize {
    (p[1].rem_euclid(16) * 256 + p[2].rem_euclid(16) * 16 + p[0].rem_euclid(16)) as usize
}

/// Builds a fully-lit chunk holding `cells` (world-space positions),
/// distributed across `section_count` sections of a `height`-tall dimension.
fn lit_chunk(cells: &[[i32; 3]], section_count: usize, height: i32) -> Chunk {
    let mut sections = Vec::with_capacity(section_count);
    for section in 0..section_count {
        let mut blocks = uniform(4096, AIR);
        let mut count = 0u16;
        for &p in cells {
            if p[1].div_euclid(16) == section as i32 {
                blocks.set(block_index(p), CUBE);
                count += 1;
            }
        }
        sections.push(Arc::new(Section {
            blocks,
            biomes: uniform(64, 0),
            non_empty_block_count: count,
        }));
    }
    // Layers cover one below and one above the dimension so border faces and
    // their front-cell light samples stay known constants everywhere.
    let mut sky = BTreeMap::new();
    let mut block = BTreeMap::new();
    for layer in 0..section_count + 2 {
        sky.insert(layer, full_light_layer());
        block.insert(layer, full_light_layer());
    }
    Chunk {
        sections,
        sky,
        block,
        block_entities: empty_block_entities(),
        surface_heightmap: Arc::new(SurfaceHeightmap::new(height).unwrap()),
        surface_heightmap_received: false,
    }
}

/// Repo-owned cube content plus the minimum received facts
/// [`Catalog::capture`] requires: fluid sprites, one biome holder, and fluid
/// overlay metadata for both states. Each model face gets its face index as a
/// texture-layer sentinel so greedy merges stay attributable.
fn fixture_content() -> Content {
    let mut content = Content::test_cube();
    for quad in &mut content.models[0].quads {
        quad.texture_layer = u32::from(quad.face);
    }
    for state in [AIR, CUBE] {
        content.insert_test_placement_metadata(state, serde_json::json!({"fluid_overlay": false}));
    }
    for name in [
        "minecraft:block/water_still",
        "minecraft:block/water_flow",
        "minecraft:block/water_overlay",
        "minecraft:block/lava_still",
        "minecraft:block/lava_flow",
    ] {
        content.insert_test_texture(
            name,
            TextureRegion {
                uv: [0., 0., 1., 1.],
                layer: 0,
                width: 16,
                height: 16,
            },
        );
    }
    content.set_registries(BTreeMap::from([(
        "minecraft:worldgen/biome".to_owned(),
        vec![(
            "minecraft:plains".to_owned(),
            serde_json::json!({
                "temperature": 0.5,
                "downfall": 0.5,
                "effects": {"water_color": "#3f76e4"},
                "attributes": {},
            }),
        )],
    )]));
    content
}

fn deterministic_rules() -> Rules {
    Rules {
        directional_shade: [1.; 6],
        ambient_occlusion: false,
        biome_blend_radius: 0,
        fade_duration_ms: None,
    }
}

fn build(world: &World, center: [i32; 2], key: [i32; 3]) -> BuiltSection {
    let content = fixture_content();
    let catalog = Catalog::capture(&content).expect("fixture catalog must capture");
    let neighborhood = world.snapshot(center);
    let cancellation = AtomicU64::new(0);
    mesher::build(
        &neighborhood,
        &catalog,
        key,
        deterministic_rules(),
        &cancellation,
        0,
        BYTE_LIMIT,
    )
    .expect("fixture build must succeed")
    .expect("center section must be present")
}

fn assert_deterministic(section: &BuiltSection) {
    assert_eq!(
        section.unknown_light_samples, 0,
        "fixture light must be fully known"
    );
    assert_eq!(
        section.unknown_biome_samples, 0,
        "tint-free fixture must not sample biomes"
    );
}

/// (vertex count, opaque index count). The cube fixture only emits opaque.
fn opaque_counts(section: &BuiltSection) -> (usize, usize) {
    let geometry = &section.geometry;
    assert!(
        geometry.indices[RenderLayer::Cutout as usize].is_empty(),
        "cube fixture must not emit cutout"
    );
    assert!(
        geometry.indices[RenderLayer::Translucent as usize].is_empty(),
        "cube fixture must not emit translucent"
    );
    (
        geometry.vertices.len(),
        geometry.indices[RenderLayer::Opaque as usize].len(),
    )
}

fn assert_constant_light(section: &BuiltSection) {
    for vertex in &section.geometry.vertices {
        assert_eq!(
            vertex.light,
            [15., 15.],
            "fixture light must be constant full-bright"
        );
    }
}

fn round_coord(v: f32) -> i32 {
    v.round() as i32
}

/// Expands emitted quads (merged or not) back to covered world-space unit
/// faces so merged output can be compared against the reference exposed-face
/// set. Quads are recovered from index groups, so shared vertices across
/// merges stay valid. Mesh vertices are section-local, so `base` (the
/// section key times 16) shifts them back to world cells.
fn unit_faces(geometry: &MeshGeometry, base: [i32; 3]) -> BTreeSet<(u8, [i32; 3])> {
    let indices = &geometry.indices[RenderLayer::Opaque as usize];
    assert_eq!(indices.len() % 6, 0, "opaque indices must come in quads");
    let mut out = BTreeSet::new();
    for group in indices.chunks_exact(6) {
        let mut ids = group.to_vec();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 4, "one index group must reference one quad");
        let verts: Vec<_> = ids
            .iter()
            .map(|id| &geometry.vertices[*id as usize])
            .collect();
        for vertex in &verts {
            assert_eq!(
                vertex.normal, verts[0].normal,
                "one quad must share one face normal"
            );
        }
        let normal = verts[0].normal.map(round_coord);
        let face = NORMALS
            .iter()
            .position(|n| *n == normal)
            .expect("quad normal must be cardinal") as u8;
        let axis = normal.iter().position(|v| *v != 0).unwrap();
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for vertex in &verts {
            for a in 0..3 {
                min[a] = min[a].min(vertex.position[a]);
                max[a] = max[a].max(vertex.position[a]);
            }
        }
        assert!(
            (max[axis] - min[axis]).abs() < 0.001,
            "quad must be planar on its normal axis"
        );
        let plane = round_coord(min[axis]);
        let tangent: Vec<usize> = (0..3).filter(|a| *a != axis).collect();
        let (t0, t1) = (tangent[0], tangent[1]);
        let (u0, u1) = (round_coord(min[t0]), round_coord(max[t0]));
        let (v0, v1) = (round_coord(min[t1]), round_coord(max[t1]));
        assert!(u1 > u0 && v1 > v0, "quad must cover a unit square");
        for u in u0..u1 {
            for v in v0..v1 {
                let mut block = [0; 3];
                block[axis] = if normal[axis] > 0 { plane - 1 } else { plane };
                block[t0] = u;
                block[t1] = v;
                for a in 0..3 {
                    block[a] += base[a];
                }
                out.insert((face, block));
            }
        }
    }
    out
}

/// Reference exposed unit faces of full cubes: every face whose neighbor cell
/// is not one of the cubes.
fn exposed_faces(cubes: &[[i32; 3]]) -> BTreeSet<(u8, [i32; 3])> {
    let occupied: BTreeSet<[i32; 3]> = cubes.iter().copied().collect();
    let mut out = BTreeSet::new();
    for &block in cubes {
        for (face, normal) in NORMALS.iter().enumerate() {
            let neighbor = [
                block[0] + normal[0],
                block[1] + normal[1],
                block[2] + normal[2],
            ];
            if !occupied.contains(&neighbor) {
                out.insert((face as u8, block));
            }
        }
    }
    out
}

fn assert_texture_sentinels(geometry: &MeshGeometry) {
    for vertex in &geometry.vertices {
        let normal = vertex.normal.map(round_coord);
        let face = NORMALS
            .iter()
            .position(|n| *n == normal)
            .expect("vertex normal must be cardinal") as u8;
        assert_eq!(
            vertex.texture_layer,
            u32::from(face),
            "merges must preserve the per-face texture sentinel"
        );
        assert!(
            vertex.uv.iter().all(|c| (-1.0..=2.0).contains(c)),
            "merged tiling must stay within the 2-wide facet range"
        );
        assert!(
            vertex.uv.iter().all(|c| c.is_finite()),
            "uv must stay finite through merges"
        );
    }
}

#[test]
fn isolated_opaque_cube_emits_six_faces() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[8, 8, 8]], 1, 16))
        .unwrap();
    let section = build(&world, [0, 0], [0, 0, 0]);
    assert_deterministic(&section);
    assert_constant_light(&section);
    // A lone full cube has no coplanar neighbor to merge with, so greedy
    // leaves it alone: six quads hold before and after the parent's edits.
    assert_eq!(opaque_counts(&section), (24, 36));
    assert_eq!(
        unit_faces(&section.geometry, [0, 0, 0]),
        exposed_faces(&[[8, 8, 8]])
    );
}

#[test]
fn positive_chunk_border_hides_shared_face() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[15, 8, 8]], 1, 16))
        .unwrap();
    world
        .insert([1, 0], lit_chunk(&[[16, 8, 8]], 1, 16))
        .unwrap();
    let section = build(&world, [0, 0], [0, 0, 0]);
    assert_deterministic(&section);
    assert_eq!(opaque_counts(&section), (20, 30));
    let faces = unit_faces(&section.geometry, [0, 0, 0]);
    assert!(
        !faces.contains(&(5, [15, 8, 8])),
        "shared east face at x=15/16 must be hidden"
    );
    assert_eq!(faces.len(), 5);
}

#[test]
fn negative_chunk_border_hides_shared_face() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[0, 8, 8]], 1, 16))
        .unwrap();
    world
        .insert([-1, 0], lit_chunk(&[[-1, 8, 8]], 1, 16))
        .unwrap();
    let section = build(&world, [0, 0], [0, 0, 0]);
    assert_deterministic(&section);
    assert_eq!(opaque_counts(&section), (20, 30));
    let faces = unit_faces(&section.geometry, [0, 0, 0]);
    assert!(
        !faces.contains(&(4, [0, 8, 8])),
        "shared west face at x=0/-1 must be hidden"
    );
    assert_eq!(faces.len(), 5);
}

#[test]
fn section_border_hides_shared_face() {
    let mut world = World::new(0, 32);
    world
        .insert([0, 0], lit_chunk(&[[8, 15, 8], [8, 16, 8]], 2, 32))
        .unwrap();
    let lower = build(&world, [0, 0], [0, 0, 0]);
    assert_deterministic(&lower);
    assert_eq!(opaque_counts(&lower), (20, 30));
    assert!(
        !unit_faces(&lower.geometry, [0, 0, 0]).contains(&(1, [8, 15, 8])),
        "shared up face at y=15/16 must be hidden"
    );
    let upper = build(&world, [0, 0], [0, 1, 0]);
    assert_deterministic(&upper);
    assert_eq!(opaque_counts(&upper), (20, 30));
    assert!(
        !unit_faces(&upper.geometry, [0, 16, 0]).contains(&(0, [8, 16, 8])),
        "shared down face at y=16/15 must be hidden"
    );
}

#[test]
fn unloaded_neighbor_keeps_border_face_visible() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[15, 8, 8]], 1, 16))
        .unwrap();
    // No chunk at [1, 0]: missing columns read as air, never as occluders.
    // Their light is genuinely unknown, so only the biome side stays exact.
    let section = build(&world, [0, 0], [0, 0, 0]);
    assert_eq!(
        section.unknown_biome_samples, 0,
        "tint-free fixture must not sample biomes"
    );
    assert!(
        section.unknown_light_samples > 0,
        "unloaded columns must report unknown light, not invented light"
    );
    assert_eq!(
        opaque_counts(&section),
        (24, 36),
        "unloaded neighbor must not cull the border face"
    );
    assert_eq!(
        unit_faces(&section.geometry, [0, 0, 0]),
        exposed_faces(&[[15, 8, 8]])
    );
}

#[test]
fn two_adjacent_cubes_merge_to_six_quads() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[8, 8, 8], [9, 8, 8]], 1, 16))
        .unwrap();
    let section = build(&world, [0, 0], [0, 0, 0]);
    assert_deterministic(&section);
    assert_constant_light(&section);
    // Failing-before baseline without greedy: 10 quads / 40 verts / 60
    // indices. After greedy the shared side walls merge into single quads:
    // six quads / 24 verts / 36 indices.
    assert_eq!(opaque_counts(&section), (24, 36));
    assert_eq!(
        unit_faces(&section.geometry, [0, 0, 0]),
        exposed_faces(&[[8, 8, 8], [9, 8, 8]]),
        "merged quads must expand to the ten exposed unit squares"
    );
    assert_texture_sentinels(&section.geometry);
}

#[test]
fn server_chunks_outside_client_distance_do_not_enter_mesh_queue() {
    let mut world = World::new(0, 16);
    for x in 0..=16 {
        world
            .insert([x, 0], lit_chunk(&[[x * 16 + 8, 8, 8]], 1, 16))
            .unwrap();
    }
    let mut terrain = super::Terrain::new(Arc::new(|| {})).unwrap();
    terrain.activate(&fixture_content(), [1; 3]).unwrap();
    terrain
        .set_render_rules([1.; 6], false, 0, Some(0))
        .unwrap();
    for x in 0..=16 {
        terrain.invalidate_chunk(&world, [x, 0]).unwrap();
    }
    let mut draws = BTreeMap::new();
    terrain.update(&world, [8., 8., 8.], 8, &mut draws).unwrap();
    assert_eq!(terrain.sections.len(), 9);
    assert!(terrain.sections.contains_key(&[8, 0, 0]));
    assert!(
        terrain.sections.keys().all(|key| key[0] <= 8),
        "server view16 must not enqueue meshes beyond client view8"
    );
}

#[test]
fn mesh_view_growth_and_movement_schedule_cached_columns() {
    let mut world = World::new(0, 16);
    for x in [-1, 0, 1, 3] {
        world
            .insert([x, 0], lit_chunk(&[[x * 16 + 8, 8, 8]], 1, 16))
            .unwrap();
    }
    let mut terrain = super::Terrain::new(Arc::new(|| {})).unwrap();
    let mut draws = BTreeMap::new();
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    assert_eq!(terrain.sections.len(), 3);
    assert!(!terrain.sections.contains_key(&[3, 0, 0]));
    terrain.invalidate_chunk(&world, [3, 0]).unwrap();
    assert!(!terrain.sections.contains_key(&[3, 0, 0]));

    terrain
        .sync_view(&world, [8., 8., 8.], 3, &mut draws)
        .unwrap();
    assert!(terrain.sections[&[3, 0, 0]].pending);
    terrain
        .sync_view(&world, [56., 8., 8.], 1, &mut draws)
        .unwrap();
    assert_eq!(terrain.sections.len(), 1);
    assert!(terrain.sections.contains_key(&[3, 0, 0]));
    assert!(
        world.chunk([0, 0]).is_some(),
        "presentation retirement must preserve received world state"
    );

    terrain
        .sync_view(&world, [-8., 8., 8.], 1, &mut draws)
        .unwrap();
    assert!(terrain.sections.contains_key(&[-1, 0, 0]));
    assert!(terrain.sections.contains_key(&[0, 0, 0]));
    assert!(!terrain.sections.contains_key(&[1, 0, 0]));
}

#[test]
fn mesh_view_retires_geometry_bytes_and_changes_reentered_revision() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[8, 8, 8]], 1, 16))
        .unwrap();
    let mut terrain = super::Terrain::new(Arc::new(|| {})).unwrap();
    let mut draws = BTreeMap::new();
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    let built = build(&world, [0, 0], [0, 0, 0]);
    let bytes = mesher::geometry_bytes(&built.geometry);
    let revision = terrain.sections[&[0, 0, 0]].revision;
    terrain.sections.get_mut(&[0, 0, 0]).unwrap().bytes = bytes;
    terrain.resident_bytes = bytes;
    draws.insert(
        [0, 0, 0],
        crate::api::ChunkDraw {
            revision,
            origin: [0.; 3],
            bounds_min: built.bounds_min,
            bounds_max: built.bounds_max,
            visibility: built.visibility,
            geometry: Some(Arc::new(built.geometry)),
            fade_duration_ms: Some(0),
        },
    );
    assert!(terrain
        .sync_view(&world, [56., 8., 8.], 1, &mut draws)
        .unwrap());
    assert!(draws.is_empty());
    assert_eq!(terrain.resident_bytes, 0);
    assert!(terrain.sections.is_empty());
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    assert!(
        terrain.sections[&[0, 0, 0]].revision > revision,
        "an old in-flight completion must not match a reentered section"
    );
}

#[test]
fn release_uploaded_frees_pending_bytes_and_keeps_metadata() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[8, 8, 8]], 1, 16))
        .unwrap();
    let mut terrain = super::Terrain::new(Arc::new(|| {})).unwrap();
    let mut draws = BTreeMap::new();
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    let built = build(&world, [0, 0], [0, 0, 0]);
    let bytes = mesher::geometry_bytes(&built.geometry);
    let revision = terrain.sections[&[0, 0, 0]].revision;
    let (bounds_min, bounds_max, visibility) =
        (built.bounds_min, built.bounds_max, built.visibility);
    terrain.sections.get_mut(&[0, 0, 0]).unwrap().bytes = bytes;
    terrain.sections.get_mut(&[0, 0, 0]).unwrap().pending = false;
    terrain
        .sections
        .get_mut(&[0, 0, 0])
        .unwrap()
        .unknown_light_samples = 7;
    terrain.resident_bytes = bytes;
    draws.insert(
        [0, 0, 0],
        crate::api::ChunkDraw {
            revision,
            origin: [0.; 3],
            bounds_min,
            bounds_max,
            visibility,
            geometry: Some(Arc::new(built.geometry)),
            fade_duration_ms: Some(0),
        },
    );
    terrain.release_uploaded(&mut draws, [([0, 0, 0], revision)]);
    let draw = &draws[&[0, 0, 0]];
    assert!(
        draw.geometry.is_none(),
        "uploaded CPU payload must be released"
    );
    assert_eq!(draw.revision, revision);
    assert_eq!(
        (draw.bounds_min, draw.bounds_max, draw.visibility),
        (bounds_min, bounds_max, visibility)
    );
    assert_eq!(terrain.sections[&[0, 0, 0]].bytes, 0);
    assert_eq!(terrain.resident_bytes, 0);
    assert_eq!(terrain.sections[&[0, 0, 0]].unknown_light_samples, 7);
    assert!(!terrain.sections[&[0, 0, 0]].pending);
    // A repeated ack for the same revision must not double-free.
    terrain.release_uploaded(&mut draws, [([0, 0, 0], revision)]);
    assert_eq!(terrain.resident_bytes, 0);
}

#[test]
fn stale_ack_preserves_replacement_payload() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[8, 8, 8]], 1, 16))
        .unwrap();
    let mut terrain = super::Terrain::new(Arc::new(|| {})).unwrap();
    let mut draws = BTreeMap::new();
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    let built = build(&world, [0, 0], [0, 0, 0]);
    let bytes = mesher::geometry_bytes(&built.geometry);
    let rev1 = terrain.sections[&[0, 0, 0]].revision;
    terrain.sections.get_mut(&[0, 0, 0]).unwrap().bytes = bytes;
    terrain.resident_bytes = bytes;
    draws.insert(
        [0, 0, 0],
        crate::api::ChunkDraw {
            revision: rev1,
            origin: [0.; 3],
            bounds_min: built.bounds_min,
            bounds_max: built.bounds_max,
            visibility: built.visibility,
            geometry: Some(Arc::new(built.geometry)),
            fade_duration_ms: Some(0),
        },
    );
    // An edit advances the requested revision while the old payload is still
    // resident: the old ack must not free anything.
    let section = terrain.sections.get_mut(&[0, 0, 0]).unwrap();
    section.revision += 1;
    section.pending = true;
    let rev2 = terrain.sections[&[0, 0, 0]].revision;
    terrain.release_uploaded(&mut draws, [([0, 0, 0], rev1)]);
    assert!(
        draws[&[0, 0, 0]].geometry.is_some(),
        "ack for a superseded request must not free the pending payload"
    );
    assert_eq!(terrain.resident_bytes, bytes);
    // The remesh commits the replacement; the old ack is now stale.
    let rebuilt = build(&world, [0, 0], [0, 0, 0]);
    let bytes2 = mesher::geometry_bytes(&rebuilt.geometry);
    let draw = draws.get_mut(&[0, 0, 0]).unwrap();
    draw.revision = rev2;
    draw.geometry = Some(Arc::new(rebuilt.geometry));
    let section = terrain.sections.get_mut(&[0, 0, 0]).unwrap();
    terrain.resident_bytes = terrain.resident_bytes - section.bytes + bytes2;
    section.bytes = bytes2;
    section.pending = false;
    terrain.release_uploaded(&mut draws, [([0, 0, 0], rev1)]);
    assert!(
        draws[&[0, 0, 0]].geometry.is_some(),
        "stale ack must not free the replacement payload"
    );
    assert_eq!(terrain.sections[&[0, 0, 0]].bytes, bytes2);
    // Unknown keys are harmless.
    terrain.release_uploaded(&mut draws, [([9, 9, 9], rev1)]);
    assert_eq!(terrain.resident_bytes, bytes2);
    // The exact-revision ack releases.
    terrain.release_uploaded(&mut draws, [([0, 0, 0], rev2)]);
    assert!(draws[&[0, 0, 0]].geometry.is_none());
    assert_eq!(terrain.resident_bytes, 0);
}

#[test]
fn idle_sync_skips_rescan_and_direct_lifecycle_change_evicts() {
    let mut world = World::new(0, 16);
    world
        .insert([0, 0], lit_chunk(&[[8, 8, 8]], 1, 16))
        .unwrap();
    let mut terrain = super::Terrain::new(Arc::new(|| {})).unwrap();
    terrain.activate(&fixture_content(), [1; 3]).unwrap();
    terrain
        .set_render_rules([1.; 6], false, 0, Some(0))
        .unwrap();
    let mut draws = BTreeMap::new();
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    assert!(terrain.has_pending, "growth must schedule the first mesh");
    assert!(
        !terrain
            .sync_view(&world, [8., 8., 8.], 1, &mut draws)
            .unwrap(),
        "idle same-view sync must change nothing"
    );
    terrain.update(&world, [8., 8., 8.], 1, &mut draws).unwrap();
    assert!(
        !terrain.has_pending,
        "dispatched-or-committed sections must not hide behind a stale flag"
    );
    // Direct world unload without any Terrain notification must still evict.
    world.remove([0, 0]);
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    assert!(terrain.sections.is_empty());
    assert!(draws.is_empty());
    // Reentry schedules a remesh and syncs the lifecycle cache.
    world
        .insert([0, 0], lit_chunk(&[[8, 8, 8]], 1, 16))
        .unwrap();
    terrain
        .sync_view(&world, [8., 8., 8.], 1, &mut draws)
        .unwrap();
    assert!(
        terrain.sections[&[0, 0, 0]].pending,
        "reentry must schedule a remesh"
    );
    assert!(terrain.has_pending);
    assert_eq!(
        terrain.view_lifecycle,
        world.chunk_lifecycle_revision(),
        "sync must cache the observed lifecycle revision"
    );
    assert!(
        !terrain
            .sync_view(&world, [8., 8., 8.], 1, &mut draws)
            .unwrap(),
        "re-cached same-view sync must skip again"
    );
}
