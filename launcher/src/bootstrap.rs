//! First-run content bootstrap. Every stage is idempotent and skips quickly
//! when its result is already installed, so Play after a clean install does
//! not touch the network.
use crate::{
    derive,
    jdk,
    mojang,
    pins,
    net::{self, Check, Job},
    paths::{self, GAME_VERSION, Layout},
    progress::{Cancelled, Reporter},
};
use anyhow::{Context, Result, anyhow, bail};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    fs,
    io::{Read, Write},
    path::Path,
};

const STAGES: usize = 8;

/// What is installed, so unchanged stages can be skipped.
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Installed {
    pub game_version: String,
    pub client_sha1: String,
    /// Bundle name -> sha256 of the installed archive.
    pub bundles: std::collections::BTreeMap<String, String>,
    pub resources_for: String,
    pub entities_for: String,
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
        && !installed.entities_for.is_empty()
        && paths::missing(&layout.root).is_empty()
}

pub struct Options {
    pub manifest_url: String,
}

#[derive(Debug)]
struct Bundle {
    name: String,
    url: String,
    sha256: String,
    size: u64,
}

fn fetch_bundles(client: &Client, manifest_url: &str) -> Result<Vec<Bundle>> {
    let bytes = net::fetch_bytes(client, manifest_url, None)
        .with_context(|| format!("cannot reach the Solaris content manifest at {manifest_url}"))?;
    parse_bundles(&bytes)
}

/// Pure manifest handling, separate from the network fetch for testing.
fn parse_bundles(bytes: &[u8]) -> Result<Vec<Bundle>> {
    let manifest: Value = serde_json::from_slice(bytes).context("parsing content manifest")?;
    if manifest["game_version"] != GAME_VERSION {
        bail!("content manifest is for {}, launcher needs {GAME_VERSION}", manifest["game_version"]);
    }
    let mut bundles = Vec::new();
    for entry in manifest["bundles"].as_array().context("manifest.bundles")? {
        if entry["kind"] != "facts" {
            continue;
        }
        let text = |key: &str| {
            entry[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("manifest bundle missing {key}"))
        };
        let name = text("name")?;
        let sha256 = text("sha256")?;
        // The manifest travels over plain HTTP: only the pinned archive is accepted.
        if name != pins::FACTS_NAME {
            continue;
        }
        if !sha256.eq_ignore_ascii_case(pins::FACTS_SHA256) {
            bail!(
                "refusing content bundle {name}: manifest sha256 {sha256} does not match the hash pinned in this launcher ({})",
                pins::FACTS_SHA256
            );
        }
        bundles.push(Bundle {
            name,
            url: text("url")?,
            sha256: pins::FACTS_SHA256.to_owned(),
            size: entry["size"].as_u64().context("manifest bundle size")?,
        });
    }
    if bundles.is_empty() {
        bail!(
            "content manifest does not list the bundle {} this launcher is pinned to; update the launcher",
            pins::FACTS_NAME
        );
    }
    Ok(bundles)
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

fn install_bundle(layout: &Layout, archive: &Path, rep: &Reporter) -> Result<()> {
    let staging = layout.join(".staging-bundle");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    let decoder = zstd::stream::read::Decoder::new(fs::File::open(archive)?)?;
    tar::Archive::new(decoder).unpack(&staging).context("unpacking Solaris data bundle")?;
    for entry in fs::read_dir(&staging)? {
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
    let _ = fs::remove_dir_all(&staging);
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
        rep.check_cancel().map_err(|_: Cancelled| net::UserCancelled)?;
        let mut entry = zip.by_index(index)?;
        if !entry.is_file() || !entry.name().starts_with("assets/") {
            continue;
        }
        let Some(relative) = entry.enclosed_name() else {
            bail!("client jar entry {} escapes the extraction root", entry.name());
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
    let bundles = fetch_bundles(&client, &options.manifest_url)?;
    for bundle in &bundles {
        let present = installed.bundles.get(&bundle.name) == Some(&bundle.sha256)
            && layout.join("facts").is_dir()
            && layout.join("data/vanilla/reports").is_dir();
        if present && !force {
            rep.log(format!("{} already installed", bundle.name));
            continue;
        }
        let archive = layout.cache_dir().join(format!("{}.tar.zst", bundle.name));
        rep.set_total(bundle.size);
        net::download(
            &client,
            &Job {
                url: bundle.url.clone(),
                dest: archive.clone(),
                check: Check::Sha256(bundle.sha256.clone()),
                size: bundle.size,
            },
            rep,
        )?;
        install_bundle(layout, &archive, rep)?;
        installed.bundles.insert(bundle.name.clone(), bundle.sha256.clone());
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

    let entities_stamp = format!("{}:{}", release.client.sha1, derive::exporter_stamp());
    let entities_ready = derive::OUTPUTS.iter().all(|n| layout.join("entities").join(n).is_file());
    let derive_needed = force || !entities_ready || installed.entities_for != entities_stamp;

    rep.stage(6, STAGES, "Downloading Minecraft server (entity tooling)");
    let jdk_home = if derive_needed {
        let server = layout.join("mojang/versions/26.1.2/server.jar");
        rep.set_total(release.server.size);
        net::download(&client, &release.server.job(server), rep)?;
        rep.stage(7, STAGES, "Downloading Java toolchain (Temurin JDK)");
        Some(jdk::ensure(&client, layout, rep)?)
    } else {
        rep.log("entity data up to date");
        rep.stage(7, STAGES, "Java toolchain not needed");
        None
    };

    rep.stage(8, STAGES, "Deriving entity models from the client");
    match jdk_home {
        Some(home) => {
            derive::run(layout, &home, rep)?;
            installed.entities_for = entities_stamp;
            installed.save(layout)?;
        }
        None => rep.log("entity data already derived"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str, sha: &str) -> Vec<u8> {
        format!(
            r#"{{"game_version":"26.1.2","bundles":[{{"name":"{name}","kind":"facts","url":"http://h/x","sha256":"{sha}","size":5}}]}}"#
        )
        .into_bytes()
    }

    #[test]
    fn pinned_bundle_is_accepted() {
        let bundles = parse_bundles(&manifest(pins::FACTS_NAME, pins::FACTS_SHA256)).unwrap();
        assert_eq!(bundles.len(), 1);
        assert_eq!(bundles[0].sha256, pins::FACTS_SHA256);
    }

    #[test]
    fn manifest_hash_that_differs_from_the_pin_is_refused() {
        let wrong = "1".repeat(64);
        let error = parse_bundles(&manifest(pins::FACTS_NAME, &wrong)).unwrap_err();
        assert!(error.to_string().contains("pinned"), "{error}");
    }

    #[test]
    fn unpinned_bundle_name_is_never_installed() {
        assert!(parse_bundles(&manifest("solaris-facts-26.1.2-r99", pins::FACTS_SHA256)).is_err());
    }

    #[test]
    fn wrong_game_version_is_refused() {
        let text = br#"{"game_version":"1.0","bundles":[]}"#;
        assert!(parse_bundles(text).is_err());
    }

    #[test]
    fn pin_is_well_formed() {
        assert_eq!(pins::FACTS_SHA256.len(), 64);
        assert!(pins::FACTS_SHA256.bytes().all(|b| b.is_ascii_hexdigit()));
    }
}
