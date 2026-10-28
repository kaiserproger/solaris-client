//! One local data root; Mojang content is linked from the user's existing caches.
use std::path::{Path, PathBuf};

const INPUTS: &[(&str, &str)] = &[
    (
        "COLLISION_SHAPES",
        "facts/block_collision_shapes_26_3.bin",
    ),
    ("TERRAIN_FACTS", "facts/terrain-facts.json"),
    (
        "BLOCK_STATE_PREDICATES",
        "facts/block-state-predicates.json",
    ),
    ("BLOCK_USE_FACTS", "facts/block-use-classes.json"),
    ("ITEM_USE_FACTS", "facts/item-use-owners.json"),
    ("INTERACTION_FACTS", "facts/interaction-facts.json"),
    ("ITEM_PROPERTIES", "facts/item-properties.json"),
    ("CREATIVE_TABS", "facts/creative-facts.json"),
    ("PLACEMENT_FACTS", "facts/placement-state-facts.json"),
    ("ITEM_PLACEMENT_FACTS", "facts/item-placement-facts.json"),
    ("ACTION_STATE_FACTS", "facts/action-state-facts.json"),
    ("ENTITY_USE_STATIC", "facts/entity-use-static.json"),
    ("ASSET_INDEX", "assets/asset-index.json"),
    ("ASSET_ROOT", "assets"),
    ("DATA_DIR", "data/vanilla"),
    ("RESOURCE_ROOT", "resources"),
];

pub(crate) fn default_root() -> Result<PathBuf, String> {
    #[cfg(windows)]
    if let Some(app) = std::env::var_os("APPDATA").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(app).join("solaris-client"));
    }
    if let Some(data) = std::env::var_os("XDG_DATA_HOME").filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(data).join("solaris-client"));
    }
    std::env::var_os("HOME")
        .filter(|s| !s.is_empty())
        .map(|home| PathBuf::from(home).join(".local/share/solaris-client"))
        .ok_or_else(|| {
            "data directory unavailable: set HOME/XDG_DATA_HOME or pass --data-dir DIR".into()
        })
}

/// Called only by the CLI before the event loop or any application threads exist.
/// Explicit environment overrides are retained, including for harness launches.
pub(crate) fn apply_defaults(root: &Path) {
    for (suffix, relative) in INPUTS {
        let var = format!("SOLARIS_CLIENT_{suffix}");
        if std::env::var_os(&var).is_none() {
            // SAFETY: startup is single-threaded; no environment readers exist yet.
            unsafe { std::env::set_var(var, root.join(relative)) };
        }
    }
    // Singleplayer keeps worlds and the managed server beside the content.
    if std::env::var_os("SOLARIS_CLIENT_ROOT").is_none() {
        // SAFETY: same single-threaded startup window as above.
        unsafe { std::env::set_var("SOLARIS_CLIENT_ROOT", root) };
    }
}

fn require(path: &Path, directory: bool) -> Result<(), String> {
    let valid = if directory {
        path.is_dir()
    } else {
        path.is_file()
    };
    if valid {
        Ok(())
    } else {
        Err(format!(
            "missing content {}: {} (run solaris-launcher once to download it, or solaris-client setup --from-dist DIR --assets DIR)",
            if directory { "directory" } else { "file" },
            path.display()
        ))
    }
}

/// Fail before creating a window instead of leaving startup on a blank frame.
pub(crate) fn validate(presentation: bool) -> Result<(), String> {
    for (suffix, _) in INPUTS {
        if !presentation && matches!(*suffix, "ASSET_ROOT" | "ASSET_INDEX" | "RESOURCE_ROOT") {
            continue;
        }
        let var = format!("SOLARIS_CLIENT_{suffix}");
        let path = std::env::var_os(&var).ok_or_else(|| format!("missing content path: {var}"))?;
        require(
            Path::new(&path),
            matches!(*suffix, "ASSET_ROOT" | "DATA_DIR" | "RESOURCE_ROOT"),
        )?;
    }
    let data = PathBuf::from(std::env::var_os("SOLARIS_CLIENT_DATA_DIR").unwrap());
    for file in [
        "blocks.json",
        "block_mining.json",
        "block_light.json",
        "registries.json",
    ] {
        require(&data.join("reports").join(file), false)?;
    }
    if presentation {
        let assets = PathBuf::from(std::env::var_os("SOLARIS_CLIENT_ASSET_ROOT").unwrap());
        require(&assets.join("objects"), true)?;
    }
    Ok(())
}

fn link(source: &Path, destination: &Path) -> Result<(), String> {
    let source = source
        .canonicalize()
        .map_err(|e| format!("{}: {e}", source.display()))?;
    if let Ok(existing) = destination.canonicalize() {
        if existing == source {
            return Ok(());
        }
        return Err(format!(
            "setup destination already exists: {}",
            destination.display()
        ));
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, destination)
        .map_err(|e| format!("{}: {e}", destination.display()))?;
    #[cfg(not(unix))]
    return Err("setup currently requires Unix symlinks".into());
    Ok(())
}

pub(crate) fn setup(dist: &Path, assets: &Path, root: &Path) -> Result<(), String> {
    // Validate every source before creating the destination. Never overwrite caches.
    for directory in ["facts", "data", "resources"] {
        require(&dist.join(directory), true)?;
    }
    require(&dist.join("assets/asset-index.json"), false)?;
    require(&assets.join("objects"), true)?;
    std::fs::create_dir_all(root.join("assets")).map_err(|e| format!("{}: {e}", root.display()))?;
    for directory in ["facts", "data", "resources"] {
        link(&dist.join(directory), &root.join(directory))?;
    }
    // Pin exactly the index paired with the verified dist facts, not an arbitrary
    // newest Prism index. Its referenced object hashes are resolved in Prism.
    link(
        &dist.join("assets/asset-index.json"),
        &root.join("assets/asset-index.json"),
    )?;
    link(&assets.join("objects"), &root.join("assets/objects"))?;
    println!(
        "Content ready at {} (keep the source caches in place)",
        root.display()
    );
    Ok(())
}
