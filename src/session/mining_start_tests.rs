//! MCP mining tools: a refused or unreachable attack must name its cause instead of
//! surfacing as an opaque `started:false` / "omitted actual started fact" after the
//! whole deadline, and a block that never shows a crack must not idle the task.
use super::*;
use crate::{
    entities::Models,
    minecraft::{self, Outcome},
    protocol::{
        network::Network,
        world::{Chunk, Palette, Section, SurfaceHeightmap},
    },
};
use std::net::TcpListener;

/// Player eye at (4.5, 5.62, 4.5) standing in an all-air chunk.
fn session() -> Session {
    let mut session = Session::new_mode("mining-start-test".into(), false);
    let palette = |size| Palette {
        bits: 0,
        values: vec![0],
        words: Vec::new(),
        size,
    };
    session.world = World::new(0, 16);
    session
        .world
        .insert(
            [0, 0],
            Chunk {
                sections: vec![Arc::new(Section {
                    blocks: palette(4096),
                    biomes: palette(64),
                    non_empty_block_count: 0,
                })],
                sky: BTreeMap::new(),
                block: BTreeMap::new(),
                block_entities: crate::protocol::world::empty_block_entities(),
                surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                surface_heightmap_received: false,
            },
        )
        .unwrap();
    session.entities.models = Some(Arc::new(Models {
        rigs: BTreeMap::new(),
        schemas: BTreeMap::new(),
        families: BTreeMap::new(),
        builtin_variants: BTreeMap::new(),
        simple_particles: Vec::new(),
        default_skins: Vec::new(),
        poses: BTreeMap::from([(0, "STANDING".into())]),
        attributes: BTreeMap::new(),
        attribute_ids: BTreeMap::new(),
        digest: String::new(),
        presentation: BTreeMap::new(),
        animations: BTreeMap::new(),
        motion: BTreeMap::new(),
    }));
    session.content = Some(Content::test_cube());
    session.phase = "play".into();
    session.player.position = [4.5, 4., 4.5];
    session.player.pose_known = true;
    session
}

fn cube(session: &mut Session, position: [i32; 3]) {
    session
        .world
        .set(position, 1, |state| Ok(state == 0))
        .unwrap();
}

fn mine(session: &mut Session, name: &str, position: [i32; 3]) -> Result<Outcome<minecraft::Task>> {
    let arguments = json!({
        "x": position[0], "y": position[1], "z": position[2], "face": "up",
        "ticks": 3, "timeout_seconds": 5.0,
        "expected_drop_item_id": "test:cube", "expected_drop_count": 0,
    });
    minecraft::start(session, name, &arguments)
}

fn refusal(session: &mut Session, name: &str, position: [i32; 3]) -> String {
    match mine(session, name, position) {
        Err(error) => error,
        Ok(_) => panic!("{name} unexpectedly started"),
    }
}

#[test]
fn unreachable_block_names_reach_instead_of_the_started_fact() {
    let mut session = session();
    cube(&mut session, [4, 4, 12]);
    // A nearby plant/cube on the way must not turn "too far" into "blocked".
    cube(&mut session, [4, 5, 6]);
    for name in ["minecraft_mine_until_crack", "minecraft_break_block"] {
        let error = refusal(&mut session, name, [4, 4, 12]);
        assert!(error.contains("out of reach"), "{name}: {error}");
        assert!(error.contains("4.5"), "{name}: {error}");
    }
}

#[test]
fn air_target_fails_immediately_instead_of_burning_the_deadline() {
    let mut session = session();
    for name in ["minecraft_mine_until_crack", "minecraft_break_block"] {
        let error = refusal(&mut session, name, [4, 4, 7]);
        assert!(error.contains("already air"), "{name}: {error}");
    }
}

#[test]
fn occluded_block_names_the_block_in_front() {
    let mut session = session();
    cube(&mut session, [4, 5, 6]);
    cube(&mut session, [4, 4, 7]);
    for name in ["minecraft_mine_until_crack", "minecraft_break_block"] {
        let error = refusal(&mut session, name, [4, 4, 7]);
        assert!(error.contains("not targetable"), "{name}: {error}");
        assert!(error.contains("4,5,6"), "{name}: {error}");
    }
}

#[test]
fn spectator_and_adventure_name_the_game_mode() {
    let mut session = session();
    cube(&mut session, [4, 4, 6]);
    session.player.game_mode = 3;
    for name in ["minecraft_mine_until_crack", "minecraft_break_block"] {
        let error = refusal(&mut session, name, [4, 4, 6]);
        assert!(error.contains("spectator"), "{name}: {error}");
    }
    session.player.game_mode = 2;
    session.player.abilities.may_build = false;
    for name in ["minecraft_mine_until_crack", "minecraft_break_block"] {
        let error = refusal(&mut session, name, [4, 4, 6]);
        assert!(error.contains("adventure"), "{name}: {error}");
    }
}

/// A loopback peer is enough: commands only need a live outbound queue.
fn connect(session: &mut Session) -> TcpListener {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap().to_string();
    session.network = Some(Network::connect(address, "Miner".into(), 8, Arc::new(|| {})).unwrap());
    listener
}

#[test]
fn instant_break_without_a_crack_fails_loudly_instead_of_idling() {
    let mut session = session();
    let _peer = connect(&mut session);
    let content = session.content.as_mut().unwrap();
    content.states[1].hardness = 0.;
    content.insert_test_block_sound(
        1,
        1.,
        1.,
        BTreeMap::from([("break".into(), "test:break".into())]),
    );
    cube(&mut session, [4, 4, 6]);
    let started = mine(&mut session, "minecraft_mine_until_crack", [4, 4, 6]);
    let Ok(Outcome::Pending(mut task)) = started else {
        panic!("an in-reach block must start: {:?}", started.err());
    };
    session.client_tick += 3;
    let error = task
        .on_event(&mut session)
        .expect_err("a block that broke early shows no crack stage");
    assert!(error.contains("no crack stage"), "{error}");
}

#[test]
fn f5_key_cycles_the_camera_through_the_ordinary_key_path() {
    let mut session = session();
    for (key, mode) in [
        ("key.keyboard.f5", 1),
        ("key.keyboard.f5", 2),
        ("key.keyboard.f5", 0),
    ] {
        session
            .input(InputIntent::Key {
                key: key.into(),
                pressed: true,
            })
            .unwrap();
        assert_eq!(session.camera_mode, mode);
        session
            .input(InputIntent::Key {
                key: key.into(),
                pressed: false,
            })
            .unwrap();
        assert!(session.keys.is_empty());
    }
    // The look and interaction eye never move with the view.
    assert_eq!(session.look_angles(), (180., 8.));
}
