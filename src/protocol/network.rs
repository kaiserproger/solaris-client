use super::{extract, frame, Reader, Result, Writer, MAX_FRAME};
use super::packet_ids::{login, configuration, play};
use parking_lot::Mutex;
use std::{
    io::{Read, Write},
    net::{Shutdown, TcpStream, ToSocketAddrs},
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::Duration,
};
/// Java's `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`: the identity an
/// offline-mode server derives from the login name.
pub fn offline_uuid(username: &str) -> [u8; 16] {
    let mut uuid = md5::compute(format!("OfflinePlayer:{username}")).0;
    uuid[6] = (uuid[6] & 15) | 48;
    uuid[8] = (uuid[8] & 63) | 128;
    uuid
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Login,
    Configuration,
    Play,
}
pub enum Event {
    Status(serde_json::Value),
    Packet(Phase, i32, Vec<u8>),
    Error(String),
}
impl Event {
    fn queued_bytes(&self) -> usize {
        // Charge the payload's allocation, the enum, and conservative channel/
        // allocator overhead even for tiny packets. Status JSON is bounded by
        // its 32767-byte input; charge a full frame for its parsed tree.
        let payload = match self {
            Self::Packet(_, _, body) => body.capacity(),
            Self::Status(_) => MAX_FRAME,
            Self::Error(error) => error.capacity(),
        };
        payload + std::mem::size_of::<Self>() + 64
    }
}
const MAX_INBOUND_BACKLOG_BYTES: usize = 64 * 1024 * 1024;
/// Outbound packets the session may queue ahead of the I/O owner. The session
/// emits a few packets per tick and the owner drains continuously, so reaching
/// this means the writer is genuinely stalled (TCP backpressure, the same
/// condition the write timeout reports) rather than a momentary burst.
const MAX_OUTBOUND_BACKLOG: usize = 8192;
struct ConnectionState {
    cancelled: AtomicBool,
    outbound_backlog: AtomicUsize,
    inbound_bytes: AtomicUsize,
    inbound_events: AtomicUsize,
    /// Disconnect/transfer ends this stream; never wait behind obsolete world data.
    urgent: Mutex<Option<Event>>,
    terminal_error: Mutex<Option<String>>,
    /// Latest setting; only the I/O owner chooses the phase-specific packet ID.
    view_distance: AtomicU8,
}
impl ConnectionState {
    fn publish(&self, events: &Sender<Event>, event: Event) -> Result<bool> {
        let bytes = event.queued_bytes();
        let previous = self.inbound_bytes.fetch_add(bytes, Ordering::AcqRel);
        if previous + bytes > MAX_INBOUND_BACKLOG_BYTES {
            self.inbound_bytes.fetch_sub(bytes, Ordering::AcqRel);
            return Err("inbound event backlog exceeded 64 MiB: session consumer stalled".into());
        }
        let pending = self.inbound_events.fetch_add(1, Ordering::AcqRel);
        events.send(event).map_err(|_| {
            self.inbound_bytes.fetch_sub(bytes, Ordering::AcqRel);
            self.inbound_events.fetch_sub(1, Ordering::AcqRel);
            "session gone".to_string()
        })?;
        Ok(pending == 0)
    }
}
pub struct Network {
    incoming: Receiver<Event>,
    outgoing: mpsc::Sender<(i32, Vec<u8>)>,
    state: Arc<ConnectionState>,
    socket: Option<TcpStream>,
    #[cfg(test)]
    worker: Option<thread::JoinHandle<()>>,
}
impl Network {
    pub fn connect(
        address: String,
        username: String,
        view_distance: u8,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<Self> {
        let (send, incoming) = mpsc::channel();
        let (outgoing, receive) = mpsc::channel();
        let state = Arc::new(ConnectionState {
            cancelled: AtomicBool::new(false),
            outbound_backlog: AtomicUsize::new(0),
            inbound_bytes: AtomicUsize::new(0),
            inbound_events: AtomicUsize::new(0),
            urgent: Mutex::new(None),
            terminal_error: Mutex::new(None),
            view_distance: AtomicU8::new(view_distance),
        });
        let stop = state.clone();
        let endpoint = endpoint(&address)?;
        let addresses: Vec<_> = endpoint
            .to_socket_addrs()
            .map_err(|e| e.to_string())?
            .take(8)
            .collect();
        let addr = *addresses.first().ok_or("address resolved no targets")?;
        let socket =
            TcpStream::connect_timeout(&addr, Duration::from_secs(5)).map_err(|e| e.to_string())?;
        socket
            .set_read_timeout(Some(Duration::from_millis(10)))
            .map_err(|e| e.to_string())?;
        socket
            .set_write_timeout(Some(Duration::from_secs(2)))
            .map_err(|e| e.to_string())?;
        socket.set_nodelay(true).map_err(|e| e.to_string())?;
        let control = socket.try_clone().map_err(|e| e.to_string())?;
        let worker = thread::Builder::new()
            .name("minecraft-io".into())
            .spawn(move || {
                let result = run(
                    socket,
                    addr,
                    endpoint,
                    username,
                    &send,
                    receive,
                    &stop,
                    wake.as_ref(),
                );
                if let Err(e) = result {
                    *stop.terminal_error.lock() = Some(e);
                    wake();
                }
            })
            .map_err(|e| e.to_string())?;
        #[cfg(not(test))]
        drop(worker);
        Ok(Self {
            incoming,
            outgoing,
            state,
            socket: Some(control),
            #[cfg(test)]
            worker: Some(worker),
        })
    }
    pub fn try_recv(&self) -> Result<Event, mpsc::TryRecvError> {
        if let Some(event) = self.state.urgent.lock().take() {
            return Ok(event);
        }
        match self.incoming.try_recv() {
            Ok(event) => {
                self.state.inbound_bytes.fetch_sub(event.queued_bytes(), Ordering::AcqRel);
                self.state.inbound_events.fetch_sub(1, Ordering::AcqRel);
                Ok(event)
            }
            Err(error) => match self.state.terminal_error.lock().take() {
                Some(error) => Ok(Event::Error(error)),
                None => Err(error),
            },
        }
    }
    /// A frame drains this snapshot, not an endless stream from a busy peer.
    pub(crate) fn pending_events(&self) -> usize {
        self.state.inbound_events.load(Ordering::Acquire)
            + usize::from(self.state.urgent.lock().is_some())
    }
    pub fn send(&self, id: i32, body: Vec<u8>) -> Result<()> {
        // Never fails because of a transient burst; only a stalled writer is fatal.
        let state = &self.state;
        if state.outbound_backlog.fetch_add(1, Ordering::AcqRel) >= MAX_OUTBOUND_BACKLOG {
            state.outbound_backlog.fetch_sub(1, Ordering::AcqRel);
            return Err("outbound backlog exceeded: network writer stalled".into());
        }
        self.outgoing.send((id, body)).map_err(|_| {
            state.outbound_backlog.fetch_sub(1, Ordering::AcqRel);
            "outbound queue closed: network thread ended".to_string()
        })
    }

    /// Publish a setting; the I/O owner sends the proper Configuration/Play form.
    pub fn set_view_distance(&self, view_distance: u8) {
        self.state
            .view_distance
            .store(view_distance, Ordering::Release);
    }
}
impl Drop for Network {
    fn drop(&mut self) {
        self.state.cancelled.store(true, Ordering::Release);
        if let Some(socket) = self.socket.take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}
fn endpoint(s: &str) -> Result<String> {
    if s.is_empty() || s.len() > 255 {
        return Err("invalid server address".into());
    }
    if s.starts_with('[') {
        Ok(if s.contains("]:") {
            s.into()
        } else {
            format!("{s}:25565")
        })
    } else if s.matches(':').count() == 0 {
        Ok(format!("{s}:25565"))
    } else if s.matches(':').count() == 1 {
        Ok(s.into())
    } else {
        Ok(format!("[{s}]:25565"))
    }
}
fn handshake(endpoint: &str, port: u16, next: i32) -> Vec<u8> {
    let mut w = Writer::default();
    w.var(777);
    let host = if endpoint.starts_with('[') {
        endpoint.split(']').next().unwrap().trim_start_matches('[')
    } else {
        endpoint.split(':').next().unwrap()
    };
    w.string(host);
    w.0.extend(port.to_be_bytes());
    w.var(next);
    w.0
}
/// Write every queued session packet. The I/O owner calls this between reads
/// and between delivered events so inbound floods cannot starve outbound.
fn flush_outbound(
    socket: &mut TcpStream,
    commands: &Receiver<(i32, Vec<u8>)>,
    state: &ConnectionState,
    threshold: Option<usize>,
) -> Result<()> {
    while let Ok((id, body)) = commands.try_recv() {
        state.outbound_backlog.fetch_sub(1, Ordering::AcqRel);
        write(socket, id, &body, threshold)?;
    }
    Ok(())
}
fn write(socket: &mut TcpStream, id: i32, body: &[u8], threshold: Option<usize>) -> Result<()> {
    socket
        .write_all(&frame(id, body, threshold)?)
        .map_err(|e| e.to_string())
}
fn run(
    mut socket: TcpStream,
    addr: std::net::SocketAddr,
    endpoint: String,
    username: String,
    events: &Sender<Event>,
    commands: Receiver<(i32, Vec<u8>)>,
    state: &ConnectionState,
    wake: &(dyn Fn() + Send + Sync),
) -> Result<()> {
    // Status uses its own connection, so its bytes cannot cross login framing.
    let mut status =
        TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    status
        .set_read_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    write(&mut status, 0, &handshake(&endpoint, addr.port(), 1), None)?;
    write(&mut status, 0, &[], None)?;
    let mut bytes = Vec::new();
    let mut tmp = [0; 8192];
    loop {
        let n = status.read(&mut tmp).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("status peer closed".into());
        }
        bytes.extend_from_slice(&tmp[..n]);
        if bytes.len() > MAX_FRAME {
            return Err("status buffer bound".into());
        }
        if let Some((id, body)) = extract(&mut bytes, None)? {
            if id != 0 {
                return Err("unexpected status packet".into());
            }
            let mut r = Reader::new(&body);
            let text = r.string(32767)?;
            r.done()?;
            let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            if v["version"]["protocol"].as_i64() != Some(777) {
                let version = &v["version"];
                let name = version["name"].as_str().unwrap_or("unknown version");
                let protocol = version["protocol"].as_i64()
                    .map_or_else(|| "unknown protocol".to_owned(), |n| format!("protocol {n}"));
                return Err(format!("Incompatible server version: {name} ({protocol}). This client supports Minecraft {} (protocol 777).", crate::singleplayer::GAME_VERSION));
            }
            state.publish(events, Event::Status(v))?;
            wake();
            break;
        }
    }
    write(&mut socket, 0, &handshake(&endpoint, addr.port(), 2), None)?;
    let mut login = Writer::default();
    login.string(&username);
    login.0.extend(offline_uuid(&username));
    write(&mut socket, login::serverbound::HELLO, &login.0, None)?;
    let mut phase = Phase::Login;
    let mut compression = None;
    let mut buffer = Vec::with_capacity(65536);
    let mut advertised_radius = None;
    while !state.cancelled.load(Ordering::Acquire) {
        flush_outbound(&mut socket, &commands, state, compression)?;
        let radius = state.view_distance.load(Ordering::Acquire);
        if phase != Phase::Login && advertised_radius != Some(radius) {
            let id = if phase == Phase::Configuration {
                configuration::serverbound::CLIENT_INFORMATION
            } else {
                play::serverbound::CLIENT_INFORMATION
            };
            write(&mut socket, id, &client_information(radius), compression)?;
            advertised_radius = Some(radius);
        }
        loop {
            let Some((id, body)) = extract(&mut buffer, compression)? else {
                break;
            };
            flush_outbound(&mut socket, &commands, state, compression)?;
            let original = phase;
            let radius = state.view_distance.load(Ordering::Acquire);
            if phase == Phase::Login {
                match id {
                    login::clientbound::HELLO => return Err("online-mode encryption requires official authentication; offline profile refused".into()),
                    login::clientbound::LOGIN_COMPRESSION => {
                        let mut r = Reader::new(&body);
                        let threshold = r.count(MAX_FRAME)?;
                        r.done()?;
                        compression = Some(threshold);
                    }
                    login::clientbound::LOGIN_FINISHED => {
                        write(&mut socket, login::serverbound::LOGIN_ACKNOWLEDGED, &[], compression)?;
                        phase = Phase::Configuration;
                        write(&mut socket, configuration::serverbound::CLIENT_INFORMATION, &client_information(radius), compression)?;
                        advertised_radius = Some(radius);
                    }
                    login::clientbound::CUSTOM_QUERY => {
                        let mut r = Reader::new(&body);
                        let query = r.var()?;
                        let _channel = r.string(32767)?;
                        let mut w = Writer::default();
                        w.var(query);
                        w.byte(0);
                        write(&mut socket, login::serverbound::CUSTOM_QUERY_ANSWER, &w.0, compression)?;
                    }
                    login::clientbound::COOKIE_REQUEST => {
                        let mut r = Reader::new(&body);
                        let key = r.string(32767)?;
                        let mut w = Writer::default();
                        w.string(&key);
                        w.byte(0);
                        write(&mut socket, login::serverbound::COOKIE_RESPONSE, &w.0, compression)?;
                    }
                    login::clientbound::LOGIN_DISCONNECT => {}
                    _ => return Err(format!("unknown mandatory Login packet {id}")),
                }
            }
            if original == Phase::Configuration {
                match id {
                    configuration::clientbound::FINISH_CONFIGURATION => {
                        write(
                            &mut socket,
                            configuration::serverbound::FINISH_CONFIGURATION,
                            &[],
                            compression,
                        )?;
                        phase = Phase::Play
                    }
                    configuration::clientbound::KEEP_ALIVE => write(
                        &mut socket,
                        configuration::serverbound::KEEP_ALIVE,
                        &body,
                        compression,
                    )?,
                    configuration::clientbound::PING => write(
                        &mut socket,
                        configuration::serverbound::PONG,
                        &body,
                        compression,
                    )?,
                    configuration::clientbound::SELECT_KNOWN_PACKS => write(
                        &mut socket,
                        configuration::serverbound::SELECT_KNOWN_PACKS,
                        &[0],
                        compression,
                    )?,
                    // A Loader probe is answered here, in packet order, so the hello
                    // is written before the KnownPacks reply the server waits behind.
                    configuration::clientbound::CUSTOM_PAYLOAD if matches!(Reader::new(&body).string(32767), Ok(ref channel) if channel == "solaris:loader/probe") => {
                        let mut w = Writer::default();
                        w.string("solaris:loader/hello");
                        write(&mut socket, configuration::serverbound::CUSTOM_PAYLOAD, &w.0, compression)?
                    }
                    configuration::clientbound::COOKIE_REQUEST => {
                        let mut r = Reader::new(&body);
                        let key = r.string(32767)?;
                        let mut w = Writer::default();
                        w.string(&key);
                        w.byte(0);
                        write(
                            &mut socket,
                            configuration::serverbound::COOKIE_RESPONSE,
                            &w.0,
                            compression,
                        )?
                    }
                    _ => {}
                }
            }
            if original == Phase::Play {
                match id {
                    play::clientbound::KEEP_ALIVE => write(&mut socket, play::serverbound::KEEP_ALIVE, &body, compression)?,
                    play::clientbound::PING => write(&mut socket, play::serverbound::PONG, &body, compression)?,
                    play::clientbound::START_CONFIGURATION => {
                        write(&mut socket, play::serverbound::CONFIGURATION_ACKNOWLEDGED, &[], compression)?;
                        phase = Phase::Configuration;
                        write(&mut socket, configuration::serverbound::CLIENT_INFORMATION, &client_information(radius), compression)?;
                        advertised_radius = Some(radius);
                    }
                    _ => {}
                }
            }
            let terminal = matches!(
                (original, id),
                (Phase::Login, login::clientbound::LOGIN_DISCONNECT) | (Phase::Configuration, configuration::clientbound::DISCONNECT | configuration::clientbound::TRANSFER) | (Phase::Play, play::clientbound::DISCONNECT | play::clientbound::TRANSFER)
            );
            if terminal {
                *state.urgent.lock() = Some(Event::Packet(original, id, body));
                wake();
                return Ok(());
            }
            if state.publish(events, Event::Packet(original, id, body))? {
                wake();
            }
        }
        match socket.read(&mut tmp) {
            Ok(0) => return Err("server closed connection".into()),
            Ok(n) => {
                if buffer.len() + n > MAX_FRAME + 5 {
                    return Err("network buffer bound".into());
                }
                buffer.extend_from_slice(&tmp[..n]);
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

fn client_information(view_distance: u8) -> Vec<u8> {
    let mut w = Writer::default();
    w.string("en_us");
    w.byte(view_distance);
    w.var(0);
    w.byte(1);
    w.byte(127);
    w.var(1);
    w.byte(0);
    w.byte(1);
    w.var(0);
    w.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn read_packet(socket: &mut TcpStream) -> (i32, Vec<u8>) {
        let mut header = Vec::new();
        loop {
            let mut byte = [0];
            socket.read_exact(&mut byte).unwrap();
            header.push(byte[0]);
            assert!(
                header.len() <= 5,
                "fixture received an oversized frame header"
            );
            if byte[0] & 128 == 0 {
                break;
            }
        }
        let size = Reader::new(&header).count(1024).unwrap();
        let mut payload = vec![0; size];
        socket.read_exact(&mut payload).unwrap();
        let mut reader = Reader::new(&payload);
        let id = reader.var().unwrap();
        (id, payload[reader.offset..].to_vec())
    }

    #[test]
    fn client_information_encodes_configured_radius() {
        // Regression: UI distance and the Configuration wire byte disagreed.
        for radius in [8u8, 12] {
            let body = client_information(radius);
            let mut expected = vec![5, b'e', b'n', b'_', b'u', b's'];
            expected.push(radius);
            expected.extend([0, 1, 127, 1, 0, 1, 0]);
            assert_eq!(
                body, expected,
                "view-distance byte must echo the configured radius"
            );
        }
    }

    #[test]
    fn wrong_version_reports_readable_client_and_server_versions() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let peer = thread::spawn(move || {
            let (_login, _) = listener.accept().unwrap();
            let (mut status, _) = listener.accept().unwrap();
            read_packet(&mut status);
            read_packet(&mut status);
            let mut response = Writer::default();
            response.string(r#"{"version":{"name":"1.21.4","protocol":769}}"#);
            write(&mut status, 0, &response.0, None).unwrap();
        });
        let mut network = Network::connect(
            address.to_string(), "VersionTest".into(), 8, Arc::new(|| {}),
        ).unwrap();
        peer.join().unwrap();
        network.worker.take().unwrap().join().unwrap();
        let Ok(Event::Error(error)) = network.try_recv() else { panic!("missing version error") };
        assert!(error.contains("26.3"), "{error}");
        assert!(error.contains("1.21.4"), "{error}");
        assert!(!error.contains("{\""), "{error}");
    }

    #[test]
    fn saturated_packet_queue_preserves_terminal_error_once() {
        // Declared protocol-error fixture, not a vanilla compatibility oracle.
        for malformed_frame in [false, true] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let peer = thread::spawn(move || {
                let (mut login, _) = listener.accept().unwrap();
                let (mut status, _) = listener.accept().unwrap();
                for socket in [&mut login, &mut status] {
                    socket
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                }
                assert_eq!(read_packet(&mut status).0, 0);
                assert_eq!(read_packet(&mut status), (0, Vec::new()));
                let mut response = Writer::default();
                response.string(r#"{"version":{"protocol":777}}"#);
                write(&mut status, 0, &response.0, None).unwrap();

                assert_eq!(read_packet(&mut login).0, 0);
                assert_eq!(read_packet(&mut login).0, 0);
                let mut success = Writer(vec![0; 16]);
                success.string("TerminalTest");
                success.var(0);
                write(&mut login, 2, &success.0, None).unwrap();
                assert_eq!(read_packet(&mut login), (3, Vec::new()));
                // Configuration ClientInformation must advertise the
                // configured radius (body[6] is the view-distance byte).
                let (info_id, info_body) = read_packet(&mut login);
                assert_eq!(info_id, 0);
                assert_eq!(info_body[6], 8);
                for token in 0i64..14 {
                    write(&mut login, 4, &token.to_be_bytes(), None).unwrap();
                    assert_eq!(read_packet(&mut login), (4, token.to_be_bytes().to_vec()));
                }
                if malformed_frame {
                    let mut invalid = Writer::default();
                    invalid.var(-1);
                    login.write_all(&invalid.0).unwrap();
                }
                login.shutdown(Shutdown::Write).unwrap();
            });
            let mut network = Network::connect(
                address.to_string(),
                "TerminalTest".into(),
                8,
                Arc::new(|| {}),
            )
            .unwrap();
            peer.join().unwrap();
            // Joining the real I/O worker proves all 16 sends completed and
            // termination ran while no consumer had freed a queue slot.
            network.worker.take().unwrap().join().unwrap();
            assert!(matches!(
                network.try_recv(),
                Ok(Event::Status(status)) if status["version"]["protocol"] == 777
            ));
            assert!(matches!(
                network.try_recv(),
                Ok(Event::Packet(Phase::Login, 2, body)) if {
                    let mut expected = Writer(vec![0; 16]);
                    expected.string("TerminalTest");
                    expected.var(0);
                    body == expected.0
                }
            ));
            for token in 0i64..14 {
                assert!(matches!(
                    network.try_recv(),
                    Ok(Event::Packet(Phase::Configuration, 4, body))
                        if body == token.to_be_bytes()
                ));
            }
            let expected_error = if malformed_frame {
                "frame length bound"
            } else {
                "server closed connection"
            };
            assert!(
                matches!(
                    network.try_recv(),
                    Ok(Event::Error(error)) if error == expected_error
                ),
                "terminal error was lost behind the saturated packet queue"
            );
            for _ in 0..2 {
                assert!(matches!(
                    network.try_recv(),
                    Err(mpsc::TryRecvError::Disconnected)
                ));
            }
        }
    }

    #[test]
    fn radius_updates_use_the_workers_current_wire_phase() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (ready, stages) = mpsc::channel();
        let peer = thread::spawn(move || {
            let (mut login, _) = listener.accept().unwrap();
            let (mut status, _) = listener.accept().unwrap();
            for socket in [&mut login, &mut status] {
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
            }
            assert_eq!(read_packet(&mut status).0, 0);
            assert_eq!(read_packet(&mut status), (0, Vec::new()));
            let mut response = Writer::default();
            response.string(r#"{"version":{"protocol":777}}"#);
            write(&mut status, 0, &response.0, None).unwrap();
            assert_eq!(read_packet(&mut login).0, 0);
            assert_eq!(read_packet(&mut login).0, 0);
            let mut success = Writer(vec![0; 16]);
            success.string("RadiusTest");
            success.var(0);
            write(&mut login, 2, &success.0, None).unwrap();
            assert_eq!(read_packet(&mut login), (3, Vec::new()));
            assert_eq!(read_packet(&mut login), (0, client_information(8)));

            ready.send(Phase::Configuration).unwrap();
            assert_eq!(read_packet(&mut login), (0, client_information(12)));
            write(&mut login, 3, &[], None).unwrap();
            assert_eq!(read_packet(&mut login), (3, Vec::new()));
            ready.send(Phase::Play).unwrap();
            assert_eq!(read_packet(&mut login), (0x0e, client_information(16)));

            // A reconfiguration must use the latest published radius, not the
            // startup value and not the session owner's potentially stale phase.
            write(&mut login, 120, &[], None).unwrap();
            assert_eq!(read_packet(&mut login), (16, Vec::new()));
            assert_eq!(read_packet(&mut login), (0, client_information(16)));
            login.shutdown(Shutdown::Write).unwrap();
        });
        let mut network =
            Network::connect(address.to_string(), "RadiusTest".into(), 8, Arc::new(|| {})).unwrap();
        assert_eq!(
            stages.recv_timeout(Duration::from_secs(5)).unwrap(),
            Phase::Configuration
        );
        network.set_view_distance(12);
        assert_eq!(
            stages.recv_timeout(Duration::from_secs(5)).unwrap(),
            Phase::Play
        );
        network.set_view_distance(16);
        peer.join().unwrap();
        network.worker.take().unwrap().join().unwrap();
    }

    #[test]
    fn outbound_burst_while_the_io_owner_is_busy_is_buffered_not_fatal() {
        // Regression: more than 128 packets queued while the I/O owner was busy
        // ended a live session with "outbound queue unavailable: sending on a
        // full channel". Nothing accepts the sockets, so the owner cannot drain.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let network = Network::connect(
            listener.local_addr().unwrap().to_string(),
            "Burst".into(),
            8,
            Arc::new(|| {}),
        )
        .unwrap();
        for _ in 0..1000 {
            network.send(0x1e, vec![0; 8]).unwrap();
        }
    }

    #[test]
    fn inbound_byte_limit_releases_capacity_and_never_blocks() {
        let (send, incoming) = mpsc::channel();
        let (outgoing, _) = mpsc::channel();
        let network = Network {
            incoming,
            outgoing,
            state: Arc::new(ConnectionState {
                cancelled: AtomicBool::new(false),
                outbound_backlog: AtomicUsize::new(0),
                inbound_bytes: AtomicUsize::new(0),
                inbound_events: AtomicUsize::new(0),
                urgent: Mutex::new(None),
                terminal_error: Mutex::new(None),
                view_distance: AtomicU8::new(8),
            }),
            socket: None,
            worker: None,
        };
        for index in 0..7 {
            assert_eq!(
                network.state.publish(&send, Event::Packet(Phase::Play, 8, vec![0; MAX_FRAME])).unwrap(),
                index == 0,
                "wake only the transition from an empty queue"
            );
        }
        let bytes = network.state.inbound_bytes.load(Ordering::Acquire);
        assert!(network.state.publish(&send, Event::Packet(Phase::Play, 8, vec![0; MAX_FRAME])).unwrap_err().contains("inbound event backlog exceeded"));
        assert_eq!(network.state.inbound_bytes.load(Ordering::Acquire), bytes);
        assert_eq!(network.pending_events(), 7);
        assert!(network.try_recv().is_ok());
        assert!(network.state.publish(&send, Event::Packet(Phase::Play, 8, vec![0; MAX_FRAME])).is_ok());
        for _ in 0..7 { assert!(network.try_recv().is_ok()); }
        assert_eq!(network.pending_events(), 0);
        assert_eq!(network.state.inbound_bytes.load(Ordering::Acquire), 0);
    }

    #[test]
    fn disconnect_and_transfer_bypass_world_backlog() {
        for (phase, id) in [(Phase::Configuration, 2), (Phase::Configuration, 12), (Phase::Play, 32), (Phase::Play, 132)] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let peer = thread::spawn(move || {
                let (mut login, _) = listener.accept().unwrap();
                let (mut status, _) = listener.accept().unwrap();
                login.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                read_packet(&mut status);
                read_packet(&mut status);
                let mut response = Writer::default();
                response.string(r#"{"version":{"protocol":777}}"#);
                write(&mut status, 0, &response.0, None).unwrap();
                read_packet(&mut login);
                read_packet(&mut login);
                let mut success = Writer(vec![0; 16]);
                success.string("UrgentTest");
                success.var(0);
                write(&mut login, 2, &success.0, None).unwrap();
                assert_eq!(read_packet(&mut login).0, 3);
                assert_eq!(read_packet(&mut login).0, 0);
                for token in 0i64..32 {
                    write(&mut login, 4, &token.to_be_bytes(), None).unwrap();
                    assert_eq!(read_packet(&mut login).0, 4);
                }
                if phase == Phase::Play {
                    write(&mut login, 3, &[], None).unwrap();
                    assert_eq!(read_packet(&mut login).0, 3);
                }
                write(&mut login, id, &[1, 2, 3], None).unwrap();
            });
            let mut network = Network::connect(address.to_string(), "UrgentTest".into(), 8, Arc::new(|| {})).unwrap();
            peer.join().unwrap();
            network.worker.take().unwrap().join().unwrap();
            assert!(matches!(network.try_recv(), Ok(Event::Packet(received, packet_id, body))
                if received == phase && packet_id == id && body == [1, 2, 3]));
        }
    }
}
