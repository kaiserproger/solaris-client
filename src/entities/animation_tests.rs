//! Authored state/rig fixtures; no proprietary model or texture bytes.
use super::*;
use glam::{Mat4, Vec3};
use resources::{Bone, BoneRole, Rig};

fn fixture() -> (Models, Entity) {
    let mut models = Models {
        rigs: BTreeMap::new(),
        schemas: BTreeMap::new(),
        families: BTreeMap::new(),
        builtin_variants: BTreeMap::new(),
        simple_particles: vec![],
        default_skins: vec![],
        poses: BTreeMap::from([(0, "STANDING".into())]),
        attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(),
        digest: String::new(),
        armor_models: BTreeMap::new(),
        motion: BTreeMap::new(),
    };
    let transform = Transform {
        position: [0.; 3],
        yaw: 0.,
        pitch: 0.,
        head_yaw: 0.,
    };
    let mut entity = Entity {
        handle: Handle {
            network_id: 41,
            generation: 9,
        },
        uuid: "00000000-0000-0000-0000-000000000041".into(),
        kind: 1,
        type_name: "minecraft:pig".into(),
        position: [0.; 3],
        velocity: [0.; 3],
        transform,
        previous: transform,
        on_ground: true,
        spawn_data: 0,
        observed_tick: 0,
        revision: 1,
        appearance_revision: 1,
        physical: physical::AnimationState::default(),
        binding: None,
        source: None,
        minecart_steps: vec![],
        acceleration_power: None,
        metadata: BTreeMap::new(),
        equipment: std::array::from_fn(|_| None),
        predicted_metadata: BTreeMap::new(),
        predicted_equipment: std::array::from_fn(|_| None),
        predicted_leash: None,
        effects: BTreeMap::new(),
        attributes: BTreeMap::new(),
        passenger_ids: vec![],
        passengers: vec![],
        vehicle: None,
        leash: None,
        leash_holder_id: None,
        animation: None,
        damage_tick: None,
        death_tick: None,
        hurt_direction: 0.,
        submitted_revision: None,
    };
    let facts: render_living::MotionFacts = serde_json::from_value(json!({
        "class":"authored.fixture", "is_mob":true, "walk_uses_y":"never", "methods":{},
        "body_rule":"standard", "walk_rule":"standard",
        "head_rule":{"kind":"fixed","value":75.0},
        "body_head_rule":{"kind":"fixed","value":75.0}
    }))
    .unwrap();
    models.motion.insert(entity.type_name.clone(), facts);
    models.schemas.insert(
        entity.type_name.clone(),
        resources::EntitySchema {
            protocol_id: 1,
            width: 1.,
            fire_immune: false,
            class: "authored.fixture".into(),
            eye_height: 0.8,
            dimension_constants: BTreeMap::new(),
            enum_ids: BTreeMap::new(),
            enum_numbers: BTreeMap::new(),
            hierarchy: vec!["net.minecraft.world.entity.LivingEntity".into()],
            method_owners: BTreeMap::new(),
            dimensions: resources::Dimensions {
                width: 1.,
                height: 1.,
                eye_height: 0.8,
                fixed: false,
                attachments: BTreeMap::new(),
            },
            default_attributes: BTreeMap::from([(
                "minecraft:scale".into(),
                resources::DefaultAttribute { id: 1, value: 1. },
            )]),
            height: 1.,
            accessors: BTreeMap::new(),
        },
    );
    entity.source = Some(source_state::SourceState::new(&entity, &models).unwrap());
    (models, entity)
}

fn tick_walk(models: &Models, entity: &mut Entity, x: f64) {
    let mut source = entity.source.take().unwrap();
    source.living.as_mut().unwrap().tick(
        models,
        entity,
        &render_living::TickInput {
            age: 1,
            position: [x, 0., 0.],
            yaw: 0.,
            alive: true,
            passenger: false,
            baby: false,
            visually_swimming: false,
            in_water: false,
            uses_y_for_walk: false,
            rotation_changed_before_base_tick: false,
            is_mob: true,
            first_passenger_is_mob: false,
            max_head_yaw: 75.,
            max_body_head_yaw: 75.,
            attack_active: false,
            camel_sitting: false,
            camel_visually_sitting: false,
            camel_sitting_down: false,
            camel_pose_transition: false,
            camel_pose_time: 0,
        },
    );
    entity.source = Some(source);
}
fn rig() -> Rig {
    let bones = [
        ("right_hind_leg", BoneRole::RightLeg),
        ("left_hind_leg", BoneRole::LeftLeg),
        ("right_arm", BoneRole::RightArm),
        ("left_arm", BoneRole::LeftArm),
        ("head", BoneRole::Head),
    ]
    .into_iter()
    .map(|(name, role)| Bone {
        parent: None,
        pivot: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role,
        name: name.into(),
        quads: vec![],
        normals: vec![],
    })
    .collect();
    Rig {
        bones,
        part_by_name: BTreeMap::new(),
    }
}

#[test]
fn animation_received_motion_swings_limbs_without_velocity_packets() {
    let (models, mut entity) = fixture();
    let rig = rig();
    let mut idle = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, 0, 1., Mat4::IDENTITY, &mut idle);
    tick_walk(&models, &mut entity, 0.2);
    tick_walk(&models, &mut entity, 0.4);
    assert_eq!(entity.velocity, [0.; 3]);
    let mut moving = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, 2, 1., Mat4::IDENTITY, &mut moving);
    assert_ne!(moving[0], idle[0]);
    assert_ne!(moving[2], idle[2]);
    let right = moving[0].transform_vector3(Vec3::Y);
    let left = moving[1].transform_vector3(Vec3::Y);
    assert!(
        right.z * left.z < 0.,
        "opposite legs must swing in opposite directions"
    );
    let mut halfway = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, 2, 0.5, Mat4::IDENTITY, &mut halfway);
    assert_ne!(halfway[0], moving[0]);
}

#[test]
fn animation_villager_model_family_swings_legs_at_half_amplitude() {
    let (models, mut entity) = fixture();
    let rig = rig();
    tick_walk(&models, &mut entity, 0.2);
    tick_walk(&models, &mut entity, 0.4);
    let swing = |entity: &Entity| {
        let mut matrices = vec![];
        presentation::evaluate_rig(&rig, entity, &models, 2, 1., Mat4::IDENTITY, &mut matrices);
        matrices[0].transform_vector3(Vec3::Y).z.asin()
    };
    let humanoid = swing(&entity);
    assert!(humanoid.abs() > 0.05, "fixture must actually walk");
    for name in [
        "minecraft:villager",
        "minecraft:wandering_trader",
        "minecraft:witch",
    ] {
        entity.type_name = name.into();
        let family = swing(&entity);
        assert!(
            (family - humanoid * 0.5).abs() < 1e-5,
            "{name} legs {family} vs humanoid {humanoid}"
        );
    }
}

/// A handheld tool in an arm that hangs at the side must point forward out of the fist
/// (source ItemInHandLayer + `rotationXYZ` display transform), not lie along the arm.
#[test]
fn held_tool_points_forward_from_the_fist_not_along_the_arm() {
    use crate::protocol::models::{BakedModel, ModelTransform};
    let mut model = BakedModel::default();
    model.display.insert(
        "thirdperson_righthand".into(),
        ModelTransform {
            rotation: [0., -90., 55.],
            translation: [0., 4. / 16., 0.5 / 16.],
            scale: [0.85; 3],
        },
    );
    // Arm bone frame: +Y runs down the arm, model front is -Z.
    let matrix = presentation::held_item_attachment(Mat4::IDENTITY, 0)
        * presentation::item_matrix(&model, "thirdperson_righthand");
    let handle = matrix.transform_point3(Vec3::new(0.1, 0.1, 0.5));
    let tip = matrix.transform_point3(Vec3::new(0.95, 0.95, 0.5));
    let blade = tip - handle;
    assert!(
        blade.z < -0.9,
        "blade must point forward (-Z), got {blade:?}"
    );
    assert!(blade.x.abs() < 0.05, "no sideways lean, got {blade:?}");
    assert!(
        blade.y.abs() < 0.25,
        "near level, not along the arm, got {blade:?}"
    );
    // The grip sits at the fist, 0.625 down the arm and 1/16 outboard of the right arm.
    assert!(
        (handle.y - 0.51).abs() < 0.05 && handle.x < 0.,
        "{handle:?}"
    );
    let left = presentation::held_item_attachment(Mat4::IDENTITY, 1);
    assert!(left.transform_point3(Vec3::ZERO).x > 0.);
}

#[test]
fn animation_idle_is_not_driven_by_global_clock_or_stale_velocity() {
    let (models, mut entity) = fixture();
    entity.velocity = [0.5, 0., 0.];
    let rig = rig();
    let mut first = vec![];
    let mut later = vec![];
    presentation::evaluate_rig(&rig, &entity, &models, 0, 1., Mat4::IDENTITY, &mut first);
    presentation::evaluate_rig(&rig, &entity, &models, 100, 1., Mat4::IDENTITY, &mut later);
    assert_eq!(first, later);
}

#[test]
fn animation_head_tracks_relative_to_lagged_body() {
    let (models, mut entity) = fixture();
    let living = entity.source.as_mut().unwrap().living.as_mut().unwrap();
    living.body_yaw_old = 20.;
    living.body_yaw = 40.;
    living.head_yaw_old = 60.;
    living.head_yaw = 80.;
    entity.transform.pitch = 25.;
    entity.previous.pitch = 25.;
    let mut matrices = vec![];
    presentation::evaluate_rig(
        &rig(),
        &entity,
        &models,
        0,
        0.5,
        Mat4::IDENTITY,
        &mut matrices,
    );
    let expected =
        Mat4::from_rotation_y(40f32.to_radians()) * Mat4::from_rotation_x(25f32.to_radians());
    for (actual, expected) in matrices[4]
        .to_cols_array()
        .into_iter()
        .zip(expected.to_cols_array())
    {
        assert!((actual - expected).abs() < 1e-5);
    }
}


#[test]
fn rc2_received_sleep_pose_rotates_rig_horizontally() {
    let (mut models, mut entity) = fixture();
    let schema = models.schemas.get_mut(&entity.type_name).unwrap();
    schema.accessors.insert("DATA_POSE".into(), [6, 20]);
    schema.accessors.insert("SLEEPING_POS_ID".into(), [14, 11]);
    // Entity metadata body from the server: pose=2 and optional head block position=(0,0,0).
    let packet = [6, 20, 2, 14, 11, 1, 0, 0, 0, 0, 0, 0, 0, 0, 255];
    entity.metadata = metadata::decode(&mut crate::protocol::Reader::new(&packet), &Content::test_cube(), &models).unwrap();
    models.rigs.insert("authored:rig".into(), Rig {
        bones: vec![Bone {
            parent: None, pivot: Vec3::ZERO, rotation: Vec3::ZERO, scale: Vec3::ONE,
            role: BoneRole::Fixed, name: "body".into(),
            quads: vec![[[0., 0., 0., 0., 0.], [1., 0., 0., 1., 0.],
                         [1., 1., 0., 1., 1.], [0., 1., 0., 0., 1.]]],
            normals: vec![[0., 0., 1.]],
        }],
        part_by_name: BTreeMap::new(),
    });
    entity.binding = Some(Ok(presentation::Binding {
        rig: "authored:rig".into(),
        texture: crate::protocol::content::TextureRegion { uv: [0., 0., 1., 1.], layer: 0, width: 1, height: 1 },
        style: presentation::RootStyle::Living, layers: None,
    }));
    models.poses.insert(2, "SLEEPING".into());
    models.schemas.get_mut(&entity.type_name).unwrap().dimension_constants.insert(
        "SLEEPING_DIMENSIONS".into(),
        resources::DimensionConstant::Dimensions(resources::Dimensions {
            width: 0.2, height: 0.2, eye_height: 0.2, fixed: true, attachments: BTreeMap::new(),
        }),
    );
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.local_avatar = Some(entity);
    store.set_local_visible(true);
    let mut axes = Vec::new();
    for direction in [[0, 1], [-1, 0], [0, -1], [1, 0]] {
        let mut geometry = DynamicGeometry::default();
        store.append_geometry(&Content::test_cube(), [0.; 3], [0.; 3], [0.; 2], 32.,
            0, 0., &|_| Ok(Some([15., 0.])), &|position| {
                assert_eq!(position, [0, 0, 0]);
                Some(direction)
            }, &mut geometry).unwrap();
        assert_eq!(geometry.vertices.len(), 4);
        assert!(geometry.vertices[0].normal[1] < -0.99, "the model back must face downward");
        let a = Vec3::from_array(geometry.vertices[0].position);
        let b = Vec3::from_array(geometry.vertices[3].position);
        let axis = b - a;
        assert!(axis.y.abs() < 1e-5, "body axis must be horizontal: {axis:?}");
        axes.push(axis);
    }
    assert!(axes[0].dot(axes[2]) < -0.99);
    assert!(axes[1].dot(axes[3]) < -0.99);
    assert!(axes[0].dot(axes[1]).abs() < 1e-5);
}

#[test]
fn rc2_passenger_packet_bends_villager_legs_and_dismount_restores_them() {
    use crate::protocol::{Reader, packet_ids::play::clientbound as cb};
    let (models, mut villager) = fixture();
    let (_, mut seat) = fixture();
    seat.handle.network_id = 42;
    villager.type_name = "minecraft:villager".into();
    let mut store = Store::new();
    store.slots = vec![villager, seat];
    store.ids.insert(41, 0);
    store.ids.insert(42, 1);
    let matrices = |store: &Store| {
        let mut out = Vec::new();
        presentation::evaluate_rig(&rig(), store.get(41).unwrap(), &models, 0, 0., Mat4::IDENTITY, &mut out);
        out
    };
    let standing = matrices(&store);
    // Vehicle 42, one passenger 41: the ordinary SET_PASSENGERS wire body.
    store.apply(cb::SET_PASSENGERS, &mut Reader::new(&[42, 1, 41]), &Content::test_cube(), 0).unwrap();
    let seated = matrices(&store);
    assert!(seated[0].transform_vector3(Vec3::Y).z.abs() > 0.9);
    assert!(seated[1].transform_vector3(Vec3::Y).z.abs() > 0.9);
    store.apply(cb::SET_PASSENGERS, &mut Reader::new(&[42, 0]), &Content::test_cube(), 0).unwrap();
    assert_eq!(matrices(&store), standing);
}
#[test]
fn animation_local_render_slot_is_restored_even_when_extraction_fails() {
    let (_, entity) = fixture();
    let mut store = Store::new();
    store.local_avatar = Some(entity);
    store.set_local_visible(true);
    let result = store.append_geometry(
        &Content::test_cube(),
        [0.; 3],
        [0.; 3],
        [0.; 2],
        32.,
        0,
        0.,
        &|_| Ok(Some([15., 0.])),
        &|_| None,
        &mut DynamicGeometry::default(),
    );
    assert!(result.is_err());
    assert!(store.local_avatar.is_some());
    assert!(store.local_render_slot.is_none());
    assert_eq!(store.len(), 0);
    assert!(store.ids.is_empty() && store.spatial.is_empty());
}

#[test]
fn animation_server_interpolation_keeps_ground_height_and_shortest_rotation() {
    let (_, mut entity) = fixture();
    entity.previous.position = [0., 64., 0.];
    entity.transform.position = [0.3, 64., 0.3];
    entity.previous.yaw = 179.;
    entity.transform.yaw = -179.;
    entity.physical.lerp_duration = 3;
    entity.physical.lerp_progress = 1;
    let halfway = entity.presented_transform(10, 0.5);
    assert_eq!(halfway.position, [0.15, 64., 0.15]);
    assert_eq!(halfway.yaw, 180.);
}

#[test]
fn animation_midtick_packet_continues_from_the_displayed_pose_without_snapping() {
    let (_, mut entity) = fixture();
    entity.previous.position = [0., 64., 0.];
    entity.transform.position = [3., 64., 0.];
    entity.physical.lerp_duration = 3;
    entity.physical.lerp_progress = 1;
    let shown = entity.presented_transform(10, 0.5).position[0];
    assert!((shown - 1.5).abs() < 1e-9);
    // A new position packet lands half a tick into tick 10.
    let mut target = entity.transform;
    target.position = [4.5, 64., 0.];
    entity.move_to(target, None, true, 10, false, 0.5);
    let at_arrival = entity.presented_transform(10, 0.5).position[0];
    assert!((at_arrival - shown).abs() < 1e-9, "{at_arrival} vs {shown}");
    // Time since arrival drives the blend across the tick boundary: no jump.
    let before = entity.presented_transform(10, 0.999).position[0];
    entity.physical.lerp_progress = 1;
    let after = entity.presented_transform(11, 0.).position[0];
    assert!(
        after >= before && after - before < 0.02,
        "{before} -> {after}"
    );
    // Duration is measured from arrival, then it settles exactly on target
    // without a sawtooth when the tick counter stops at duration + 1.
    entity.physical.lerp_progress = 3;
    let late = entity.presented_transform(13, 0.25).position[0];
    assert!(late < 4.5);
    entity.physical.lerp_progress = 4;
    for partial in [0., 0.3, 0.99] {
        assert_eq!(entity.presented_transform(14, partial).position[0], 4.5);
    }
    // A snap (teleport) never inherits a phase.
    entity.move_to(target, None, true, 15, true, 0.7);
    assert_eq!(entity.physical.lerp_phase, 0.);
    assert_eq!(entity.presented_transform(15, 0.2).position[0], 4.5);
}

#[test]
fn animation_hurt_flash_is_emitted_and_expires_on_real_rig_vertices() {
    let (mut models, mut entity) = fixture();
    let bone = Bone {
        parent: None,
        pivot: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role: BoneRole::Fixed,
        name: "body".into(),
        quads: vec![[
            [0., 0., 0., 0., 0.],
            [1., 0., 0., 1., 0.],
            [1., 1., 0., 1., 1.],
            [0., 1., 0., 0., 1.],
        ]],
        normals: vec![[0., 0., 1.]],
    };
    models.rigs.insert(
        "authored:rig".into(),
        Rig {
            bones: vec![bone],
            part_by_name: BTreeMap::new(),
        },
    );
    entity.binding = Some(Ok(presentation::Binding {
        rig: "authored:rig".into(),
        texture: crate::protocol::content::TextureRegion {
            uv: [0., 0., 1., 1.],
            layer: 0,
            width: 1,
            height: 1,
        },
        style: presentation::RootStyle::Living,
        layers: None,
    }));
    entity.damage_tick = Some(10);
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.local_avatar = Some(entity);
    store.set_local_visible(true);
    for (tick, expected) in [(10, [0., 3.]), (19, [0., 3.]), (20, [0., 10.])] {
        let mut geometry = DynamicGeometry::default();
        let stats = store
            .append_geometry(
                &Content::test_cube(),
                [0.; 3],
                [0.; 3],
                [0.; 2],
                32.,
                tick,
                0.5,
                &|_| Ok(Some([15., 0.])),
                &|_| None,
                &mut geometry,
            )
            .unwrap();
        assert_eq!(stats.submitted, 1, "{:?}", stats.unrendered);
        assert_eq!(geometry.vertices.len(), 4);
        assert!(geometry
            .vertices
            .iter()
            .all(|vertex| vertex.overlay_uv == expected));
        assert_eq!(store.len(), 0);
    }
}

#[test]
fn first_person_arm_uses_baked_geometry_skin_uvs_and_normals() {
    let (mut models, mut entity) = fixture();
    let bone = Bone {
        parent: None,
        pivot: Vec3::new(-0.3, 0.125, 0.),
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role: BoneRole::RightArm,
        name: "right_arm".into(),
        quads: vec![[
            [0., 0.625, 0., 0.1, 0.2],
            [0., 0.625, 0.2, 0.1, 0.4],
            [0.2, 0.625, 0.2, 0.3, 0.4],
            [0.2, 0.625, 0., 0.3, 0.2],
        ]],
        normals: vec![[0., 1., 0.]],
    };
    models.rigs.insert(
        "authored:arm".into(),
        Rig {
            bones: vec![bone],
            part_by_name: BTreeMap::new(),
        },
    );
    entity.binding = Some(Ok(presentation::Binding {
        rig: "authored:arm".into(),
        texture: crate::protocol::content::TextureRegion {
            uv: [0.25, 0.5, 0.75, 1.],
            layer: 7,
            width: 64,
            height: 64,
        },
        style: presentation::RootStyle::Living,
        layers: None,
    }));
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    store.local_avatar = Some(entity);
    let root = Mat4::from_translation(Vec3::new(0.52, -0.9, -0.65))
        * Mat4::from_rotation_z(0.25)
        * Mat4::from_rotation_x(-0.15);
    let mut geometry = DynamicGeometry::default();
    store
        .append_local_arm(&Content::test_cube(), true, root, [15., 0.], &mut geometry)
        .unwrap();
    assert_eq!(geometry.vertices.len(), 4);
    assert_eq!(
        geometry.indices[crate::api::EntityMaterial::EntityCutout as usize].len(),
        6
    );
    let tip = root.transform_point3(Vec3::new(0., 0.625, 0.));
    assert!((Vec3::from_array(geometry.vertices[0].position) - tip).length() < 1e-6);
    assert!(
        tip.y > -0.4,
        "fist must be visible above the offscreen shoulder"
    );
    assert_eq!(geometry.vertices[0].uv, [0.3, 0.6]);
    let normal = root.transform_vector3(Vec3::Y).normalize();
    for vertex in &geometry.vertices {
        assert!((Vec3::from_array(vertex.normal) - normal).length() < 1e-6);
        assert_eq!(vertex.texture_layer, 7);
    }
}

#[test]
fn first_person_missing_local_profile_uses_classic_skin_without_changing_remotes() {
    let (mut models, mut entity) = fixture();
    entity.type_name = "minecraft:player".into();
    let mut store = Store::new();
    for (model, path) in [
        (PlayerModel::Slim, "minecraft:entity/player/slim/alex"),
        (PlayerModel::Wide, "minecraft:entity/player/wide/steve"),
    ] {
        models.default_skins = (0..18)
            .map(|_| resources::DefaultSkin {
                texture: "authored:modern-default".into(),
                model,
            })
            .collect();
        store.local_uuid = Some(entity.uuid.clone());
        let local =
            presentation::material(&store, &models, &entity, &Content::test_cube()).unwrap_err();
        assert!(local.ends_with(path), "{local}");
        store.local_uuid = None;
        let remote =
            presentation::material(&store, &models, &entity, &Content::test_cube()).unwrap_err();
        assert!(remote.ends_with("authored:modern-default"), "{remote}");
    }
}

#[test]
fn first_person_local_avatar_ignores_empty_equipment_stacks() {
    let (mut models, entity) = fixture();
    let mut schema = models.schemas.remove("minecraft:pig").unwrap();
    schema.class = "authored.player".into();
    models.schemas.insert("minecraft:player".into(), schema);
    let mut facts = models.motion.remove("minecraft:pig").unwrap();
    facts.is_mob = false;
    facts.body_rule = Some(render_living::BodyRule::Living);
    models.motion.insert("minecraft:player".into(), facts);
    let mut content = Content::test_cube();
    content.entity_names.insert(1, "minecraft:player".into());
    let mut inventory = crate::inventory::InventoryStore::default();
    for slot in [36, 45, 8, 7, 6, 5] {
        inventory.player.slots[slot] = Some(Stack::default());
    }
    let mut player = crate::player::Player::new();
    player.uuid = Some(Arc::from(entity.uuid));
    let mut store = Store::new();
    store.models = Some(Arc::new(models));
    let world = crate::protocol::world::World::default();
    store
        .sync_local_avatar(&player, &inventory, &world, &content, (1, 1))
        .unwrap();
    assert!(store
        .local_avatar
        .as_ref()
        .unwrap()
        .equipment
        .iter()
        .all(Option::is_none));
    let mut item = Stack::default();
    item.id = 1;
    item.count = 1;
    inventory.player.slots[36] = Some(item);
    store
        .sync_local_avatar(&player, &inventory, &world, &content, (2, 2))
        .unwrap();
    let equipment = &store.local_avatar.as_ref().unwrap().equipment;
    assert_eq!(equipment[0].as_ref().map(|s| s.id), Some(1));
    assert!(equipment[1..].iter().all(Option::is_none));
}

/// The villager family has no per-hand arm bones, only the crossed `arms`: the main-hand item
/// (a witch's potion hangs from the nose) must still be emitted, at that anchor.
#[test]
fn crossed_arms_rigs_emit_the_held_item_at_the_source_anchor() {
    use crate::protocol::content::Content;
    let (models, mut entity) = fixture();
    let content = {
        let mut content = Content::test_cube();
        content.item_names.insert(1, "minecraft:stick".into());
        content.item_names.insert(2, "minecraft:potion".into());
        content
    };
    let cube = Content::test_cube().models[0].clone();
    for (kind, item, anchor) in [
        ("minecraft:villager", 1, "arms"),
        ("minecraft:wandering_trader", 1, "arms"),
        ("minecraft:witch", 1, "arms"),
        ("minecraft:witch", 2, "nose"),
    ] {
        let rig = solaris_models::authored(kind).unwrap();
        let mut items = crate::ui::items::ItemModels::new();
        items.insert_test_model("test:held", cube.clone());
        let mut stack = Stack::default();
        stack.id = item;
        stack.count = 1;
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:item_model".into(),
            serde_json::json!("test:held"),
        )]));
        entity.type_name = kind.into();
        entity.equipment[0] = Some(stack);
        let mut transforms = vec![];
        presentation::evaluate_rig(rig, &entity, &models, 0, 0., Mat4::IDENTITY, &mut transforms);
        let mut out = DynamicGeometry::default();
        let mut stats = presentation::PresentationStats::default();
        presentation::append_equipment(
            &entity,
            rig,
            &transforms,
            &mut items,
            &mut equipment::Cache::default(),
            Mat4::IDENTITY,
            &content,
            &models,
            0,
            0.,
            [15., 0.],
            &mut out,
            &mut stats,
        )
        .unwrap();
        assert!(stats.unrendered.is_empty(), "{kind}: {:?}", stats.unrendered);
        assert_eq!(out.vertices.len(), 24, "{kind} item {item} emitted");
        let centroid = out
            .vertices
            .iter()
            .fold(Vec3::ZERO, |sum, v| sum + Vec3::from_array(v.position))
            / out.vertices.len() as f32;
        let bone = transforms[rig.part_by_name[anchor]].transform_point3(Vec3::ZERO);
        assert!(
            (centroid - bone).length() < 0.45,
            "{kind}: item at {centroid:?} not at {anchor} {bone:?}"
        );
    }
}
