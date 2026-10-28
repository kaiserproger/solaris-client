//! Singleplayer: local worlds under `<data>/worlds/<name>` and the Solaris
//! server subprocess that serves one of them to the native client.
//!
//! Layout of one world directory:
//!
//! ```text
//! worlds/<name>/
//!   solaris-world.json   client-owned metadata (name, seed, mode, last played)
//!   server.toml          regenerated before every start
//!   ops.json             the local player as operator, regenerated before every start
//!   world/               native server world storage
//!   logs/server.log      server stdout+stderr of the latest run
//! ```
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(crate) const GAME_VERSION: &str = "26.1.2";
const AUTH_PLUGIN: &str = "solaris-auth";
const READY_MARKER: &str = "is listening";
const WORLD_FILE: &str = "solaris-world.json";
const TAIL_LINES: usize = 40;
const START_TIMEOUT: Duration = Duration::from_secs(180);
const STOP_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum GameMode {
    Survival,
    Creative,
}

impl GameMode {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Survival => "survival",
            Self::Creative => "creative",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Survival => "Survival",
            Self::Creative => "Creative",
        }
    }
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Survival => Self::Creative,
            Self::Creative => Self::Survival,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Difficulty {
    Peaceful,
    Easy,
    Normal,
    Hard,
}

impl Difficulty {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Peaceful => "peaceful",
            Self::Easy => "easy",
            Self::Normal => "normal",
            Self::Hard => "hard",
        }
    }
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Peaceful => "Peaceful",
            Self::Easy => "Easy",
            Self::Normal => "Normal",
            Self::Hard => "Hard",
        }
    }
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Peaceful => Self::Easy,
            Self::Easy => Self::Normal,
            Self::Normal => Self::Hard,
            Self::Hard => Self::Peaceful,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct WorldMeta {
    pub name: String,
    pub seed: i64,
    pub game_mode: GameMode,
    pub difficulty: Difficulty,
    pub created_ms: u64,
    pub last_played_ms: u64,
    /// The server has no per-world mode/difficulty config: the client applies
    /// them once with ordinary operator commands on the first join.
    pub setup_pending: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct World {
    pub dir_name: String,
    pub meta: WorldMeta,
    pub size: u64,
}

pub(crate) struct CreateSpec {
    pub name: String,
    /// Empty = random. Numbers are used as is, other text is Java-hashed like vanilla.
    pub seed: String,
    pub game_mode: GameMode,
    pub difficulty: Difficulty,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// The single local data root shared with the launcher.
pub(crate) fn data_root() -> Result<PathBuf, String> {
    if let Some(root) = std::env::var_os("SOLARIS_CLIENT_ROOT").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(root));
    }
    crate::content_paths::default_root()
}

pub(crate) fn worlds_dir(root: &Path) -> PathBuf {
    root.join("worlds")
}

/// Java `String.hashCode` over UTF-16 code units: vanilla's text-seed rule.
fn java_hash(text: &str) -> i32 {
    text.encode_utf16()
        .fold(0i32, |h, unit| h.wrapping_mul(31).wrapping_add(i32::from(unit)))
}

pub(crate) fn parse_seed(text: &str) -> Result<i64, String> {
    let text = text.trim();
    if text.is_empty() {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).map_err(|e| format!("random seed unavailable: {e}"))?;
        return Ok(i64::from_le_bytes(bytes));
    }
    Ok(text
        .parse::<i64>()
        .unwrap_or_else(|_| i64::from(java_hash(text))))
}

/// A folder name safe on every platform, derived from the display name.
fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(|c: char| c == '.' || c == ' ').to_owned();
    let reserved = ["con", "prn", "aux", "nul"];
    if cleaned.is_empty() || reserved.contains(&cleaned.to_ascii_lowercase().as_str()) {
        format!("World {cleaned}").trim().to_owned()
    } else {
        cleaned
    }
}

fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => dir_size(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

fn read_meta(dir: &Path) -> Option<WorldMeta> {
    serde_json::from_slice(&fs::read(dir.join(WORLD_FILE)).ok()?).ok()
}

pub(crate) fn write_meta(dir: &Path, meta: &WorldMeta) -> Result<(), String> {
    let path = dir.join(WORLD_FILE);
    let tmp = path.with_extension("json.tmp");
    fs::write(
        &tmp,
        serde_json::to_vec_pretty(meta).map_err(|e| e.to_string())?,
    )
    .and_then(|()| fs::rename(&tmp, &path))
    .map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// Worlds, most recently played first.
pub(crate) fn list_worlds(root: &Path) -> Vec<World> {
    let Ok(entries) = fs::read_dir(worlds_dir(root)) else {
        return Vec::new();
    };
    let mut worlds: Vec<World> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|entry| {
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            let meta = read_meta(&entry.path())?;
            Some(World {
                size: dir_size(&entry.path()),
                dir_name,
                meta,
            })
        })
        .collect();
    worlds.sort_by(|a, b| {
        b.meta
            .last_played_ms
            .cmp(&a.meta.last_played_ms)
            .then_with(|| a.dir_name.cmp(&b.dir_name))
    });
    worlds
}

pub(crate) fn create_world(root: &Path, spec: &CreateSpec) -> Result<World, String> {
    let name = spec.name.trim();
    if name.is_empty() {
        return Err("World name is required".into());
    }
    let seed = parse_seed(&spec.seed)?;
    let base = folder_name(name);
    let parent = worlds_dir(root);
    fs::create_dir_all(&parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let mut dir_name = base.clone();
    let mut n = 1;
    // `create_dir` is the atomic claim: never reuse an existing world folder.
    loop {
        match fs::create_dir(parent.join(&dir_name)) {
            Ok(()) => break,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                n += 1;
                dir_name = format!("{base} ({n})");
            }
            Err(e) => return Err(format!("cannot create world folder: {e}")),
        }
    }
    let now = now_ms();
    let meta = WorldMeta {
        name: name.to_owned(),
        seed,
        game_mode: spec.game_mode,
        difficulty: spec.difficulty,
        created_ms: now,
        last_played_ms: now,
        setup_pending: true,
    };
    write_meta(&parent.join(&dir_name), &meta)?;
    Ok(World {
        dir_name,
        meta,
        size: 0,
    })
}

/// Only ever deletes a direct child of `worlds/` that carries world metadata.
pub(crate) fn delete_world(root: &Path, dir_name: &str) -> Result<(), String> {
    if dir_name.is_empty() || dir_name.contains(['/', '\\']) || dir_name == ".." || dir_name == "." {
        return Err("invalid world folder".into());
    }
    let dir = worlds_dir(root).join(dir_name);
    if read_meta(&dir).is_none() {
        return Err(format!("{} is not a Solaris world", dir.display()));
    }
    fs::remove_dir_all(&dir).map_err(|e| format!("cannot delete {}: {e}", dir.display()))
}

pub(crate) fn open_folder(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    let opener = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    Command::new(opener)
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("cannot run {opener}: {e}"))
}

pub(crate) fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// `YYYY-MM-DD HH:MM` in UTC (no timezone database is available).
pub(crate) fn format_time(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

// ---------------------------------------------------------------------------
// Locating server pieces
// ---------------------------------------------------------------------------

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(Path::to_path_buf)
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

/// `solaris-v2` next to the client binary, else `<data>/servers/solaris-v2`.
pub(crate) fn server_binary(root: &Path) -> Result<PathBuf, String> {
    let name = exe_name("solaris-v2");
    let candidates = [
        exe_dir().map(|d| d.join(&name)),
        Some(root.join("servers").join(&name)),
    ];
    let candidates: Vec<PathBuf> = candidates.into_iter().flatten().collect();
    candidates
        .iter()
        .find(|p| p.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "Solaris server ({name}) not found. Place it next to the game or in {}",
                root.join("servers").display()
            )
        })
}

/// The bundled plugin pack: `plugins/` next to the client, else `<data>/servers/plugins`.
fn plugin_pack(root: &Path) -> Option<PathBuf> {
    [
        exe_dir().map(|d| d.join("plugins")),
        Some(root.join("servers/plugins")),
    ]
    .into_iter()
    .flatten()
    .find(|p| p.is_dir())
}

/// The local server's content cache (`vanilla_data_dir`), installed by the launcher
/// from the pinned Solaris content bundle.
fn content_cache(root: &Path) -> PathBuf {
    root.join("servers/content").join(GAME_VERSION)
}

fn content_ready(cache: &Path) -> bool {
    ["version.json", "reports/registries.json", "data/minecraft/recipe"]
        .iter()
        .all(|p| cache.join(p).exists())
}

/// Copy the plugin pack minus the login plugin (singleplayer is offline and
/// operator-owned) into `<data>/servers/sp-plugins`. Returns that directory.
fn sync_plugins(root: &Path) -> Result<Option<PathBuf>, String> {
    let Some(pack) = plugin_pack(root) else {
        return Ok(None);
    };
    let dest = root.join("servers/sp-plugins");
    let io = |e: std::io::Error| format!("plugin pack {}: {e}", pack.display());
    fs::create_dir_all(&dest).map_err(io)?;
    let mut wanted = Vec::new();
    for entry in fs::read_dir(&pack).map_err(io)?.filter_map(Result::ok) {
        let name = entry.file_name();
        if name == AUTH_PLUGIN || !entry.path().is_dir() {
            continue;
        }
        let target = dest.join(&name);
        fs::create_dir_all(&target).map_err(io)?;
        for file in fs::read_dir(entry.path()).map_err(io)?.filter_map(Result::ok) {
            let to = target.join(file.file_name());
            let source = file.path();
            let same = fs::metadata(&to)
                .and_then(|a| fs::metadata(&source).map(|b| (a, b)))
                .is_ok_and(|(a, b)| a.len() == b.len() && a.modified().ok() == b.modified().ok());
            if source.is_file() && !same {
                fs::copy(&source, &to).map_err(io)?;
            }
        }
        wanted.push(name);
    }
    for stale in fs::read_dir(&dest).map_err(io)?.filter_map(Result::ok) {
        if !wanted.contains(&stale.file_name()) {
            let _ = fs::remove_dir_all(stale.path());
        }
    }
    Ok(Some(dest))
}

// ---------------------------------------------------------------------------
// Configuration files
// ---------------------------------------------------------------------------

fn toml_str(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

pub(crate) struct ConfigSpec<'a> {
    pub name: &'a str,
    pub view_distance: u8,
    pub seed: i64,
    pub world_dir: &'a Path,
    pub vanilla_dir: &'a Path,
    pub plugins_dir: Option<&'a Path>,
}

/// `server.toml` for one local, offline, loopback-only world.
pub(crate) fn server_config(spec: &ConfigSpec) -> String {
    let view = spec.view_distance.clamp(2, 32);
    let mut out = format!(
        "[server]\nname = {}\nmotd = \"Solaris singleplayer\"\nmax_players = 8\n\
         view_distance = {view}\nsimulation_distance = {}\n\n\
         [network]\nbind_address = \"127.0.0.1\"\nport = 0\n\n\
         [auth]\nonline_mode = false\n\n\
         [dashboard]\nenabled = false\n\n",
        toml_str(spec.name),
        view.min(8),
    );
    if let Some(plugins) = spec.plugins_dir {
        out.push_str(&format!(
            "[plugins]\ndirectory = {}\n\n",
            toml_str(&plugins.to_string_lossy())
        ));
    }
    out.push_str(&format!(
        "[data]\nworld_dir = {}\nseed = {}\nvanilla_data_dir = {}\n",
        toml_str(&spec.world_dir.to_string_lossy()),
        spec.seed,
        toml_str(&spec.vanilla_dir.to_string_lossy()),
    ));
    out
}

/// `ops.json` making the local player a level-4 operator.
pub(crate) fn ops_json(username: &str) -> String {
    let uuid = crate::protocol::network::offline_uuid(username);
    let uuid = uuid::Uuid::from_bytes(uuid).hyphenated().to_string();
    serde_json::to_string_pretty(&serde_json::json!([{
        "uuid": uuid,
        "name": username,
        "level": 4,
        "bypassesPlayerLimit": false,
    }]))
    .expect("ops.json serializes")
}

// ---------------------------------------------------------------------------
// Server process
// ---------------------------------------------------------------------------

pub(crate) enum Event {
    /// The server is accepting connections; carries the port it actually bound.
    Listening(Option<u16>),
    /// One output stream reached EOF.
    Closed,
    Cancel,
}

/// Cancels a start in progress (before or while the server boots).
#[derive(Clone)]
pub(crate) struct Cancel {
    flag: Arc<AtomicBool>,
    events: Sender<Event>,
}

impl Cancel {
    pub(crate) fn cancel(&self) {
        self.flag.store(true, Ordering::Release);
        let _ = self.events.send(Event::Cancel);
    }
    pub(crate) fn cancelled(&self) -> bool {
        self.flag.load(Ordering::Acquire)
    }
}

pub(crate) fn start_channel() -> (Cancel, Receiver<Event>) {
    let (events, rx) = mpsc::channel();
    (
        Cancel {
            flag: Arc::new(AtomicBool::new(false)),
            events,
        },
        rx,
    )
}

/// A running singleplayer server. Dropping it stops it and waits for exit.
pub(crate) struct Server {
    child: Child,
    #[cfg(windows)]
    stdin: Option<std::process::ChildStdin>,
    tail: Arc<Mutex<VecDeque<String>>>,
    events: Receiver<Event>,
    pub(crate) port: u16,
    pub(crate) log_path: PathBuf,
    stopped: bool,
    /// Output streams that reached EOF (both closed = the process is exiting).
    closed: u8,
}

/// Drop terminal colour escapes (`ESC [ ... letter`) from a log line.
fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
        } else if chars.clone().next() == Some('[') {
            chars.next();
            for n in chars.by_ref() {
                if ('@'..='~').contains(&n) {
                    break;
                }
            }
        }
    }
    out
}

/// The last `lines` log lines, each cut to fit the error screen.
fn tail_text(tail: &Mutex<VecDeque<String>>, lines: usize) -> String {
    const WIDTH: usize = 60;
    let tail = tail.lock().unwrap_or_else(|e| e.into_inner());
    let skip = tail.len().saturating_sub(lines);
    tail.iter()
        .skip(skip)
        .map(|line| {
            if line.chars().count() > WIDTH {
                let cut: String = line.chars().take(WIDTH - 1).collect();
                format!("{cut}…")
            } else {
                line.clone()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn pump(
    reader: impl std::io::Read + Send + 'static,
    log: Arc<Mutex<fs::File>>,
    tail: Arc<Mutex<VecDeque<String>>>,
    events: Sender<Event>,
) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            if let Ok(mut file) = log.lock() {
                let _ = writeln!(file, "{line}");
            }
            if let Ok(mut tail) = tail.lock() {
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(strip_ansi(&line));
            }
            if line.contains(READY_MARKER) {
                let _ = events.send(Event::Listening(listening_port(&line)));
            }
        }
        let _ = events.send(Event::Closed);
    });
}

/// Port from the server's ready line, e.g. `... Solaris is listening (edge v2) addr=127.0.0.1:41567 ...`.
/// The server binds port 0 and reports the address it actually got.
fn listening_port(line: &str) -> Option<u16> {
    let line = strip_ansi(line);
    let rest = line.split_once("addr=")?.1;
    rest.split_whitespace()
        .next()?
        .parse::<std::net::SocketAddr>()
        .ok()
        .map(|addr| addr.port())
}

/// Test seam for session tests: serve a shell script as the "server".
#[cfg(all(test, unix))]
pub(crate) fn spawn_fake_server(root: &Path, script: &str) -> Result<Server, String> {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let binary = root.join("fake-server");
    fs::write(&binary, format!("#!/bin/sh\n{script}\n")).map_err(|e| e.to_string())?;
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    let (cancel, events) = start_channel();
    spawn_server(
        &binary,
        &root.join("server.toml"),
        root,
        &root.join("logs/server.log"),
        &cancel,
        events,
    )
}

/// Run the server binary on `config` and wait until it reports it is listening.
fn spawn_server(
    binary: &Path,
    config: &Path,
    cwd: &Path,
    log_path: &Path,
    cancel: &Cancel,
    events: Receiver<Event>,
) -> Result<Server, String> {
    if cancel.cancelled() {
        return Err("Cancelled".into());
    }
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let log = Arc::new(Mutex::new(
        fs::File::create(log_path)
            .map_err(|e| format!("cannot create {}: {e}", log_path.display()))?,
    ));
    let mut command = Command::new(binary);
    command
        .arg("--config")
        .arg(config)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1");
    // Unix stops the server with SIGTERM and needs no console; Windows has no
    // signal, so the console stays on stdin and receives `stop`.
    #[cfg(unix)]
    command.arg("--no-console").stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.stdin(Stdio::piped()).creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", binary.display()))?;
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    pump(
        child.stdout.take().expect("piped stdout"),
        log.clone(),
        tail.clone(),
        cancel.events.clone(),
    );
    pump(
        child.stderr.take().expect("piped stderr"),
        log,
        tail.clone(),
        cancel.events.clone(),
    );
    let mut server = Server {
        #[cfg(windows)]
        stdin: child.stdin.take(),
        child,
        tail,
        events,
        port: 0,
        log_path: log_path.to_owned(),
        stopped: false,
        closed: 0,
    };
    loop {
        match server.events.recv_timeout(START_TIMEOUT) {
            Ok(Event::Listening(Some(port))) => {
                server.port = port;
                return Ok(server);
            }
            Ok(Event::Listening(None)) => {
                let reason = server.crash_report("Server reported ready without a local address");
                let _ = server.stop();
                return Err(reason);
            }
            Ok(Event::Closed) => {
                server.closed += 1;
                if server.closed == 2 {
                    let _ = server.child.wait();
                    let reason = server.crash_report("Server exited during startup");
                    server.stopped = true;
                    return Err(reason);
                }
            }
            Ok(Event::Cancel) => {
                let _ = server.stop();
                return Err("Cancelled".into());
            }
            Err(_) => {
                let reason = server.crash_report("Server did not start in time");
                let _ = server.stop();
                return Err(reason);
            }
        }
    }
}

impl Server {
    /// Why the server is gone, with its last log lines, if it has exited.
    pub(crate) fn exited(&mut self) -> Option<String> {
        if self.stopped {
            return None;
        }
        match self.child.try_wait() {
            Ok(Some(status)) => {
                self.stopped = true;
                Some(self.crash_report(&format!("Server stopped unexpectedly ({status})")))
            }
            _ => None,
        }
    }

    /// Like `exited`, but when the output streams have not both closed yet,
    /// wait up to `timeout` for the process to finish exiting.
    pub(crate) fn exited_within(&mut self, timeout: Duration) -> Option<String> {
        let deadline = std::time::Instant::now() + timeout;
        while !self.stopped && self.closed < 2 {
            match self
                .events
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
            {
                Ok(Event::Closed) => self.closed += 1,
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        let _ = self.child.wait();
        self.exited()
    }

    fn crash_report(&self, headline: &str) -> String {
        format!(
            "{headline}.\nLog: {}\n{}",
            self.log_path.display(),
            tail_text(&self.tail, 3)
        )
    }

    fn request_stop(&mut self) {
        #[cfg(unix)]
        // SAFETY: plain signal delivery to the child we own and have not reaped.
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(windows)]
        if let Some(mut stdin) = self.stdin.take() {
            let _ = stdin.write_all(b"stop\n");
            let _ = stdin.flush();
        }
    }

    /// Graceful stop: the server checkpoints the world before exiting. `Err` carries the
    /// server-error report (with the log tail) when the stop had to be forced after
    /// `STOP_TIMEOUT` or the server exited unsuccessfully.
    pub(crate) fn stop(&mut self) -> Result<(), String> {
        if self.stopped {
            return Ok(());
        }
        self.stopped = true;
        let mut forced = false;
        if !matches!(self.child.try_wait(), Ok(Some(_))) {
            self.request_stop();
            while self.closed < 2 {
                match self.events.recv_timeout(STOP_TIMEOUT) {
                    Ok(Event::Closed) => self.closed += 1,
                    Ok(_) => {}
                    Err(_) => {
                        let _ = self.child.kill();
                        forced = true;
                        break;
                    }
                }
            }
        }
        let status = self.child.wait();
        if forced {
            return Err(self.crash_report(
                "Server did not stop in time and was killed; the world may not be fully saved",
            ));
        }
        match status {
            Ok(status) if status.success() => Ok(()),
            Ok(status) => Err(self.crash_report(&format!(
                "Server exited with {status} while stopping; the world may not be fully saved"
            ))),
            Err(error) => Err(self.crash_report(&format!(
                "Cannot confirm the server stopped ({error}); the world may not be fully saved"
            ))),
        }
    }
}

impl Drop for Server {
    /// Best effort: failures are only reported by an explicit `stop`.
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(crate) struct Launch<'a> {
    pub root: &'a Path,
    pub world_dir: &'a str,
    pub username: &'a str,
    pub view_distance: u8,
}

/// `solaris-launcher` next to this executable, else `$SOLARIS_LAUNCHER_BIN`.
fn launcher_binary() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("SOLARIS_LAUNCHER_BIN").map(PathBuf::from)
        && custom.is_file()
    {
        return Some(custom);
    }
    let beside = exe_dir()?.join(exe_name("solaris-launcher"));
    beside.is_file().then_some(beside)
}

/// Run `solaris-launcher --singleplayer-runtime`, which installs the local server's
/// content cache from the pinned Solaris content bundle (first singleplayer Play only;
/// afterwards it returns at once without network) and relay its progress lines.
fn install_content(
    launcher: &Path,
    root: &Path,
    cancel: &Cancel,
    progress: &dyn Fn(String),
) -> Result<(), String> {
    let (read, write) = std::io::pipe().map_err(|e| format!("cannot create pipe: {e}"))?;
    let mut child = Command::new(launcher)
        .arg("--singleplayer-runtime")
        .arg("--data-dir")
        .arg(root)
        .stdin(Stdio::null())
        .stdout(write.try_clone().map_err(|e| e.to_string())?)
        .stderr(write)
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", launcher.display()))?;
    let mut tail = VecDeque::new();
    for line in BufReader::new(read).lines().map_while(Result::ok) {
        if tail.len() == 6 {
            tail.pop_front();
        }
        tail.push_back(strip_ansi(&line));
        // Stage lines look like `[1/1] Downloading singleplayer server data`.
        if line.starts_with('[') {
            progress(format!("Preparing singleplayer (first run only): {line}"));
        }
        if cancel.cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Cancelled".into());
        }
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "Downloading the singleplayer server data failed ({status}).\n{}",
            tail.iter().cloned().collect::<Vec<_>>().join("\n")
        ));
    }
    Ok(())
}

/// The content cache the local server starts from. The launcher owns its pin, so it is
/// asked every time when present; without it an already installed cache is used as is.
fn ensure_content(
    root: &Path,
    cancel: &Cancel,
    progress: &dyn Fn(String),
) -> Result<PathBuf, String> {
    let cache = content_cache(root);
    match launcher_binary() {
        Some(launcher) => install_content(&launcher, root, cancel, progress)?,
        None if content_ready(&cache) => {}
        None => {
            return Err(
                "Singleplayer needs the server data that solaris-launcher downloads (first run \
                 only). It was not found next to the client: run \
                 `solaris-launcher --singleplayer-runtime` once, or set SOLARIS_LAUNCHER_BIN."
                    .to_owned(),
            );
        }
    }
    if !content_ready(&cache) {
        return Err(format!(
            "Singleplayer server data is incomplete ({})",
            cache.display()
        ));
    }
    Ok(cache)
}

/// Write the per-run files of a world and report its server directory.
pub(crate) fn prepare_world(
    launch: &Launch,
    vanilla: &Path,
    plugins: Option<&Path>,
) -> Result<(PathBuf, WorldMeta), String> {
    let dir = worlds_dir(launch.root).join(launch.world_dir);
    let mut meta = read_meta(&dir).ok_or_else(|| format!("{} is not a Solaris world", dir.display()))?;
    let world_storage = dir.join("world");
    let config = server_config(&ConfigSpec {
        name: &meta.name,
        view_distance: launch.view_distance,
        seed: meta.seed,
        world_dir: &world_storage,
        vanilla_dir: vanilla,
        plugins_dir: plugins,
    });
    let write = |name: &str, text: &str| {
        fs::write(dir.join(name), text).map_err(|e| format!("cannot write {name}: {e}"))
    };
    write("server.toml", &config)?;
    write("ops.json", &ops_json(launch.username))?;
    meta.last_played_ms = now_ms();
    write_meta(&dir, &meta)?;
    Ok((dir, meta))
}

/// Everything between pressing Play and a listening server.
pub(crate) fn start(
    launch: &Launch,
    cancel: &Cancel,
    events: Receiver<Event>,
    progress: &dyn Fn(String),
) -> Result<(Server, WorldMeta), String> {
    let binary = server_binary(launch.root)?;
    let vanilla = ensure_content(launch.root, cancel, progress)?;
    if cancel.cancelled() {
        return Err("Cancelled".into());
    }
    progress("Starting world server".into());
    let plugins = sync_plugins(launch.root)?;
    let (dir, meta) = prepare_world(launch, &vanilla, plugins.as_deref())?;
    let server = spawn_server(
        &binary,
        &dir.join("server.toml"),
        &dir,
        &dir.join("logs/server.log"),
        cancel,
        events,
    )?;
    Ok((server, meta))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("solaris-sp-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn spec(name: &str) -> CreateSpec {
        CreateSpec {
            name: name.into(),
            seed: "12345".into(),
            game_mode: GameMode::Creative,
            difficulty: Difficulty::Hard,
        }
    }

    #[test]
    fn seeds_follow_vanilla_rules() {
        assert_eq!(parse_seed("  42 ").unwrap(), 42);
        assert_eq!(parse_seed("-7").unwrap(), -7);
        // Java: "abc".hashCode()
        assert_eq!(parse_seed("abc").unwrap(), 96354);
        assert_ne!(parse_seed("").unwrap(), parse_seed("").unwrap());
    }

    #[test]
    fn config_binds_loopback_offline_with_requested_distances() {
        let text = server_config(&ConfigSpec {
            name: "My \"World\"",
            view_distance: 12,
            seed: -5,
            world_dir: Path::new("/data/worlds/w/world"),
            vanilla_dir: Path::new("/data/servers/content/26.1.2"),
            plugins_dir: Some(Path::new("/data/servers/sp-plugins")),
        });
        for line in [
            "name = \"My \\\"World\\\"\"",
            "view_distance = 12",
            "simulation_distance = 8",
            "bind_address = \"127.0.0.1\"",
            "port = 0",
            "online_mode = false",
            "enabled = false",
            "directory = \"/data/servers/sp-plugins\"",
            "world_dir = \"/data/worlds/w/world\"",
            "seed = -5",
            "vanilla_data_dir = \"/data/servers/content/26.1.2\"",
        ] {
            assert!(text.contains(line), "missing `{line}` in\n{text}");
        }
        assert!(!text.contains("require_plugin_login"));
        let no_plugins = server_config(&ConfigSpec {
            name: "w",
            view_distance: 1,
            seed: 0,
            world_dir: Path::new("w"),
            vanilla_dir: Path::new("v"),
            plugins_dir: None,
        });
        assert!(!no_plugins.contains("[plugins]"));
        assert!(no_plugins.contains("view_distance = 2"));
    }

    #[test]
    fn ops_json_lists_offline_uuid_at_level_four() {
        let json: serde_json::Value = serde_json::from_str(&ops_json("Player")).unwrap();
        assert_eq!(json[0]["name"], "Player");
        assert_eq!(json[0]["level"], 4);
        // Java: UUID.nameUUIDFromBytes("OfflinePlayer:Player")
        assert_eq!(json[0]["uuid"], "a01e3843-e521-3998-958a-f459800e4d11");
    }

    #[test]
    fn worlds_create_list_delete_and_never_collide() {
        let root = temp_root("worlds");
        let a = create_world(&root, &spec("My World")).unwrap();
        let b = create_world(&root, &spec("My World")).unwrap();
        assert_eq!(a.dir_name, "My World");
        assert_eq!(b.dir_name, "My World (2)");
        assert_eq!(a.meta.seed, 12345);
        assert!(a.meta.setup_pending);
        assert!(create_world(&root, &spec("  ")).is_err());
        let odd = create_world(&root, &spec("a/b:c")).unwrap();
        assert_eq!(odd.dir_name, "a_b_c");
        let listed = list_worlds(&root);
        assert_eq!(listed.len(), 3);
        // Not a world: has no metadata, so it is neither listed nor deletable.
        fs::create_dir_all(worlds_dir(&root).join("stray")).unwrap();
        assert_eq!(list_worlds(&root).len(), 3);
        assert!(delete_world(&root, "stray").is_err());
        assert!(delete_world(&root, "..").is_err());
        delete_world(&root, "My World (2)").unwrap();
        assert_eq!(list_worlds(&root).len(), 2);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn prepare_world_writes_config_ops_and_marks_played() {
        let root = temp_root("prepare");
        let world = create_world(&root, &spec("w")).unwrap();
        let launch = Launch {
            root: &root,
            world_dir: &world.dir_name,
            username: "Steve",
            view_distance: 6,
        };
        let (dir, meta) = prepare_world(&launch, &root.join("content"), None).unwrap();
        assert!(meta.last_played_ms >= world.meta.last_played_ms);
        let config = fs::read_to_string(dir.join("server.toml")).unwrap();
        assert!(config.contains("port = 0"));
        assert!(config.contains(&format!("seed = {}", 12345)));
        let ops = fs::read_to_string(dir.join("ops.json")).unwrap();
        assert!(ops.contains("Steve"));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_sync_drops_the_login_plugin() {
        let root = temp_root("plugins");
        let pack = root.join("servers/plugins");
        for name in ["solaris-auth", "solaris-essentials"] {
            fs::create_dir_all(pack.join(name)).unwrap();
            fs::write(pack.join(name).join("plugin.toml"), name).unwrap();
        }
        let dest = sync_plugins(&root).unwrap().unwrap();
        assert!(dest.join("solaris-essentials/plugin.toml").is_file());
        assert!(!dest.join("solaris-auth").exists());
        // A previously synced login plugin is removed again.
        fs::create_dir_all(dest.join("old")).unwrap();
        sync_plugins(&root).unwrap();
        assert!(!dest.join("old").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn listening_port_is_read_from_the_servers_ready_line() {
        let line = "2026-10-19T02:00:00Z  INFO solaris_v2: Solaris is listening (edge v2) \
                    addr=127.0.0.1:41567 name=My World";
        assert_eq!(listening_port(line), Some(41567));
        assert_eq!(listening_port("Solaris is listening (edge v2) addr=[::1]:7 name=w"), Some(7));
        assert_eq!(listening_port("Solaris is listening (edge v2)"), None);
        assert_eq!(listening_port("Solaris is listening addr=nonsense"), None);
    }

    #[test]
    fn log_tail_is_plain_and_short() {
        assert_eq!(
            strip_ansi("\u{1b}[2m2026\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m ready"),
            "2026  INFO ready"
        );
        let tail = Mutex::new(VecDeque::from([
            "one".to_owned(),
            "two".to_owned(),
            "x".repeat(100),
        ]));
        let text = tail_text(&tail, 2);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "two");
        assert_eq!(lines[1].chars().count(), 60);
        assert!(lines[1].ends_with('…'));
    }

    #[test]
    fn time_and_size_formatting() {
        assert_eq!(format_time(0), "1970-01-01 00:00");
        assert_eq!(format_time(1_789_800_000_000), "2026-09-19 06:40");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(3 * 1024 * 1024 + 512 * 1024), "3.5 MB");
    }

    #[cfg(unix)]
    fn fake_server(root: &Path, script: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = root.join("fake-server");
        fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[cfg(unix)]
    fn launch_fake(root: &Path, binary: &Path) -> Result<Server, String> {
        let (cancel, events) = start_channel();
        spawn_server(
            binary,
            &root.join("server.toml"),
            root,
            &root.join("logs/server.log"),
            &cancel,
            events,
        )
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_waits_for_listening_then_sigterm_lets_server_checkpoint() {
        let root = temp_root("life");
        let binary = fake_server(
            &root,
            "trap 'echo checkpoint written > \"$PWD/checkpoint\"; exit 0' TERM\n\
             echo 'Solaris is listening (edge v2) addr=127.0.0.1:41234 name=w'\n\
             while :; do sleep 1 & wait $!; done",
        );
        let mut server = launch_fake(&root, &binary).unwrap();
        assert!(server.exited().is_none());
        assert_eq!(server.port, 41234);
        server.stop().unwrap();
        // `stop` returned only after the process ran its TERM handler to the end.
        assert_eq!(
            fs::read_to_string(root.join("checkpoint")).unwrap().trim(),
            "checkpoint written"
        );
        let log = fs::read_to_string(root.join("logs/server.log")).unwrap();
        assert!(log.contains("is listening"));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn stop_reports_a_server_that_exits_unsuccessfully() {
        let root = temp_root("badstop");
        let binary = fake_server(
            &root,
            "trap 'echo save failed >&2; exit 3' TERM\n\
             echo 'is listening addr=127.0.0.1:41236'\n\
             while :; do sleep 1 & wait $!; done",
        );
        let mut server = launch_fake(&root, &binary).unwrap();
        let report = server.stop().unwrap_err();
        assert!(report.contains("may not be fully saved"), "{report}");
        assert!(report.contains("save failed"), "{report}");
        assert!(report.contains("server.log"), "{report}");
        assert!(server.stop().is_ok(), "a second stop is a no-op");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn startup_crash_reports_log_tail() {
        let root = temp_root("crash");
        let binary = fake_server(&root, "echo 'boom: world locked' >&2\nexit 3");
        let error = launch_fake(&root, &binary).err().unwrap();
        assert!(error.contains("exited during startup"), "{error}");
        assert!(error.contains("boom: world locked"), "{error}");
        assert!(error.contains("server.log"), "{error}");
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn crash_while_playing_is_reported_once() {
        let root = temp_root("midgame");
        let binary = fake_server(
            &root,
            "echo 'is listening addr=127.0.0.1:41235'\nread line < /dev/null\necho 'panic: tick failed' >&2\nexit 101",
        );
        let mut server = launch_fake(&root, &binary).unwrap();
        // The script exits right away; the report waits for the exit to finish.
        let message = server
            .exited_within(Duration::from_secs(10))
            .expect("exit is reported");
        assert!(message.contains("stopped unexpectedly"), "{message}");
        assert!(message.contains("panic: tick failed"), "{message}");
        assert!(server.exited().is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn cancel_during_startup_stops_the_process() {
        let root = temp_root("cancel");
        let binary = fake_server(
            &root,
            "trap 'exit 0' TERM\nwhile :; do sleep 1 & wait $!; done",
        );
        let (cancel, events) = start_channel();
        cancel.cancel();
        let error = spawn_server(
            &binary,
            &root.join("server.toml"),
            &root,
            &root.join("logs/server.log"),
            &cancel,
            events,
        )
        .err()
        .unwrap();
        assert_eq!(error, "Cancelled");
        let _ = fs::remove_dir_all(root);
    }
}
