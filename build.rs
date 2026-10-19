//! Embeds the Solaris-authored entity models (`assets/solaris/models/entity/*.json`).
use std::{env, fmt::Write as _, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let dir = root.join("assets/solaris/models/entity");
    println!("cargo:rerun-if-changed={}", dir.display());
    println!("cargo:rerun-if-changed=build.rs");
    let mut names: Vec<String> = match fs::read_dir(&dir) {
        Ok(entries) => entries
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| name.ends_with(".json"))
            .collect(),
        Err(_) => Vec::new(),
    };
    names.sort();
    let mut out = String::from("pub(crate) const EMBEDDED: &[(&str, &str)] = &[\n");
    for name in &names {
        let path = dir.join(name);
        println!("cargo:rerun-if-changed={}", path.display());
        writeln!(
            out,
            "    ({name:?}, include_str!({:?})),",
            path.display().to_string()
        )
        .unwrap();
    }
    out.push_str("];\n");
    let target = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("solaris_models_embedded.rs");
    fs::write(target, out).unwrap();
}
