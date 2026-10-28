//! Official Mojang endpoints: version manifest, version json, asset index.
use crate::{
    net::{self, Check, Job},
    paths::{GAME_VERSION, Layout},
    progress::Reporter,
};
use anyhow::{Context, Result, anyhow};
use reqwest::blocking::Client;
use serde_json::Value;
use std::fs;

pub const VERSION_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const RESOURCES_URL: &str = "https://resources.download.minecraft.net";

#[derive(Clone, Debug)]
pub struct FileRef {
    pub url: String,
    pub sha1: String,
    pub size: u64,
}

impl FileRef {
    fn parse(value: &Value, what: &str) -> Result<Self> {
        let field = |key: &str| value.get(key).ok_or_else(|| anyhow!("version json: {what}.{key} missing"));
        Ok(Self {
            url: field("url")?.as_str().context("url")?.to_owned(),
            sha1: field("sha1")?.as_str().context("sha1")?.to_owned(),
            size: field("size")?.as_u64().context("size")?,
        })
    }

    pub fn job(&self, dest: std::path::PathBuf) -> Job {
        Job {
            url: self.url.clone(),
            dest,
            check: Check::Sha1(self.sha1.clone()),
            size: self.size,
        }
    }
}

pub struct Release {
    pub client: FileRef,
    pub server: FileRef,
    pub asset_index: FileRef,
}

fn parse_release(text: &[u8]) -> Result<Release> {
    let json: Value = serde_json::from_slice(text).context("parsing version json")?;
    if json["id"] != GAME_VERSION {
        anyhow::bail!("version json is for {}, expected {GAME_VERSION}", json["id"]);
    }
    Ok(Release {
        client: FileRef::parse(&json["downloads"]["client"], "downloads.client")?,
        server: FileRef::parse(&json["downloads"]["server"], "downloads.server")?,
        asset_index: FileRef::parse(&json["assetIndex"], "assetIndex")?,
    })
}

/// Fetch (or, offline, reuse) the pinned version's metadata.
pub fn resolve(client: &Client, layout: &Layout, rep: &Reporter) -> Result<Release> {
    let cached = fs::read(layout.version_json()).ok();
    let online = || -> Result<Vec<u8>> {
        let manifest = net::fetch_bytes(client, VERSION_MANIFEST, None)?;
        let manifest: Value = serde_json::from_slice(&manifest).context("parsing version manifest")?;
        let entry = manifest["versions"]
            .as_array()
            .and_then(|list| list.iter().find(|v| v["id"] == GAME_VERSION))
            .ok_or_else(|| anyhow!("Mojang manifest does not list {GAME_VERSION}"))?;
        let url = entry["url"].as_str().context("manifest entry url")?;
        let sha1 = entry["sha1"].as_str().context("manifest entry sha1")?;
        net::fetch_bytes(client, url, Some(&Check::Sha1(sha1.to_owned())))
    };
    let bytes = match online() {
        Ok(bytes) => {
            fs::create_dir_all(layout.version_dir())?;
            fs::write(layout.version_json(), &bytes)?;
            bytes
        }
        Err(error) => match cached {
            Some(bytes) => {
                rep.log(format!("Mojang manifest unavailable ({error:#}); using cached version json"));
                bytes
            }
            None => return Err(error.context("cannot reach Mojang version manifest")),
        },
    };
    parse_release(&bytes)
}

/// Download jobs for every object in the (already verified) asset index.
pub fn asset_jobs(layout: &Layout, index: &[u8]) -> Result<Vec<Job>> {
    let index: Value = serde_json::from_slice(index).context("parsing asset index")?;
    let objects = index["objects"].as_object().context("asset index has no objects")?;
    let mut seen = std::collections::BTreeSet::new();
    let mut jobs = Vec::new();
    for (name, object) in objects {
        let hash = object["hash"].as_str().with_context(|| format!("asset {name}: hash"))?;
        let size = object["size"].as_u64().with_context(|| format!("asset {name}: size"))?;
        if hash.len() < 3 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("asset {name}: malformed hash {hash}");
        }
        if !seen.insert(hash.to_owned()) {
            continue;
        }
        jobs.push(Job {
            url: format!("{RESOURCES_URL}/{}/{hash}", &hash[..2]),
            dest: layout.asset_object(hash),
            check: Check::Sha1(hash.to_owned()),
            size,
        });
    }
    Ok(jobs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_and_dedups_assets() {
        let json = br#"{"id":"26.3","downloads":{"client":{"url":"u","sha1":"a","size":1},"server":{"url":"s","sha1":"b","size":2}},"assetIndex":{"id":"34","url":"i","sha1":"c","size":3}}"#;
        let release = parse_release(json).unwrap();
        assert_eq!(release.server.size, 2);
        let layout = Layout::new("/tmp/x".into());
        let index = br#"{"objects":{"a":{"hash":"abcdef","size":1},"b":{"hash":"abcdef","size":1},"c":{"hash":"012345","size":2}}}"#;
        let jobs = asset_jobs(&layout, index).unwrap();
        assert_eq!(jobs.len(), 2);
        assert!(jobs[0].url.ends_with("/ab/abcdef"));
        assert!(parse_release(br#"{"id":"1.0"}"#).is_err());
    }
}
