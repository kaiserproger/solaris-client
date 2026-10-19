//! Local derivation of the entity data (`entities/models.json`,
//! `presentation.json`, `motion-facts.json`) from the official client jar.
//!
//! Mojang's entity models and animations exist only as Java code, so this runs
//! the repo's exporter sources (embedded below) on a JDK. Everything produced
//! stays in the user's data directory; it is never uploaded or redistributed.
use crate::{families, jdk, net::{self, Check}, paths::Layout, progress::Reporter};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

const EXPORT_MODELS: &str = include_str!("../../src/entities/ExportModels.java");
const EXPORT_PRESENTATION: &str = include_str!("../../src/entities/ExportPresentation.java");
const EXPORT_MOTION: &str = include_str!("../../src/entities/ExportMotionFacts.java");

const DERIVED: &[&str] = &["models.json", "presentation.json", "motion-facts.json"];
/// Files that must exist in `entities/` for the client to start.
pub const OUTPUTS: &[&str] = &["models.json", "presentation.json", "motion-facts.json", "provenance.json"];

/// Identity of the exporter sources: derived data is redone when they change.
pub fn exporter_stamp() -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for part in [EXPORT_MODELS, EXPORT_PRESENTATION, EXPORT_MOTION] {
        hasher.update(part.as_bytes());
        hasher.update([0]);
    }
    hasher.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn run_logged(mut command: Command, rep: &Reporter) -> Result<String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| format!("starting {:?}", command.get_program()))?;
    let stderr = child.stderr.take().unwrap();
    let rep2 = rep.clone();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            // The JVM warns about joml's use of sun.misc.Unsafe on every run.
            if !line.starts_with("WARNING:") && !line.trim().is_empty() {
                rep2.log(format!("  {line}"));
            }
        }
    });
    let mut stdout = String::new();
    child.stdout.take().unwrap().read_to_string(&mut stdout)?;
    let status = child.wait()?;
    let _ = reader.join();
    if !status.success() {
        bail!("{:?} failed with {status}", command.get_program());
    }
    Ok(stdout)
}

fn separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

fn log_summary(rep: &Reporter, stdout: &str) {
    for line in stdout.lines().filter(|l| !l.trim().is_empty()).take(6) {
        rep.log(format!("  {line}"));
    }
}

/// Unpack `META-INF/libraries/*.jar` from the server jar (the exporters' classpath).
fn extract_libraries(server_jar: &Path, dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let mut zip = zip::ZipArchive::new(fs::File::open(server_jar)?)?;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        if name.starts_with("META-INF/libraries/") && name.ends_with(".jar") {
            if entry.size() > 64 << 20 {
                bail!("library {name} exceeds size bound");
            }
            let file = dir.join(Path::new(&name).file_name().unwrap());
            std::io::copy(&mut entry, &mut fs::File::create(file)?)?;
        }
    }
    Ok(())
}

pub fn run(layout: &Layout, jdk_home: &Path, rep: &Reporter) -> Result<()> {
    let work = layout.join("work/entities");
    let _ = fs::remove_dir_all(&work);
    let (src, classes, libs) = (work.join("src"), work.join("classes"), work.join("libs"));
    fs::create_dir_all(&src)?;
    fs::create_dir_all(&classes)?;
    let sources = [
        ("ExportModels.java", EXPORT_MODELS),
        ("ExportPresentation.java", EXPORT_PRESENTATION),
        ("ExportMotionFacts.java", EXPORT_MOTION),
    ];
    let mut java_files = Vec::new();
    for (name, text) in sources {
        let path = src.join(name);
        fs::write(&path, text)?;
        java_files.push(path);
    }
    extract_libraries(&layout.join("mojang/versions/26.1.2/server.jar"), &libs)?;
    let client_jar = layout.client_jar();
    let sha256 = net::hash_file(&client_jar, &Check::Sha256(String::new()))?;
    let lib_glob = libs.join("*");
    let classpath = |with_classes: bool| -> String {
        let mut parts: Vec<String> = Vec::new();
        if with_classes {
            parts.push(classes.display().to_string());
        }
        parts.push(client_jar.display().to_string());
        parts.push(lib_glob.display().to_string());
        parts.join(separator())
    };

    rep.log("Compiling entity exporters");
    let mut javac = Command::new(jdk::javac(jdk_home));
    javac
        .args(["-nowarn", "-proc:none", "-XDignore.symbol.file=true", "-Xlint:none", "-d"])
        .arg(&classes)
        .arg("-cp")
        .arg(classpath(false))
        .args(&java_files);
    run_logged(javac, rep)?;

    let java = |main: &str, args: &[&str]| -> Result<String> {
        let mut command = Command::new(jdk::java(jdk_home));
        command
            .arg("-Djava.awt.headless=true")
            .arg("-cp")
            .arg(classpath(true))
            .arg(main)
            .args(args)
            .current_dir(&work);
        run_logged(command, rep)
    };

    rep.log("Exporting entity model rigs");
    let models_path = work.join("models.json");
    log_summary(rep, &java("ExportModels", &[&models_path.to_string_lossy(), &sha256])?);

    rep.log("Resolving renderer families");
    let mut zip = zip::ZipArchive::new(fs::File::open(&client_jar)?)?;
    let renderers: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|e| e.name().to_owned()))
        .filter(|n| {
            n.starts_with("net/minecraft/client/renderer/entity/") && n.ends_with("Renderer.class") && !n.contains('$')
        })
        .map(|n| n[..n.len() - 6].replace('/', "."))
        .collect();
    let mut transcript = String::new();
    for batch in renderers.chunks(60) {
        let mut command = Command::new(jdk::javap(jdk_home));
        command.args(["-c", "-p", "-classpath"]).arg(&client_jar).args(batch);
        transcript.push_str(&run_logged(command, rep)?);
    }
    let bodies = families::parse_transcript(&transcript)?;
    let mut document: Value = serde_json::from_slice(&fs::read(&models_path)?).context("parsing raw models.json")?;
    let variants = families::builtin_variants(&mut zip)?;
    families::build(&mut document, &bodies, variants)?;
    let data = serde_json::to_vec(&document)?;
    fs::write(&models_path, &data)?;
    let provenance = json!({
        "schema": 1, "game_version": "26.1.2", "client_sha256": sha256,
        "roots": document["roots"].as_object().map_or(0, |m| m.len()),
        "entity_types": document["entities"].as_object().map_or(0, |m| m.len()),
        "distribution": "local Mojang-derived cache; never redistribute",
    });
    fs::write(work.join("provenance.json"), serde_json::to_vec_pretty(&provenance)?)?;

    rep.log("Adding face normals");
    let exporter_path = src.join("ExportModels.java");
    log_summary(
        rep,
        &java(
            "ExportModels",
            &["--augment-normals", &models_path.to_string_lossy(), &sha256, &exporter_path.to_string_lossy()],
        )?,
    );

    rep.log("Exporting entity presentation");
    let presentation = work.join("presentation.json");
    log_summary(
        rep,
        &java("ExportPresentation", &[&client_jar.to_string_lossy(), &presentation.to_string_lossy()])?,
    );

    rep.log("Exporting entity motion facts");
    let motion = work.join("motion-facts.json");
    log_summary(rep, &java("ExportMotionFacts", &[&models_path.to_string_lossy(), &motion.to_string_lossy()])?);

    let out = layout.join("entities");
    fs::create_dir_all(&out)?;
    for name in DERIVED {
        let from: PathBuf = work.join(name);
        if !from.is_file() {
            bail!("exporter did not produce {name}");
        }
        let to = out.join(name);
        let tmp = out.join(format!("{name}.tmp"));
        fs::copy(&from, &tmp)?;
        fs::rename(&tmp, &to)?;
    }
    // The client refuses entity data whose digests are not recorded next to it.
    let digest = |name: &str| net::hash_file(&out.join(name), &Check::Sha256(String::new()));
    let receipt = json!({
        "schema": 2,
        "game_version": "26.1.2",
        "client_sha256": sha256,
        "models_sha256": digest("models.json")?,
        "presentation_sha256": digest("presentation.json")?,
        "motion_sha256": digest("motion-facts.json")?,
        "distribution": "local Mojang-derived cache; never redistribute",
    });
    fs::write(out.join("provenance.json"), serde_json::to_vec_pretty(&receipt)?)?;
    let _ = fs::remove_dir_all(layout.join("work"));
    let _ = std::io::stdout().flush();
    Ok(())
}
