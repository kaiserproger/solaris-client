//! Real 26.3 passenger packets must move the local player's camera with the boat.
use super::*;
use crate::entities::Models;
use crate::protocol::{packet_ids::play::clientbound as cb, world::{Chunk, Palette, Section, SurfaceHeightmap}, Writer};

fn fixture() -> (Session, std::net::TcpListener) {
    let mut session = Session::new_mode("boat-camera-test".into(), false);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    session.network = Some(Network::connect(listener.local_addr().unwrap().to_string(), "BoatTest".into(), 8, Arc::new(|| {})).unwrap());
    let mut content = Content::test_cube();
    // Use the pinned 26.3 registry/schema, not a guessed boat entity class.
    let facts: Value = serde_json::from_str(include_str!("../../assets/solaris/entities/entity-facts.json")).unwrap();
    for (name, schema) in facts["entities"].as_object().unwrap() {
        content.entity_names.insert(schema["protocol_id"].as_u64().unwrap() as u32, name.clone());
    }
    // Empty-air fixture still needs the received holder tables used by collision
    // and movement tag lookups. No tested cell belongs to these tags.
    content.set_registries(BTreeMap::from([
        ("minecraft:block".into(), vec![("test:air".into(), Value::Null), ("test:cube".into(), Value::Null)]),
        ("minecraft:item".into(), vec![("minecraft:air".into(), Value::Null)]),
        ("minecraft:fluid".into(), vec![("minecraft:empty".into(), Value::Null)]),
        ("minecraft:entity_type".into(), content.entity_names.values().map(|name| (name.clone(), Value::Null)).collect()),
    ]));
    content.set_tags(BTreeMap::from([
        ("minecraft:block".into(), BTreeMap::new()),
        ("minecraft:item".into(), BTreeMap::from([("minecraft:freeze_immune_wearables".into(), vec![])])),
        ("minecraft:fluid".into(), BTreeMap::new()),
        ("minecraft:entity_type".into(), BTreeMap::from([
            ("minecraft:powder_snow_walkable_mobs".into(), vec![]),
            ("minecraft:can_float_while_ridden".into(), vec![]),
            ("minecraft:freeze_immune_entity_types".into(), vec![]),
        ])),
    ])).unwrap();
    let models = Arc::new(Models::load_state(&content).unwrap());
    session.player.init_vanilla_defaults(models.schemas["minecraft:player"].default_attributes.iter().map(|(name, attribute)| (name.as_str(), attribute.value)));
    session.entities.activate_models(models);
    session.content = Some(content);
    session.world = World::new(0, 16);
    let palette = |size| Palette { bits: 0, values: vec![0], words: vec![], size };
    session.world.insert([0, 0], Chunk {
        sections: vec![Arc::new(Section { blocks: palette(4096), biomes: palette(64), non_empty_block_count: 0 })],
        sky: BTreeMap::new(), block: BTreeMap::new(),
        block_entities: crate::protocol::world::empty_block_entities(),
        surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()), surface_heightmap_received: false,
    }).unwrap();
    session.environment.activate_dimension(
        "minecraft:overworld",
        &json!({"min_y":0,"height":16,"has_skylight":false,"has_ceiling":true,"skybox":"none"}),
        0,
    ).unwrap();
    session.phase = "play".into();
    session.player.entity_id = 7;
    session.player.pose_known = true;
    session.player.position = [3.5, 5., 4.5];
    let mut time = Writer::default();
    time.i64(100);
    time.var(0); // 26.3 world-clock updates
    session.apply(Phase::Play, cb::SET_TIME, &time.0).unwrap();
    let mut boat = Writer::default();
    boat.var(42);
    boat.0.extend([0u8; 16]);
    boat.var(90); // 26.3 minecraft:oak_boat, entity-facts.json
    for coordinate in [6.5, 5., 4.5] { boat.f64(coordinate); }
    boat.0.extend([0u8; 5]); // zero LP velocity, pitch, yaw, head yaw, data
    session.apply(Phase::Play, cb::ADD_ENTITY, &boat.0).unwrap();
    (session, listener)
}

fn passengers(session: &mut Session, mounted: bool) {
    let mut packet = Writer::default();
    packet.var(42);
    packet.var(i32::from(mounted));
    if mounted { packet.var(7); }
    session.apply(Phase::Play, cb::SET_PASSENGERS, &packet.0).unwrap();
}

fn tick(session: &mut Session) {
    session.tick_after_events(true);
    assert_eq!(session.error, None, "session tick must reach player prediction");
    assert_eq!(session.player.movement_blocked, None, "mounted prediction must not silently fail");
}
fn assert_seat_height(session: &Session) {
    let (_, vehicle, _, _) = session.player.predicted_vehicle_transform().unwrap();
    // 26.3 Boat.rideHeight = dimensions.height / 3; Entity.positionRider
    // subtracts the standing player's VEHICLE attachment (0.6).
    let seat_y = vehicle[1] + f64::from(0.5625f32 / 3.) - 0.6;
    assert!((session.player.position[1] - seat_y).abs() < 1e-6);
    assert!((session.player.eye_position()[1] - seat_y - f64::from(1.62f32)).abs() < 1e-6);
}


#[test]
fn rc1_boat_passenger_camera_tracks_mount_correction_and_reboarding() {
    let (mut session, _listener) = fixture();
    passengers(&mut session, true);
    assert!(session.entities.vehicle_of(7).is_some()); // own entity is intentionally absent
    tick(&mut session);
    assert!((session.player.position[0] - 6.5).abs() < 1e-6, "boarding must move local view to boat: {:?}", session.player.position);
    assert_seat_height(&session);
    let eye = session.player.eye_position();
    assert!((eye[0] - session.player.position[0]).abs() < 1e-6);

    let mut correction = Writer::default();
    for coordinate in [8.5, 5., 4.5] { correction.f64(coordinate); }
    correction.f32(0.);
    correction.f32(0.);
    session.apply(Phase::Play, cb::MOVE_VEHICLE, &correction.0).unwrap();
    tick(&mut session);
    assert!((session.player.position[0] - 8.5).abs() < 1e-6, "vehicle correction must move local view: {:?}", session.player.position);
    assert_seat_height(&session);

    passengers(&mut session, false);
    assert!(session.entities.vehicle_of(7).is_none());
    tick(&mut session);
    passengers(&mut session, true);
    tick(&mut session);
    assert!((session.player.position[0] - 6.5).abs() < 1e-6, "reboarding must follow received boat: {:?}", session.player.position);
    assert_seat_height(&session);
}
