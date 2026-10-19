//! Starting the native client as a child process.
use crate::progress::Reporter;
use anyhow::{Context, Result, bail};
use std::{
    io::{BufRead, BufReader},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
};

pub struct Spec {
    pub root: PathBuf,
    pub server: String,
    pub username: String,
    pub fullscreen: bool,
}

/// `solaris-client` next to this executable, else `$SOLARIS_CLIENT_BIN`, else PATH.
pub fn client_binary() -> Result<PathBuf> {
    let name = if cfg!(windows) { "solaris-client.exe" } else { "solaris-client" };
    if let Some(custom) = std::env::var_os("SOLARIS_CLIENT_BIN").map(PathBuf::from) {
        if custom.is_file() {
            return Ok(custom);
        }
        bail!("SOLARIS_CLIENT_BIN={} is not a file", custom.display());
    }
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        let beside = dir.join(name);
        if beside.is_file() {
            return Ok(beside);
        }
    }
    Ok(PathBuf::from(name))
}

fn pipe(reader: impl std::io::Read + Send + 'static, rep: Reporter) {
    std::thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(std::result::Result::ok) {
            rep.log(format!("[client] {line}"));
        }
    });
}

pub fn spawn(spec: &Spec, rep: &Reporter) -> Result<Child> {
    let binary = client_binary()?;
    let mut command = Command::new(&binary);
    command
        .arg("--data-dir")
        .arg(&spec.root)
        .arg("--server")
        .arg(&spec.server)
        .arg("--username")
        .arg(&spec.username)
        .env("SOLARIS_CLIENT_SETTINGS", spec.root.join("client-settings.json"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if spec.fullscreen {
        command.arg("--fullscreen");
    }
    rep.log(format!(
        "Starting {} --server {} --username {}",
        binary.display(),
        spec.server,
        spec.username
    ));
    let mut child = command
        .spawn()
        .with_context(|| format!("cannot start {} (is solaris-client next to the launcher?)", binary.display()))?;
    pipe(child.stdout.take().unwrap(), rep.clone());
    pipe(child.stderr.take().unwrap(), rep.clone());
    Ok(child)
}
