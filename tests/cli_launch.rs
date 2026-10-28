use std::{path::PathBuf, process::Command};

fn client() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_solaris-client"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("SOLARIS_CLIENT_") {
            command.env_remove(key);
        }
    }
    command
}

#[test]
fn launch_and_legacy_help_work_without_content() {
    for args in [
        vec!["--help"],
        vec!["play", "--help"],
        vec!["setup", "--help"],
    ] {
        let output = client().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let help = String::from_utf8(output.stdout).unwrap();
        assert!(help.contains("Minecraft Java 26.3"), "{help}");
        assert!(!help.contains("26.1.2"), "{help}");
        for flag in [
            "--server",
            "--data-dir",
            "--width",
            "--height",
            "--fullscreen",
            "--from-dist",
        ] {
            assert!(help.contains(flag), "help omitted {flag}: {help}");
        }
    }
}

#[test]
fn missing_data_fails_before_opening_a_window() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".analysis/cli-missing-content-regression");
    let output = client()
        .args(["--data-dir", root.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("missing content file"), "{error}");
    assert!(
        error.contains("facts/block_collision_shapes_26_3.bin"),
        "{error}"
    );
    assert!(error.contains(root.to_str().unwrap()), "{error}");
}

#[test]
fn direct_join_needs_no_explicit_profile_or_username() {
    for args in [
        vec!["--server", "localhost"],
        vec!["--server", "localhost", "--username", "CliTest"],
        vec!["play", "--server", "localhost", "--username", "CliTest"],
    ] {
        let output = client()
            .args(args)
            .args(["--data-dir", "/nonexistent/solaris-cli-direct"])
            .output()
            .unwrap();
        let error = String::from_utf8(output.stderr).unwrap();
        assert!(!output.status.success());
        assert!(
            error.contains("missing content file: /nonexistent/solaris-cli-direct/facts/"),
            "{error}"
        );
    }
}

#[test]
fn setup_names_missing_source_before_populating_root() {
    let root = std::env::temp_dir().join(format!("solaris-cli-setup-test-{}", std::process::id()));
    let output = client()
        .args([
            "setup",
            "--from-dist",
            "/nonexistent/solaris-dist",
            "--assets",
            "/nonexistent/prism-assets",
            "--data-dir",
            root.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(!root.exists());
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains("/nonexistent/solaris-dist/facts"), "{error}");
}

#[test]
fn xdg_default_and_explicit_environment_override_are_respected() {
    let root = std::env::temp_dir().join(format!("solaris-cli-xdg-{}", std::process::id()));
    let output = client().env("XDG_DATA_HOME", &root).output().unwrap();
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(
        error.contains(
            root.join("solaris-client/facts/block_collision_shapes_26_3.bin")
                .to_str()
                .unwrap()
        ),
        "{error}"
    );
    let override_path = root.join("custom-collision.bin");
    let output = client()
        .env("SOLARIS_CLIENT_COLLISION_SHAPES", &override_path)
        .output()
        .unwrap();
    let error = String::from_utf8(output.stderr).unwrap();
    assert!(error.contains(override_path.to_str().unwrap()), "{error}");
}

#[cfg(unix)]
#[test]
fn setup_links_existing_caches_and_is_repeatable() {
    let base = std::env::temp_dir().join(format!("solaris-cli-links-{}", std::process::id()));
    let dist = base.join("dist");
    let assets = base.join("prism");
    let root = base.join("installed");
    for dir in ["facts", "entities", "data", "resources", "assets"] {
        std::fs::create_dir_all(dist.join(dir)).unwrap();
    }
    std::fs::create_dir_all(assets.join("objects")).unwrap();
    std::fs::write(dist.join("assets/asset-index.json"), "{}").unwrap();
    for _ in 0..2 {
        let output = client()
            .arg("setup")
            .arg("--from-dist")
            .arg(&dist)
            .arg("--assets")
            .arg(&assets)
            .arg("--data-dir")
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for dir in ["facts", "entities", "data", "resources"] {
        assert_eq!(
            root.join(dir).canonicalize().unwrap(),
            dist.join(dir).canonicalize().unwrap()
        );
    }
    assert_eq!(
        root.join("assets/objects").canonicalize().unwrap(),
        assets.join("objects").canonicalize().unwrap()
    );
    assert_eq!(
        root.join("assets/asset-index.json").canonicalize().unwrap(),
        dist.join("assets/asset-index.json").canonicalize().unwrap()
    );
    std::fs::remove_dir_all(base).unwrap();
}
