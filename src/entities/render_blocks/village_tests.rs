//! Village block-entity regression tests: chest, bed and bell admit the real
//! static geometry pass with authored fixture rigs and textures.
//!
//! These tests route through `Store::append_static_block_geometry`, the same
//! entry the native scene uses for geometry-only factories, so a missing rig,
//! missing texture or scene-integration skip fails here instead of rendering
//! an invisible village. Quad counts are honest authored-fixture counts, not
//! vanilla asset bytes: they prove the rig path emits nonempty geometry with
//! the correct entity texture region, without depending on ignored Mojang
//! resources.
use super::super::resources::{Bone, BoneRole};
use super::*;
use std::{collections::BTreeMap, sync::Arc};

fn quad(x: f32, y: f32, z: f32, w: f32, h: f32) -> [[f32; 5]; 4] {
    [
        [x, y, z, 0., 0.],
        [x + w, y, z, 1., 0.],
        [x + w, y + h, z, 1., 1.],
        [x, y + h, z, 0., 1.],
    ]
}

fn bone(name: &str, pivot: [f32; 3], quads: Vec<[[f32; 5]; 4]>) -> Bone {
    Bone {
        parent: None,
        pivot: Vec3::from_array(pivot),
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role: BoneRole::Fixed,
        name: name.into(),
        normals: vec![[0., 0., 1.]; quads.len()],
        quads,
    }
}

fn models_with(rigs: Vec<(String, Rig)>) -> Models {
    Models {
        rigs: rigs.into_iter().collect(),
        schemas: BTreeMap::new(),
        families: BTreeMap::new(),
        builtin_variants: BTreeMap::new(),
        simple_particles: Vec::new(),
        default_skins: Vec::new(),
        poses: BTreeMap::new(),
        attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(),
        digest: String::new(),
        presentation: BTreeMap::new(),
        animations: BTreeMap::new(),
        motion: BTreeMap::new(),
    }
}

fn test_region(layer: u32) -> TextureRegion {
    TextureRegion {
        layer,
        uv: [0., 0., 1., 1.],
        width: 64,
        height: 64,
    }
}

/// Single-state fixture: `Content::test_cube` state 1 carries the village
/// block, its received properties and its constructor projection.
fn village_content(
    block: &str,
    entity: &str,
    props: BTreeMap<String, String>,
    textures: Vec<(&str, u32)>,
) -> Content {
    let mut content = Content::test_cube();
    content.names[1] = block.into();
    content.block_entity_names.insert(7, entity.into());
    content.insert_test_state_properties(1, props);
    content.insert_test_placement_metadata(1, serde_json::json!({"block_entity_type": entity}));
    for (name, layer) in textures {
        content.insert_test_texture(name, test_region(layer));
    }
    content
}

fn place(store: &mut super::super::Store, content: &Content, position: [i32; 3]) {
    store.ensure_block_entity(position, 1, content, 1).unwrap();
}

fn render_static(
    store: &mut super::super::Store,
    content: &Content,
    world: &World,
    origin: [f64; 3],
    camera: [f64; 3],
    partial: f32,
    out: &mut DynamicGeometry,
) -> super::super::PresentationStats {
    store
        .append_static_block_geometry(
            content,
            world,
            origin,
            camera,
            32.,
            false,
            false,
            partial,
            &|_| Ok(Some([7., 3.])),
            out,
        )
        .unwrap()
}

fn camera_for(position: [i32; 3]) -> ([f64; 3], [f64; 3]) {
    let origin = position.map(f64::from);
    let camera = [
        f64::from(position[0]) + 0.5,
        f64::from(position[1]) + 1.62,
        f64::from(position[2]) + 0.5,
    ];
    (origin, camera)
}

fn chest_rig() -> Rig {
    Rig {
        bones: vec![
            bone("body", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)]),
            bone("lid", [0., 1., 0.], vec![quad(0., 1., 0., 1., 1.)]),
            bone("lock", [0., 1., 0.], vec![quad(0.4, 1., 0., 0.2, 0.5)]),
        ],
        part_by_name: BTreeMap::new(),
    }
}

#[test]
fn village_chest_single_renders_body_lid_and_lock_with_chest_texture() {
    let content = village_content(
        "minecraft:chest",
        "minecraft:chest",
        BTreeMap::from([
            ("facing".into(), "north".into()),
            ("type".into(), "single".into()),
        ]),
        vec![("minecraft:entity/chest/normal", 3)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:chest#main".into(),
        chest_rig(),
    )])));
    let position = [258, 65, 135];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut geometry = DynamicGeometry::default();
    let stats = render_static(
        &mut store,
        &content,
        &world,
        origin,
        camera,
        0.,
        &mut geometry,
    );
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    // Three authored bones, one quad each: the real emit path must carry all
    // of them, not just claim a rendered chest.
    assert_eq!(geometry.vertices.len(), 12);
    assert_eq!(
        geometry.indices[EntityMaterial::EntityCutoutCull as usize].len(),
        18
    );
    assert_eq!(stats.vertices, 12);
    assert!(geometry.vertices.iter().all(|v| v.texture_layer == 3));
    assert!(geometry.vertices.iter().all(|v| v.light == [7., 3.]));
}

#[test]
fn village_chest_lid_opens_around_hinge_while_body_stays_put() {
    let content = village_content(
        "minecraft:chest",
        "minecraft:chest",
        BTreeMap::from([
            ("facing".into(), "north".into()),
            ("type".into(), "single".into()),
        ]),
        vec![("minecraft:entity/chest/normal", 3)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:chest#main".into(),
        chest_rig(),
    )])));
    let position = [258, 65, 135];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut closed = DynamicGeometry::default();
    render_static(
        &mut store,
        &content,
        &world,
        origin,
        camera,
        0.,
        &mut closed,
    );
    assert_eq!(closed.vertices.len(), 12);
    let entity = store.block_entities.get_mut(&position).unwrap();
    entity.animation.progress = 1.;
    entity.animation.previous = 1.;
    let mut open = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut open);
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    assert_eq!(open.vertices.len(), 12);
    // Bone emission order is body, lid, lock: the lid rotation must move the
    // lid/lock quads while the body quad stays exactly put.
    for (shut, swung) in closed.vertices.iter().zip(&open.vertices) {
        assert_eq!(shut.texture_layer, swung.texture_layer);
    }
    for (shut, swung) in closed.vertices[..4].iter().zip(&open.vertices[..4]) {
        assert_eq!(shut.position, swung.position);
    }
    let moved = open.vertices[4..]
        .iter()
        .zip(&closed.vertices[4..])
        .any(|(shut, swung)| {
            let d: [f32; 3] = std::array::from_fn(|i| shut.position[i] - swung.position[i]);
            d.iter().map(|v| v * v).sum::<f32>() > 1e-10
        });
    assert!(moved, "open lid must move lid/lock vertices");
}

#[test]
fn village_bed_foot_and_head_use_distinct_rigs_and_bed_texture() {
    for (part, key, quads) in [
        ("foot", "minecraft:bed_foot#main", 2usize),
        ("head", "minecraft:bed_head#main", 3usize),
    ] {
        let content = village_content(
            "minecraft:red_bed",
            "minecraft:bed",
            BTreeMap::from([
                ("facing".into(), "north".into()),
                ("part".into(), part.into()),
            ]),
            vec![("minecraft:entity/bed/red", 5)],
        );
        let rig = Rig {
            bones: vec![bone(
                "bed",
                [0., 0., 0.],
                (0..quads).map(|i| quad(i as f32, 0., 0., 1., 1.)).collect(),
            )],
            part_by_name: BTreeMap::new(),
        };
        let mut store = super::super::Store::new();
        store.activate_models(Arc::new(models_with(vec![(key.into(), rig)])));
        let position = [11, 64, -20];
        place(&mut store, &content, position);
        let (origin, camera) = camera_for(position);
        let world = World::new(0, 256);
        let mut geometry = DynamicGeometry::default();
        let stats = render_static(
            &mut store,
            &content,
            &world,
            origin,
            camera,
            0.,
            &mut geometry,
        );
        assert_eq!(stats.block_entities, 1, "bed {part} must render");
        assert!(
            stats.unrendered.is_empty(),
            "bed {part}: {:?}",
            stats.unrendered
        );
        // Distinct authored quad counts per half prove the foot/head rig key
        // selection actually ran instead of rendering one shared quad.
        assert_eq!(geometry.vertices.len(), quads * 4, "bed {part} quads");
        assert_eq!(
            geometry.indices[EntityMaterial::EntitySolid as usize].len(),
            quads * 6,
            "bed {part} indices"
        );
        assert_eq!(stats.vertices, quads * 4);
        assert!(geometry.vertices.iter().all(|v| v.texture_layer == 5));
        assert!(geometry.vertices.iter().all(|v| v.light == [7., 3.]));
    }
}

#[test]
fn village_bed_facing_rotates_rendered_geometry() {
    let mut content = village_content(
        "minecraft:red_bed",
        "minecraft:bed",
        BTreeMap::from([
            ("facing".into(), "north".into()),
            ("part".into(), "foot".into()),
        ]),
        vec![("minecraft:entity/bed/red", 5)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:bed_foot#main".into(),
        Rig {
            bones: vec![bone("bed", [0., 0., 0.], vec![quad(0., 0., 0., 1., 1.)])],
            part_by_name: BTreeMap::new(),
        },
    )])));
    let position = [11, 64, -20];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut north = DynamicGeometry::default();
    render_static(&mut store, &content, &world, origin, camera, 0., &mut north);
    assert_eq!(north.vertices.len(), 4);
    content.insert_test_state_properties(
        1,
        BTreeMap::from([
            ("facing".into(), "east".into()),
            ("part".into(), "foot".into()),
        ]),
    );
    let mut east = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut east);
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    assert_eq!(east.vertices.len(), 4);
    let moved = east.vertices.iter().zip(&north.vertices).any(|(a, b)| {
        let d: [f32; 3] = std::array::from_fn(|i| a.position[i] - b.position[i]);
        d.iter().map(|v| v * v).sum::<f32>() > 1e-10
    });
    assert!(moved, "east-facing bed must sit rotated from north");
}

#[test]
fn village_bell_body_renders_and_swing_moves_it() {
    let content = village_content(
        "minecraft:bell",
        "minecraft:bell",
        BTreeMap::new(),
        vec![("minecraft:entity/bell/bell_body", 7)],
    );
    let mut store = super::super::Store::new();
    store.activate_models(Arc::new(models_with(vec![(
        "minecraft:bell#main".into(),
        Rig {
            bones: vec![bone(
                "bell_body",
                [0., 2., 0.],
                vec![quad(0., 1., 0., 1., 1.), quad(0., 0., 0., 1., 1.)],
            )],
            part_by_name: BTreeMap::new(),
        },
    )])));
    let position = [13, 64, -20];
    place(&mut store, &content, position);
    let (origin, camera) = camera_for(position);
    let world = World::new(0, 256);
    let mut rest = DynamicGeometry::default();
    let stats = render_static(&mut store, &content, &world, origin, camera, 0., &mut rest);
    assert_eq!(stats.block_entities, 1);
    assert!(stats.unrendered.is_empty());
    assert_eq!(rest.vertices.len(), 8);
    assert_eq!(rest.indices[EntityMaterial::EntitySolid as usize].len(), 12);
    assert!(rest.vertices.iter().all(|v| v.texture_layer == 7));
    assert!(rest.vertices.iter().all(|v| v.light == [7., 3.]));
    let swing = super::math::bell_angles(10.5, 2);
    assert!(
        swing[0] != 0. || swing[1] != 0.,
        "fixture swing time must actually swing"
    );
    let entity = store.block_entities.get_mut(&position).unwrap();
    entity.animation.source.bell_direction = Some(2);
    entity.animation.source.bell_ticks = 10;
    let mut swung = DynamicGeometry::default();
    render_static(
        &mut store, &content, &world, origin, camera, 0.5, &mut swung,
    );
    assert_eq!(swung.vertices.len(), 8);
    let moved = swung.vertices.iter().zip(&rest.vertices).any(|(a, b)| {
        let d: [f32; 3] = std::array::from_fn(|i| a.position[i] - b.position[i]);
        d.iter().map(|v| v * v).sum::<f32>() > 1e-10
    });
    assert!(moved, "rung bell body must swing away from rest");
}
