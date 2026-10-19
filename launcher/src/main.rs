//! Solaris launcher: downloads the game content, then starts `solaris-client`.
mod bootstrap;
mod config;
mod derive;
mod families;
mod jdk;
mod launch;
mod mojang;
mod net;
mod paths;
mod pins;
mod progress;
mod ui;

use anyhow::{Context, Result, bail};
use paths::Layout;
use progress::Reporter;
use std::path::PathBuf;

const HELP: &str = "Solaris launcher
Usage:
  solaris-launcher                      Open the launcher window
  solaris-launcher --headless-bootstrap [--force] [--play]
                                        Download/derive content without a window
                                        (--play then starts the client and waits)
Options:
  --data-dir DIR     Data root (default: $XDG_DATA_HOME/solaris-client, %APPDATA%\\solaris-client)
  --manifest URL     Solaris content manifest (default: built in, or launcher.json)
  --server HOST[:PORT]   Server for --play / --autoplay (default: selected server)
  --username NAME        Offline username for --play / --autoplay
  --autoplay         Window mode: start the install/Play flow immediately
  --tab play|settings|logs   Window mode: initial tab
  --force            Re-verify every download and re-derive local data
  --help             This text";

pub struct Args {
    pub data_dir: Option<PathBuf>,
    pub manifest: Option<String>,
    pub server: Option<String>,
    pub username: Option<String>,
    pub headless: bool,
    pub play: bool,
    pub autoplay: bool,
    pub force: bool,
    pub tab: Option<String>,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Option<Args>> {
    let mut out = Args {
        data_dir: None,
        manifest: None,
        server: None,
        username: None,
        headless: false,
        play: false,
        autoplay: false,
        force: false,
        tab: None,
    };
    while let Some(arg) = args.next() {
        let mut value = |name: &str| args.next().with_context(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--help" | "-h" => return Ok(None),
            "--data-dir" => out.data_dir = Some(value("--data-dir")?.into()),
            "--manifest" => out.manifest = Some(value("--manifest")?),
            "--server" => out.server = Some(value("--server")?),
            "--username" => out.username = Some(value("--username")?),
            "--tab" => out.tab = Some(value("--tab")?),
            "--headless-bootstrap" => out.headless = true,
            "--play" => out.play = true,
            "--autoplay" => out.autoplay = true,
            "--force" => out.force = true,
            other => bail!("unknown argument {other} (see --help)"),
        }
    }
    Ok(Some(out))
}

fn headless(args: &Args, layout: &Layout) -> Result<()> {
    let mut config = config::Config::load(&layout.config_file());
    if let Some(url) = &args.manifest {
        config.manifest_url = url.clone();
    }
    let rep = Reporter::new(Some(&layout.log_file()), true);
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let printer = {
        let (rep, done) = (rep.clone(), done.clone());
        std::thread::spawn(move || {
            while !done.load(std::sync::atomic::Ordering::Relaxed) {
                for _ in 0..20 {
                    if done.load(std::sync::atomic::Ordering::Relaxed) {
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
                let s = rep.snapshot();
                if s.total > 0 {
                    println!(
                        "  ... {} {} / {} ({}/s)",
                        s.title,
                        progress::human_bytes(s.done),
                        progress::human_bytes(s.total),
                        progress::human_bytes(s.bytes_per_second as u64)
                    );
                }
            }
        })
    };
    let result = bootstrap::run(
        layout,
        &bootstrap::Options {
            manifest_url: config.manifest_url.clone(),
        },
        args.force,
        &rep,
    );
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = printer.join();
    result?;
    if args.play {
        let spec = launch::Spec {
            root: layout.root.clone(),
            server: args.server.clone().unwrap_or_else(|| config.address().to_owned()),
            username: args.username.clone().unwrap_or_else(|| config.username().to_owned()),
            fullscreen: config::fullscreen(&config::load_client_settings(&layout.client_settings())),
        };
        let mut child = launch::spawn(&spec, &rep)?;
        let status = child.wait()?;
        if !status.success() {
            bail!("solaris-client exited with {status}");
        }
    }
    Ok(())
}

fn main() {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(args)) => args,
        Ok(None) => {
            println!("{HELP}");
            return;
        }
        Err(error) => {
            eprintln!("solaris-launcher: {error:#}");
            std::process::exit(2);
        }
    };
    let root = match paths::data_root(args.data_dir.clone()) {
        Ok(root) => root,
        Err(error) => {
            eprintln!("solaris-launcher: {error}");
            std::process::exit(1);
        }
    };
    let layout = Layout::new(root);
    let result = if args.headless {
        headless(&args, &layout)
    } else {
        ui::run(args, layout)
    };
    if let Err(error) = result {
        eprintln!("solaris-launcher: {error:#}");
        std::process::exit(1);
    }
}
