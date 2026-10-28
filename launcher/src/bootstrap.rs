//! First-run content bootstrap. Every stage is idempotent and skips quickly
//! when its result is already installed, so Play after a clean install does
//! not touch the network.
use crate::{
    mojang,
    net::{self, Check, Job},
    paths::{self, GAME_VERSION, Layout},
    pins,
    progress::{Cancelled, Reporter},
};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};

const STAGES: usize = 5;

/// What is installed, so unchanged stages can be skipped.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Installed {
    pub game_version: String,
    pub client_sha1: String,
    /// Bundle name -> sha256 of the installed archive.
    pub bundles: std::collections::BTreeMap<String, String>,
    pub resources_for: String,
}

impl Installed {
    pub fn load(layout: &Layout) -> Self {
        fs::read_to_string(layout.installed_file())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }
    fn save(&self, layout: &Layout) -> Result<()> {
        let tmp = layout.installed_file().with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, layout.installed_file())?;
        Ok(())
    }
}

/// Play can start immediately: everything required is on disk and recorded.
pub fn is_ready(layout: &Layout) -> bool {
    let installed = Installed::load(layout);
    installed.game_version == GAME_VERSION
        && !installed.bundles.is_empty()
        && !installed.resources_for.is_empty()
        && paths::missing(&layout.root).is_empty()
}

pub struct Options {
    pub manifest_url: String,
}

/// One bundle kind and the exact archive this launcher release accepts for it.
struct Pin {
    kind: &'static str,
    name: &'static str,
    sha256: &'static str,
}

/// Client data tables (`facts/`, `data/vanilla/reports/`), installed by every Play.
const FACTS: Pin = Pin {
    kind: "facts",
    name: pins::FACTS_NAME,
    sha256: pins::FACTS_SHA256,
};

/// Singleplayer only: the complete server content cache that
/// `solaris-v2 content import` derives, the local server's `vanilla_data_dir`.
const CONTENT: Pin = Pin {
    kind: "content",
    name: pins::CONTENT_NAME,
    sha256: pins::CONTENT_SHA256,
};

#[derive(Debug)]
struct Bundle {
    url: String,
    size: u64,
}

fn fetch_bundle(client: &Client, manifest_url: &str, pin: &Pin) -> Result<Bundle> {
    let bytes = net::fetch_bytes(client, manifest_url, None)
        .with_context(|| format!("cannot reach the Solaris content manifest at {manifest_url}"))?;
    parse_bundle(&bytes, pin)
}

/// Pure manifest handling, separate from the network fetch for testing.
fn parse_bundle(bytes: &[u8], pin: &Pin) -> Result<Bundle> {
    let manifest: Value = serde_json::from_slice(bytes).context("parsing content manifest")?;
    if manifest["game_version"] != GAME_VERSION {
        bail!(
            "content manifest is for {}, launcher needs {GAME_VERSION}",
            manifest["game_version"]
        );
    }
    // The manifest travels over plain HTTP: only the pinned archive is accepted.
    let entry = manifest["bundles"]
        .as_array()
        .context("manifest.bundles")?
        .iter()
        .find(|entry| entry["kind"] == pin.kind && entry["name"] == pin.name)
        .ok_or_else(|| {
            anyhow!(
                "content manifest does not list the {} bundle {} this launcher is pinned to; update the launcher",
                pin.kind,
                pin.name
            )
        })?;
    let sha256 = entry["sha256"]
        .as_str()
        .context("manifest bundle missing sha256")?;
    if !sha256.eq_ignore_ascii_case(pin.sha256) {
        bail!(
            "refusing content bundle {}: manifest sha256 {sha256} does not match the hash pinned in this launcher ({})",
            pin.name,
            pin.sha256
        );
    }
    Ok(Bundle {
        url: entry["url"]
            .as_str()
            .context("manifest bundle missing url")?
            .to_owned(),
        size: entry["size"].as_u64().context("manifest bundle size")?,
    })
}

fn is_installed(installed: &Installed, pin: &Pin) -> bool {
    installed
        .bundles
        .get(pin.name)
        .is_some_and(|sha256| sha256 == pin.sha256)
}

/// Download the pinned archive (hash-checked, resumable) and unpack it into a
/// fresh staging directory on the data root's filesystem.
fn download_and_unpack(
    client: &Client,
    layout: &Layout,
    manifest_url: &str,
    pin: &Pin,
    rep: &Reporter,
) -> Result<PathBuf> {
    let bundle = fetch_bundle(client, manifest_url, pin)?;
    let archive = layout.cache_dir().join(format!("{}.tar.zst", pin.name));
    rep.set_total(bundle.size);
    net::download(
        client,
        &Job {
            url: bundle.url,
            dest: archive.clone(),
            check: Check::Sha256(pin.sha256.to_owned()),
            size: bundle.size,
        },
        rep,
    )?;
    let staging = layout.join(".staging-bundle");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let decoder = zstd::stream::read::Decoder::new(fs::File::open(&archive)?)?;
    tar::Archive::new(decoder)
        .unpack(&staging)
        .with_context(|| format!("unpacking {}", pin.name))?;
    Ok(staging)
}

fn replace_dir(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    if to.exists() {
        fs::remove_dir_all(to)?;
    }
    fs::rename(from, to).with_context(|| format!("installing {}", to.display()))
}

fn install_facts(layout: &Layout, staging: &Path, rep: &Reporter) -> Result<()> {
    for entry in fs::read_dir(staging)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        match name.as_str() {
            "facts" => replace_dir(&entry.path(), &layout.join("facts"))?,
            "data" => {
                for child in fs::read_dir(entry.path())? {
                    let child = child?;
                    replace_dir(&child.path(), &layout.join("data").join(child.file_name()))?;
                }
            }
            "bundle.json" => {}
            other => rep.log(format!("ignoring unexpected bundle entry {other}")),
        }
    }
    Ok(())
}

fn extract_resources(layout: &Layout, rep: &Reporter) -> Result<()> {
    let mut zip = zip::ZipArchive::new(fs::File::open(layout.client_jar())?)?;
    let mut total = 0;
    for index in 0..zip.len() {
        let entry = zip.by_index(index)?;
        if entry.is_file() && entry.name().starts_with("assets/") {
            total += entry.size();
        }
    }
    rep.set_total(total);
    let root = layout.join("resources");
    let mut buffer = Vec::new();
    for index in 0..zip.len() {
        rep.check_cancel()
            .map_err(|_: Cancelled| net::UserCancelled)?;
        let mut entry = zip.by_index(index)?;
        if !entry.is_file() || !entry.name().starts_with("assets/") {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            bail!(
                "client jar entry {} escapes the extraction root",
                entry.name()
            );
        };
        let target = root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        buffer.clear();
        entry.read_to_end(&mut buffer)?;
        if fs::read(&target).ok().as_deref() != Some(buffer.as_slice()) {
            fs::File::create(&target)?.write_all(&buffer)?;
        }
        rep.add_done(buffer.len() as u64);
    }
    Ok(())
}

/// Run every stage. `force` re-verifies/re-derives even when recorded as done.
pub fn run(layout: &Layout, options: &Options, force: bool, rep: &Reporter) -> Result<()> {
    fs::create_dir_all(&layout.root)?;
    let client = net::client()?;
    let mut installed = Installed::load(layout);

    rep.stage(1, STAGES, "Checking Mojang version metadata");
    let release = mojang::resolve(&client, layout, rep)?;

    rep.stage(2, STAGES, "Downloading Solaris game data");
    let present = is_installed(&installed, &FACTS)
        && layout.join("facts").is_dir()
        && layout.join("data/vanilla/reports").is_dir();
    if present && !force {
        rep.log(format!("{} already installed", FACTS.name));
    } else {
        let staging = download_and_unpack(&client, layout, &options.manifest_url, &FACTS, rep)?;
        install_facts(layout, &staging, rep)?;
        let _ = fs::remove_dir_all(&staging);
        installed
            .bundles
            .insert(FACTS.name.into(), FACTS.sha256.into());
        installed.game_version = GAME_VERSION.into();
        installed.save(layout)?;
    }

    rep.stage(3, STAGES, "Downloading Minecraft client");
    rep.set_total(release.client.size);
    net::download(&client, &release.client.job(layout.client_jar()), rep)?;
    let client_changed = installed.client_sha1 != release.client.sha1;
    installed.client_sha1 = release.client.sha1.clone();

    rep.stage(4, STAGES, "Downloading Minecraft game assets");
    let index_job = release.asset_index.job(layout.asset_index());
    rep.set_total(index_job.size);
    net::download(&client, &index_job, rep)?;
    let jobs = mojang::asset_jobs(layout, &fs::read(layout.asset_index())?)?;
    rep.log(format!("{} asset objects", jobs.len()));
    net::download_all(&client, jobs, 12, rep)?;

    rep.stage(5, STAGES, "Extracting textures, models and fonts");
    if installed.resources_for == release.client.sha1
        && layout.join("resources/assets/minecraft").is_dir()
        && !force
        && !client_changed
    {
        rep.log("resources already extracted");
    } else {
        extract_resources(layout, rep)?;
        installed.resources_for = release.client.sha1.clone();
        installed.save(layout)?;
    }

    installed.game_version = GAME_VERSION.into();
    installed.save(layout)?;
    let missing = paths::missing(&layout.root);
    if !missing.is_empty() {
        bail!("install incomplete, missing: {}", missing.join(", "));
    }
    rep.log("Game content ready");
    Ok(())
}

/// Singleplayer only (`solaris-launcher --singleplayer-runtime`, started by the client before
/// a local world): the local server's content cache, installed from the pinned content bundle
/// as its `vanilla_data_dir`. Multiplayer never needs it. Returns without touching the
/// network when the pinned bundle is already installed, so the client can call this on
/// every local start and a launcher with a new pin replaces a stale cache.
pub fn ensure_singleplayer_runtime(
    layout: &Layout,
    options: &Options,
    rep: &Reporter,
) -> Result<()> {
    let mut installed = Installed::load(layout);
    let target = layout.server_content();
    if is_installed(&installed, &CONTENT) && target.join("version.json").is_file() {
        return Ok(());
    }
    rep.stage(1, 1, "Downloading singleplayer server data");
    let client = net::client()?;
    let staging = download_and_unpack(&client, layout, &options.manifest_url, &CONTENT, rep)?;
    replace_dir(&staging.join("content"), &target)?;
    let _ = fs::remove_dir_all(&staging);
    // Left over from the JDK-based content import this bundle replaces.
    let _ = fs::remove_dir_all(layout.join("jdk"));
    let _ = fs::remove_file(layout.version_dir().join("server.jar"));
    installed
        .bundles
        .insert(CONTENT.name.into(), CONTENT.sha256.into());
    installed.save(layout)?;
    rep.log("Singleplayer server data ready");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(kind: &str, name: &str, sha: &str) -> Vec<u8> {
        format!(
            r#"{{"game_version":"{GAME_VERSION}","bundles":[{{"name":"{name}","kind":"{kind}","url":"http://h/x","sha256":"{sha}","size":5}}]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn pinned_bundles_are_accepted() {
        for pin in [&FACTS, &CONTENT] {
            let bundle = parse_bundle(&manifest(pin.kind, pin.name, pin.sha256), pin).unwrap();
            assert_eq!((bundle.url.as_str(), bundle.size), ("http://h/x", 5));
        }
    }

    #[test]
    fn manifest_hash_that_differs_from_the_pin_is_refused() {
        let wrong = "1".repeat(64);
        for pin in [&FACTS, &CONTENT] {
            let error = parse_bundle(&manifest(pin.kind, pin.name, &wrong), pin).unwrap_err();
            assert!(error.to_string().contains("pinned"), "{error}");
        }
    }

    #[test]
    fn unpinned_bundle_name_is_never_installed() {
        for pin in [&FACTS, &CONTENT] {
            let name = format!("{}-r99", pin.name);
            assert!(parse_bundle(&manifest(pin.kind, &name, pin.sha256), pin).is_err());
        }
    }

    #[test]
    fn pinned_name_under_another_kind_is_refused() {
        assert!(parse_bundle(&manifest("facts", CONTENT.name, CONTENT.sha256), &CONTENT).is_err());
    }

    #[test]
    fn wrong_game_version_is_refused() {
        let text = br#"{"game_version":"1.0","bundles":[]}"#;
        assert!(parse_bundle(text, &FACTS).is_err());
    }

    #[test]
    fn pins_are_well_formed() {
        for pin in [&FACTS, &CONTENT] {
            assert_eq!(pin.sha256.len(), 64);
            assert!(pin.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                pin.name.contains(GAME_VERSION),
                "{} is not for {GAME_VERSION}",
                pin.name
            );
        }
    }
}
