use super::{artifact_root, content_paths, headless, local_config, App, Options};
use std::{path::PathBuf, sync::Arc};
use winit::event_loop::EventLoop;

pub(crate) const HELP: &str = "Solaris native client (Minecraft Java 26.1.2 offline preview)
Usage:
  solaris-client                         Open the title screen
  solaris-client --server HOST[:PORT] [--username NAME]
                 [--width W --height H] [--fullscreen] [--data-dir DIR]
  solaris-client setup --from-dist DIR --assets PRISM_ASSETS [--data-dir DIR]
  solaris-client play [launch options] [--profile fixture|offline-test]
                 [--automation --mcp-http 127.0.0.1:0] [--headless|--windowed]
                 [--artifact-dir DIR] [--runtime-dir DIR]
  solaris-client doctor [--json]
  solaris-client mcp attach --instance ID --stdio [--descriptor FILE]

Default data: Linux `$XDG_DATA_HOME/solaris-client` (or `~/.local/share/solaris-client`),
Windows `%APPDATA%\\solaris-client`, macOS `~/Library/Application Support/solaris-client`;
relative --artifact-dir/--runtime-dir resolve there (defaults artifacts/, runtime/).
SOLARIS_CLIENT_* environment variables optionally override individual content paths.
Default username: Player. Server port defaults to 25565. --help / -h prints this text.
WASD move, Space jump, Shift sneak, Ctrl sprint, mouse look, left break/right use,
1–9 hotbar, E inventory, Escape menu.";

struct Parsed {
    options: Options,
    data_dir: Option<PathBuf>,
    explicit_profile: bool,
    help: bool,
}

fn dimension(value: Option<String>, name: &str) -> Result<u32, String> {
    value
        .ok_or_else(|| format!("missing_{name}"))?
        .parse::<u32>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| format!("invalid_{name}: expected a positive pixel count"))
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Parsed, String> {
    let mut parsed = Parsed {
        options: Options {
            automation: false,
            bind: "127.0.0.1:0".parse().unwrap(),
            artifacts: PathBuf::from("artifacts"),
            headless: None,
            runtime_dir: PathBuf::from("runtime"),
            profile: "fixture".into(),
            server: None,
            direct_join: false,
            username: None,
            width: 1280,
            height: 720,
            fullscreen: false,
        },
        data_dir: None,
        explicit_profile: false,
        help: false,
    };
    let options = &mut parsed.options;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                parsed.help = true;
                return Ok(parsed);
            }
            "--automation" => options.automation = true,
            "--mcp-http" => {
                options.bind = args
                    .next()
                    .ok_or("missing_mcp_address")?
                    .parse()
                    .map_err(|e| format!("invalid_mcp_address: {e}"))?
            }
            "--artifact-dir" => {
                options.artifacts = PathBuf::from(args.next().ok_or("missing_artifact_dir")?)
            }
            "--runtime-dir" => {
                options.runtime_dir = PathBuf::from(args.next().ok_or("missing_runtime_dir")?)
            }
            "--headless" | "--windowed" => {
                if options.headless.is_some() {
                    return Err("choose_one_of_--headless_or_--windowed".into());
                }
                options.headless = Some(arg == "--headless");
            }
            "--profile" => {
                options.profile = args.next().ok_or("missing_profile")?;
                parsed.explicit_profile = true;
            }
            "--server" => {
                options.server = Some(args.next().ok_or("missing_server")?);
                options.direct_join = true;
            }
            "--username" => options.username = Some(args.next().ok_or("missing_username")?),
            "--data-dir" => {
                parsed.data_dir = Some(PathBuf::from(args.next().ok_or("missing_data_dir")?))
            }
            "--width" => options.width = dimension(args.next(), "width")?,
            "--height" => options.height = dimension(args.next(), "height")?,
            "--fullscreen" => options.fullscreen = true,
            _ => return Err(format!("unknown_play_argument: {arg}")),
        }
    }
    if !options.bind.ip().is_loopback() {
        return Err("mcp_loopback_required".into());
    }
    Ok(parsed)
}

pub(crate) fn play(args: impl Iterator<Item = String>) -> Result<(), String> {
    // Help does not require installed content, a display, or config mutation.
    let parsed = parse(args)?;
    if parsed.help {
        println!("{HELP}");
        return Ok(());
    }
    let Parsed {
        mut options,
        data_dir,
        explicit_profile,
        ..
    } = parsed;
    let root = data_dir
        .map(Ok)
        .unwrap_or_else(content_paths::default_root)?;
    content_paths::apply_defaults(&root);
    let local = local_config::apply_exe_defaults();
    options.server = options.server.or(local.server);
    options.username = options
        .username
        .or(local.username)
        .or_else(|| Some("Player".into()));
    options.headless = options.headless.or(local.windowed.map(|v| !v));
    if !explicit_profile {
        options.profile = if options.direct_join {
            "offline-test".into()
        } else {
            local.profile.unwrap_or_else(|| "fixture".into())
        };
    }
    if options.profile != "fixture" && options.profile != "offline-test" {
        return Err("supported_profiles_are_fixture_and_offline-test".into());
    }
    if options.direct_join && options.profile != "offline-test" {
        return Err("network_play_requires_--profile_offline-test".into());
    }
    let headless = options
        .headless
        .unwrap_or(options.profile == "offline-test" && options.automation);
    // Fixture headless mode deliberately needs no Minecraft resources.
    if !headless || options.profile == "offline-test" {
        content_paths::validate(!headless)?;
    }
    options.artifacts = artifact_root(&options.artifacts, &root)?;
    options.runtime_dir = artifact_root(&options.runtime_dir, &root)?;
    if headless {
        return headless::run(options);
    }
    let event_loop = EventLoop::<()>::with_user_event()
        .build()
        .map_err(|e| e.to_string())?;
    let proxy = event_loop.create_proxy();
    let mut app = App::new(
        options,
        Arc::new(move || {
            let _ = proxy.send_event(());
        }),
    )?;
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

pub(crate) fn setup(mut args: impl Iterator<Item = String>) -> Result<(), String> {
    let mut dist = None;
    let mut assets = None;
    let mut root = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(());
            }
            "--from-dist" => dist = Some(PathBuf::from(args.next().ok_or("missing_from_dist")?)),
            "--assets" => assets = Some(PathBuf::from(args.next().ok_or("missing_assets")?)),
            "--data-dir" => root = Some(PathBuf::from(args.next().ok_or("missing_data_dir")?)),
            _ => return Err(format!("unknown_setup_argument: {arg}")),
        }
    }
    let dist = dist.ok_or("setup requires --from-dist DIR")?;
    let assets = assets.ok_or("setup requires --assets PRISM_ASSETS")?;
    let root = root.map(Ok).unwrap_or_else(content_paths::default_root)?;
    content_paths::setup(&dist, &assets, &root)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parsed(args: &[&str]) -> Result<Parsed, String> {
        parse(args.iter().map(|s| (*s).to_owned()))
    }
    #[test]
    fn empty_launch_opens_title_not_direct_join() {
        let parsed = parsed(&[]).unwrap();
        assert!(!parsed.options.direct_join);
        assert_eq!(parsed.options.headless, None);
    }
    #[test]
    fn direct_join_window_flags_and_legacy_automation_parse_together() {
        let parsed = parsed(&[
            "--server",
            "localhost",
            "--username",
            "CliTest",
            "--width",
            "1600",
            "--height",
            "900",
            "--fullscreen",
            "--automation",
            "--profile",
            "offline-test",
            "--windowed",
        ])
        .unwrap();
        assert!(parsed.options.direct_join);
        assert_eq!(parsed.options.server.as_deref(), Some("localhost"));
        assert_eq!(parsed.options.username.as_deref(), Some("CliTest"));
        assert_eq!((parsed.options.width, parsed.options.height), (1600, 900));
        assert!(parsed.options.fullscreen);
        assert!(parsed.options.automation);
        assert_eq!(parsed.options.headless, Some(false));
    }
    #[test]
    fn invalid_dimensions_and_conflicting_window_flags_fail() {
        for args in [
            vec!["--width", "0"],
            vec!["--height", "no"],
            vec!["--headless", "--windowed"],
        ] {
            assert!(parsed(&args).is_err());
        }
    }
}
