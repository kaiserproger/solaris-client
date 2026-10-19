//! Rust port of the post-processing in `src/entities/process_assets.py`:
//! turns the raw `ExportModels` output plus `javap` renderer disassembly into
//! the final `models.json` document (renderer families, texture candidates and
//! built-in variants). Pure functions, validated against the retained export.
use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::{collections::BTreeMap, io::Read};

/// One `javap -c -p` class body and its superclass.
pub struct Bodies {
    /// Insertion-ordered (transcript order) class name -> body text.
    pub order: Vec<String>,
    pub bodies: BTreeMap<String, String>,
    pub parents: BTreeMap<String, Option<String>>,
}

fn strip_generics(header: &str) -> Result<String> {
    let mut text = header.to_owned();
    while text.contains('<') {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::with_capacity(text.len());
        let mut changed = false;
        let mut i = 0;
        while i < chars.len() {
            if chars[i] == '<' {
                let mut j = i + 1;
                while j < chars.len() && chars[j] != '<' && chars[j] != '>' {
                    j += 1;
                }
                if j < chars.len() && chars[j] == '>' {
                    changed = true;
                    i = j + 1;
                    continue;
                }
            }
            out.push(chars[i]);
            i += 1;
        }
        if !changed {
            bail!("renderer generic signature cannot be balanced");
        }
        text = out;
    }
    Ok(text)
}

fn word_after(text: &str, keyword: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = text[from..].find(keyword) {
        let at = from + found;
        let before_ok = text[..at].chars().next_back().is_none_or(|c| !(c.is_alphanumeric() || c == '_'));
        if before_ok {
            let name: String = text[at + keyword.len()..]
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
                .collect();
            if !name.is_empty() {
                return Some(name);
            }
        }
        from = at + keyword.len();
    }
    None
}

pub fn parse_transcript(transcript: &str) -> Result<Bodies> {
    let mut result = Bodies {
        order: Vec::new(),
        bodies: BTreeMap::new(),
        parents: BTreeMap::new(),
    };
    let marker = "Compiled from \"";
    let mut starts: Vec<usize> = transcript.match_indices(marker).map(|(i, _)| i).collect();
    if starts.first() != Some(&0) {
        starts.insert(0, 0);
    }
    starts.push(transcript.len());
    for pair in starts.windows(2) {
        let body = &transcript[pair[0]..pair[1]];
        let header = strip_generics(body.split('{').next().unwrap_or(""))?;
        if let Some(name) = word_after(&header, "class ") {
            let parent = word_after(&header, "extends ");
            if !result.bodies.contains_key(&name) {
                result.order.push(name.clone());
            }
            result.bodies.insert(name.clone(), body.to_owned());
            result.parents.insert(name, parent);
        }
    }
    Ok(result)
}

/// `// String textures/entity/....png` constants in javap output order.
fn entity_textures(body: &str) -> Vec<String> {
    const PREFIX: &str = "// String ";
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = body[from..].find(PREFIX) {
        let start = from + at + PREFIX.len();
        from = start;
        let rest = &body[start..];
        if !rest.starts_with("textures/entity/") {
            continue;
        }
        let run: &str = rest
            .split(|c: char| c.is_whitespace() || c == '%')
            .next()
            .unwrap_or("");
        let minimum = "textures/entity/".len() + 1;
        if let Some(png) = run.rfind(".png")
            && png >= minimum
        {
            found.push(run[..png + 4].to_owned());
            from = start + png + 4;
        }
    }
    found
}

fn texture_literals(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::String(s) if s.contains(":textures/") || s.starts_with("textures/") => out.push(s.clone()),
        Value::Object(map) => map.values().for_each(|v| texture_literals(v, out)),
        Value::Array(list) => list.iter().for_each(|v| texture_literals(v, out)),
        _ => {}
    }
}

fn dedup(list: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    list.into_iter().filter(|s| seen.insert(s.clone())).collect()
}

fn title_case(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Adds `families` and `builtin_variants` to the raw model document.
pub fn build(document: &mut Value, bodies: &Bodies, variants: Value) -> Result<()> {
    let entities = document["entities"].as_object().context("models: entities")?.clone();
    let defaults_all = document["renderer_defaults"].as_object().cloned().unwrap_or_default();
    let roots = document["roots"].as_object().context("models: roots")?.clone();
    let mut families = Map::new();
    for (name, schema) in &entities {
        let class = schema["class"].as_str().context("entity class")?;
        let entity_class = class.rsplit('.').next().unwrap_or(class);
        let defaults = defaults_all.get(name).cloned().unwrap_or_else(|| json!({}));
        let candidates: Vec<String> = match defaults.get("renderer_class").and_then(Value::as_str) {
            Some(renderer) => vec![renderer.to_owned()],
            None => {
                let wanted = format!("{entity_class}Renderer").to_lowercase();
                bodies
                    .order
                    .iter()
                    .filter(|k| k.rsplit('.').next().unwrap_or(k).to_lowercase() == wanted)
                    .cloned()
                    .collect()
            }
        };
        let mut textures = Vec::new();
        for candidate in candidates.iter().cloned() {
            let mut seen = std::collections::HashSet::new();
            let mut current = Some(candidate);
            while let Some(class) = current {
                if !bodies.bodies.contains_key(&class) || !seen.insert(class.clone()) {
                    break;
                }
                textures.extend(entity_textures(&bodies.bodies[&class]));
                current = bodies.parents.get(&class).cloned().flatten();
            }
        }
        let textures = dedup(textures);
        let mut textures = textures;
        if let Some(materials) = defaults.get("materials") {
            texture_literals(materials, &mut textures);
        }
        let textures: Vec<String> = dedup(textures)
            .into_iter()
            .map(|t| t.strip_prefix("minecraft:").map(str::to_owned).unwrap_or(t))
            .collect();
        let textures = dedup(textures);
        let root = defaults
            .get("rig")
            .and_then(Value::as_str)
            .map_or_else(|| format!("{name}#main"), str::to_owned);
        if !roots.contains_key(&root) {
            continue;
        }
        let short = name.split(':').nth(1).unwrap_or(name);
        let exact: Vec<&String> = textures
            .iter()
            .filter(|t| t.rsplit('/').next() == Some(&format!("{short}.png")))
            .collect();
        let mut texture = defaults.get("texture").and_then(Value::as_str).map(str::to_owned);
        if texture.as_deref() == Some("minecraft:missingno") {
            texture = None;
        }
        let texture = texture.or_else(|| {
            exact
                .first()
                .map(|t| (*t).clone())
                .or_else(|| (textures.len() == 1).then(|| textures[0].clone()))
        });
        let baby = format!("{name}_baby#main");
        families.insert(
            name.clone(),
            json!({
                "rig": root,
                "texture": texture,
                "baby_rig": roots.contains_key(&baby).then_some(baby),
                "textures": textures,
                "renderer_class": defaults.get("renderer_class").cloned()
                    .unwrap_or_else(|| candidates.first().map_or(Value::Null, |c| json!(c))),
                "materials": defaults.get("materials").cloned().unwrap_or_else(|| json!({})),
            }),
        );
    }
    for (alias, base) in [("cave_spider", "spider"), ("spectral_arrow", "arrow"), ("glow_squid", "squid")] {
        let key = format!("minecraft:{alias}");
        let base_key = format!("minecraft:{base}");
        if !families.contains_key(&key)
            && let Some(base_entry) = families.get(&base_key).cloned()
        {
            let mut entry = base_entry;
            let suffix = format!(
                ".{}Renderer",
                alias.split('_').map(title_case).collect::<String>()
            );
            let render = bodies
                .order
                .iter()
                .find(|k| k.ends_with(&suffix))
                .map(|k| bodies.bodies[k].as_str())
                .unwrap_or("");
            let found = entity_textures(render);
            if found.len() == 1 {
                entry["texture"] = json!(found[0]);
            }
            families.insert(key, entry);
        }
    }
    document["families"] = Value::Object(families);
    document["builtin_variants"] = variants;
    Ok(())
}

/// `data/minecraft/<x>_variant/<name>.json` entries of the client jar.
pub fn builtin_variants<R: Read + std::io::Seek>(zip: &mut zip::ZipArchive<R>) -> Result<Value> {
    let mut registries: Map<String, Value> = Map::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index)?;
        let name = entry.name().to_owned();
        let pieces: Vec<&str> = name.split('/').collect();
        if pieces.len() == 4
            && pieces[0] == "data"
            && pieces[1] == "minecraft"
            && pieces[2].ends_with("_variant")
            && pieces[3].ends_with(".json")
        {
            let mut text = Vec::new();
            entry.read_to_end(&mut text)?;
            let value: Value = serde_json::from_slice(&text).with_context(|| format!("variant {name}"))?;
            let registry = registries
                .entry(format!("minecraft:{}", pieces[2]))
                .or_insert_with(|| json!({}));
            registry[format!("minecraft:{}", &pieces[3][..pieces[3].len() - 5])] = value;
        }
    }
    Ok(Value::Object(registries))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generics_are_stripped_and_unbalanced_rejected() {
        assert_eq!(
            strip_generics("public class a.B<T extends c.D<E>> extends f.G<H>").unwrap(),
            "public class a.B extends f.G"
        );
        assert!(strip_generics("a < b").is_err());
    }

    #[test]
    fn transcript_parses_class_and_parent() {
        let text = "Compiled from \"A.java\"\npublic class x.y.CowRenderer<S> extends x.y.Base<S> {\n  ldc #1 // String textures/entity/cow/cow.png\n}\nCompiled from \"B.java\"\npublic abstract class x.y.Base<S> {\n}\n";
        let b = parse_transcript(text).unwrap();
        assert_eq!(b.order, ["x.y.CowRenderer", "x.y.Base"]);
        assert_eq!(b.parents["x.y.CowRenderer"].as_deref(), Some("x.y.Base"));
        assert_eq!(b.parents["x.y.Base"], None);
        assert_eq!(entity_textures(&b.bodies["x.y.CowRenderer"]), ["textures/entity/cow/cow.png"]);
    }

    #[test]
    fn texture_scan_ignores_non_entity_and_template_strings() {
        let body = "// String textures/gui/a.png\n// String textures/entity/x/%s.png\n// String textures/entity/z.png\n";
        // `%` terminates the run, so the templated name has no `.png` and is skipped.
        assert_eq!(entity_textures(body), ["textures/entity/z.png"]);
    }
}
