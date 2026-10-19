//! Temurin JDK from the Adoptium API, used only by singleplayer's first-run content import
//! (the local server derives datapack data from the Mojang server jar with Java extractors).
//! Fetched lazily by `--singleplayer-runtime`; multiplayer never needs it.
use crate::{
    net::{self, Check, Job},
    paths::Layout,
    progress::Reporter,
};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
};

pub const JDK_MAJOR: u32 = 25;

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    }
}

pub fn java(home: &Path) -> PathBuf {
    home.join("bin").join(exe("java"))
}
pub fn javac(home: &Path) -> PathBuf {
    home.join("bin").join(exe("javac"))
}
pub fn javap(home: &Path) -> PathBuf {
    home.join("bin").join(exe("javap"))
}

fn usable(home: &Path) -> bool {
    java(home).is_file() && javac(home).is_file() && javap(home).is_file()
}

fn platform() -> Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "linux" => "linux",
        "windows" => "windows",
        "macos" => "mac",
        other => bail!("no Temurin JDK for OS {other}"),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        other => bail!("no Temurin JDK for CPU {other}"),
    };
    Ok((os, arch))
}

/// A usable JDK: `$SOLARIS_JDK` if set, else the managed copy under `<root>/jdk`,
/// downloading it first when absent.
pub fn ensure(client: &Client, layout: &Layout, rep: &Reporter) -> Result<PathBuf> {
    if let Some(custom) = std::env::var_os("SOLARIS_JDK").map(PathBuf::from) {
        if usable(&custom) {
            rep.log(format!("Using JDK from SOLARIS_JDK: {}", custom.display()));
            return Ok(custom);
        }
        bail!(
            "SOLARIS_JDK={} has no bin/java, javac and javap",
            custom.display()
        );
    }
    let home = layout.join(&format!("jdk/temurin-{JDK_MAJOR}"));
    if usable(&home) {
        return Ok(home);
    }
    let (os, arch) = platform()?;
    let api = format!(
        "https://api.adoptium.net/v3/assets/latest/{JDK_MAJOR}/hotspot?architecture={arch}&image_type=jdk&os={os}&vendor=eclipse"
    );
    rep.log(format!("Looking up Temurin JDK {JDK_MAJOR} ({os}/{arch})"));
    let listing: Value = serde_json::from_slice(&net::fetch_bytes(client, &api, None)?)
        .context("parsing Adoptium response")?;
    let package = listing
        .as_array()
        .and_then(|list| list.first())
        .map(|release| &release["binary"]["package"])
        .ok_or_else(|| anyhow!("Adoptium has no JDK {JDK_MAJOR} for {os}/{arch}"))?;
    let link = package["link"].as_str().context("package link")?;
    let sha256 = package["checksum"].as_str().context("package checksum")?;
    let size = package["size"].as_u64().context("package size")?;
    let name = package["name"].as_str().context("package name")?;
    let archive = layout.cache_dir().join(name);
    rep.set_total(size);
    net::download(
        client,
        &Job {
            url: link.to_owned(),
            dest: archive.clone(),
            check: Check::Sha256(sha256.to_owned()),
            size,
        },
        rep,
    )?;
    rep.log(format!("Unpacking {name}"));
    let staging = layout.join("jdk/.staging");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    if name.ends_with(".zip") {
        let mut zip = zip::ZipArchive::new(fs::File::open(&archive)?)?;
        zip.extract(&staging).context("unpacking JDK zip")?;
    } else {
        let tar = flate2::read::GzDecoder::new(fs::File::open(&archive)?);
        tar::Archive::new(tar)
            .unpack(&staging)
            .context("unpacking JDK tar.gz")?;
    }
    // The archive holds a single top directory (`jdk-25.0.x+y`).
    let top = fs::read_dir(&staging)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| p.is_dir())
        .ok_or_else(|| anyhow!("JDK archive was empty"))?;
    let _ = fs::remove_dir_all(&home);
    fs::rename(&top, &home)?;
    let _ = fs::remove_dir_all(&staging);
    if !usable(&home) {
        bail!("unpacked JDK at {} lacks java/javac/javap", home.display());
    }
    let _ = fs::remove_file(&archive);
    Ok(home)
}
