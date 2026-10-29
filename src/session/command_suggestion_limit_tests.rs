//! Command suggestion limits through the real session packet and chat paths.
use super::*;
use crate::protocol::{
    Reader, packet_ids::play::{clientbound as cb, serverbound as sb},
    world::{Chunk, Palette, Section, SurfaceHeightmap},
};
use crate::ui::{Modifiers, UiIntent};
use std::{io::{Read, Write}, net::{TcpListener, TcpStream}, sync::mpsc};

fn read_packet(socket: &mut TcpStream) -> (i32, Vec<u8>) {
    let mut header = Vec::new();
    loop {
        let mut byte = [0];
        socket.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
        if byte[0] & 128 == 0 { break; }
        assert!(header.len() < 5);
    }
    let mut body = vec![0; Reader::new(&header).count(32768).unwrap()];
    socket.read_exact(&mut body).unwrap();
    let mut reader = Reader::new(&body);
    let id = reader.var().unwrap();
    (id, body[reader.offset..].to_vec())
}

fn send_packet(socket: &mut TcpStream, id: i32, body: &[u8]) {
    socket.write_all(&crate::protocol::frame(id, body, None).unwrap()).unwrap();
}

type Fixture = (Session, i32, mpsc::Sender<Vec<u8>>, mpsc::Receiver<()>);

struct NetworkFixture {
    network: Network,
    requests: mpsc::Receiver<Vec<u8>>,
    responses: mpsc::Sender<Vec<u8>>,
    delivered: mpsc::Receiver<()>,
}

fn network_fixture() -> NetworkFixture {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (request_tx, request_rx) = mpsc::channel();
    let (response_tx, response_rx) = mpsc::channel::<Vec<u8>>();
    let (delivered_tx, delivered_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();
    thread::spawn(move || {
        let (mut login, _) = listener.accept().unwrap();
        let (mut status, _) = listener.accept().unwrap();
        login.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        login.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        read_packet(&mut status);
        read_packet(&mut status);
        let mut status_response = Writer::default();
        status_response.string(r#"{"version":{"protocol":777}}"#);
        send_packet(&mut status, 0, &status_response.0);
        read_packet(&mut login);
        read_packet(&mut login);
        let mut success = Writer(vec![0; 16]);
        success.string("SuggestionTest");
        success.var(0);
        send_packet(&mut login, 2, &success.0);
        assert_eq!(read_packet(&mut login).0, 3);
        assert_eq!(read_packet(&mut login).0, 0);
        send_packet(&mut login, 3, &[]);
        assert_eq!(read_packet(&mut login).0, 3);
        // An echoed Play keepalive proves the I/O owner passed configuration.
        send_packet(&mut login, cb::KEEP_ALIVE, &456i64.to_be_bytes());
        assert_eq!(read_packet(&mut login), (sb::KEEP_ALIVE, 456i64.to_be_bytes().to_vec()));
        ready_tx.send(()).unwrap();
        let (id, body) = read_packet(&mut login);
        assert_eq!(id, sb::COMMAND_SUGGESTION);
        request_tx.send(body).unwrap();
        if let Ok(response) = response_rx.recv() {
            send_packet(&mut login, cb::COMMAND_SUGGESTIONS, &response);
            send_packet(&mut login, cb::KEEP_ALIVE, &123i64.to_be_bytes());
            assert_eq!(read_packet(&mut login), (sb::KEEP_ALIVE, 123i64.to_be_bytes().to_vec()));
            delivered_tx.send(()).unwrap();
            let _ = response_rx.recv();
        }
    });
    let network = Network::connect(address.to_string(), "SuggestionTest".into(), 8, Arc::new(|| {})).unwrap();
    ready_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    NetworkFixture { network, requests: request_rx, responses: response_tx, delivered: delivered_rx }
}

fn tab() -> UiIntent {
    UiIntent::Key { key: "Tab".into(), pressed: true, modifiers: Modifiers::default() }
}

fn requested_session() -> Fixture {
    let mut session = Session::new_mode("suggestion-limit".into(), false);
    let mut content = Content::test_cube();
    content.creative_tabs_report = Arc::from(br#"{"tabs":[]}"#.as_slice());
    session.ui = Some(UiState::new_state(&content).unwrap());
    session.content = Some(content);
    session.phase = "play".into();
    session.dimension = Some("minecraft:overworld".into());
    session.player.pose_known = true;
    session.player.position = [4.5, 4., 4.5];
    session.world = World::new(0, 16);
    let uniform = |size| Palette { bits: 0, values: vec![0], words: Vec::new(), size };
    for x in -1..=1 {
        for z in -1..=1 {
            session.world.insert([x, z], Chunk {
                sections: vec![Arc::new(Section {
                    blocks: uniform(4096), biomes: uniform(64), non_empty_block_count: 0,
                })],
                sky: BTreeMap::from([(1, Arc::new(vec![0xff; 2048]))]),
                block: BTreeMap::from([(1, Arc::new(vec![0; 2048]))]),
                block_entities: crate::protocol::world::empty_block_entities(),
                surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
                surface_heightmap_received: false,
            }).unwrap();
        }
    }
    assert!(session.play_world_presentation_ready());
    let mut tree = Writer::default();
    tree.var(3);
    tree.byte(0); tree.var(1); tree.var(1);
    tree.byte(1); tree.var(1); tree.var(2); tree.string("msg");
    tree.byte(18); tree.var(0); tree.string("target");
    tree.var(5); tree.var(0); tree.string("minecraft:ask_server");
    tree.var(0);
    session.apply(Phase::Play, cb::COMMANDS, &tree.0).unwrap();
    let NetworkFixture { network, requests, responses, delivered } = network_fixture();
    session.network = Some(network);
    // Content and chat are already installed; discard the handshake events.
    for _ in 0..3 { assert!(session.network.as_ref().unwrap().try_recv().is_ok()); }
    // Settle the initial loading presentation before the user opens chat/IME.
    // Otherwise its first visibility transition legitimately releases composition.
    session.process_events();
    assert!(session.error.is_none(), "{:?}", session.error);
    session.execute_ui(UiIntent::OpenChat { command: true }).unwrap();
    session.ui.as_mut().unwrap().rebuild_for_test(&session.inventory, session.content.as_ref().unwrap()).unwrap();
    session.execute_ui(UiIntent::Text { text: "msg a tail".into() }).unwrap();
    session.ui.as_mut().unwrap().fields.get_mut("chat/input").unwrap().select(6, 6).unwrap();
    session.execute_ui(tab()).unwrap();
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    let mut reader = Reader::new(&request);
    let id = reader.var().unwrap();
    assert_eq!(reader.string(32500).unwrap(), "/msg a");
    reader.done().unwrap();
    (session, id, responses, delivered)
}

fn response(id: i32, values: &[&str]) -> Vec<u8> {
    let mut packet = Writer::default();
    packet.var(id); packet.var(5); packet.var(1); packet.var(values.len() as i32);
    for value in values { packet.string(value); packet.byte(0); }
    packet.0
}

#[test]
fn command_suggestion_limit_oversized_preserves_editor_and_does_not_skip() {
    let (mut session, id, _responses, _delivered) = requested_session();
    let field = session.ui.as_mut().unwrap().fields.get_mut("chat/input").unwrap();
    field.compose("編".into(), Some([0, 3])).unwrap();
    let before = (field.text.clone(), field.cursor, field.anchor, field.composition.clone(), field.composition_cursor);
    let oversized = "é".repeat(124); // 248 bytes; assembled input is 258 bytes.
    let packet = response(id, &[&oversized, "valid"]);
    session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &packet).unwrap();
    let field = &session.ui.as_ref().unwrap().fields["chat/input"];
    assert_eq!((field.text.clone(), field.cursor, field.anchor, field.composition.clone(), field.composition_cursor), before);
    let ui = session.ui.as_mut().unwrap();
    ui.execute(tab(), &mut session.inventory, session.content.as_ref().unwrap()).unwrap();
    let field = &ui.fields["chat/input"];
    assert_eq!((field.text.clone(), field.cursor, field.anchor, field.composition.clone(), field.composition_cursor), before);
}

#[test]
fn command_suggestion_limit_near_limit_and_stale_correlation() {
    let (mut session, id, _responses, _delivered) = requested_session();
    let near_limit = "é".repeat(123); // 246 bytes; assembled input is exactly 256 bytes.
    let packet = response(id, &[&near_limit]);
    session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &response(id.wrapping_add(1), &["stale"])).unwrap();
    assert_eq!(session.ui.as_ref().unwrap().fields["chat/input"].text, "/msg a tail");
    session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &packet).unwrap();
    let field = &session.ui.as_ref().unwrap().fields["chat/input"];
    assert_eq!(field.text, format!("/msg {near_limit} tail"));
    assert_eq!(field.text.len(), 256);
    assert_eq!(field.cursor, 5 + near_limit.len());
    assert_eq!(field.anchor, field.cursor);
}

#[test]
fn command_suggestion_limit_malformed_range_remains_an_error() {
    let (mut session, id, _responses, _delivered) = requested_session();
    let mut packet = Writer::default();
    packet.var(id); packet.var(32767); packet.var(1); packet.var(1);
    packet.string("valid"); packet.byte(0);
    assert!(session.apply(Phase::Play, cb::COMMAND_SUGGESTIONS, &packet.0).is_err());
}

#[test]
fn command_suggestion_limit_network_response_does_not_disconnect() {
    let (mut session, id, responses, delivered) = requested_session();
    let field = session.ui.as_mut().unwrap().fields.get_mut("chat/input").unwrap();
    field.compose("編".into(), Some([0, 3])).unwrap();
    let before = (field.text.clone(), field.cursor, field.anchor, field.composition.clone(), field.composition_cursor);
    responses.send(response(id, &[&"é".repeat(124)])).unwrap();
    delivered.recv_timeout(Duration::from_secs(5)).unwrap();
    session.process_events();
    assert!(session.error.is_none(), "{:?}", session.error);
    assert_eq!(session.phase, "play");
    assert!(session.network.is_some());
    let field = &session.ui.as_ref().unwrap().fields["chat/input"];
    assert_eq!((field.text.clone(), field.cursor, field.anchor, field.composition.clone(), field.composition_cursor), before);
    assert_eq!(session.packet_counts.get(&cb::COMMAND_SUGGESTIONS), Some(&1));
}
