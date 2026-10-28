//! Solaris launcher: downloads the game content, then starts `solaris-client`.
mod bootstrap;
mod config;
mod launch;
mod media;
mod mojang;
mod motion;
mod net;
mod paths;
mod pins;
mod progress;
mod theme;
mod theme_presets;
mod ui;

use anyhow::{Context, Result, bail};
use paths::Layout;
use progress::Reporter;
use std::path::PathBuf;

const HELP: &str = "Solaris launcher
Usage:
  solaris-launcher                      Open the launcher window
  solaris-launcher --headless-bootstrap [--force] [--play]
                                        Download game content without a window
                                        (--play then starts the client and waits)
  solaris-launcher --singleplayer-runtime
                                        Install the server data singleplayer needs
                                        (the client starts this before a local world;
                                        it downloads only on first use or a new pin)
Options:
  --data-dir DIR     Data root (default: $XDG_DATA_HOME/solaris-client, %APPDATA%\\solaris-client)
  --manifest URL     Solaris content manifest (default: built in, or launcher.json)
  --server HOST[:PORT]   Server for --play / --autoplay (default: selected server)
  --username NAME        Offline username for --play / --autoplay
  --autoplay         Window mode: start the install/Play flow immediately
  --tab play|settings|logs   Window mode: initial tab
  --force            Re-verify every download
  --help             This text";

pub struct Args {
    pub data_dir: Option<PathBuf>,
    pub manifest: Option<String>,
    pub server: Option<String>,
    pub username: Option<String>,
    pub headless: bool,
    pub singleplayer_runtime: bool,
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
        singleplayer_runtime: false,
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
            "--singleplayer-runtime" => out.singleplayer_runtime = true,
            "--play" => out.play = true,
            "--autoplay" => out.autoplay = true,
            "--force" => out.force = true,
            other => bail!("unknown argument {other} (see --help)"),
        }
    }
    Ok(Some(out))
}

/// `--manifest` wins over the URL saved in `launcher.json`.
fn options(args: &Args, config: &config::Config) -> bootstrap::Options {
    bootstrap::Options {
        manifest_url: args
            .manifest
            .clone()
            .unwrap_or_else(|| config.manifest_url.clone()),
    }
}

fn headless(args: &Args, layout: &Layout) -> Result<()> {
    let config = config::Config::load(&layout.config_file());
    let rep = Reporter::new(Some(&layout.log_file()), true);
    bootstrap::run(layout, &options(args, &config), args.force, &rep)?;
    if args.play {
        let spec = launch::Spec {
            root: layout.root.clone(),
            server: args
                .server
                .clone()
                .unwrap_or_else(|| config.address().to_owned()),
            username: args
                .username
                .clone()
                .unwrap_or_else(|| config.username().to_owned()),
            fullscreen: config::fullscreen(&config::load_client_settings(
                &layout.client_settings(),
            )),
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
    let result = if args.singleplayer_runtime {
        bootstrap::ensure_singleplayer_runtime(
            &layout,
            &options(&args, &config::Config::load(&layout.config_file())),
            &Reporter::new(Some(&layout.log_file()), true),
        )
    } else if args.headless {
        headless(&args, &layout)
    } else {
        ui::run(args, layout)
    };
    if let Err(error) = result {
        eprintln!("solaris-launcher: {error:#}");
        std::process::exit(1);
    }
}
