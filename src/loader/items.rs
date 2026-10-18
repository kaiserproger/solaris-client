//! Bounded schema-2 item declarations and the supported static item graph subset.
//! Admission owns bundle permissions, namespace ownership and verified archive hashes.
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

type Result<T, E = String> = std::result::Result<T, E>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ItemDefinition {
    pub id: String,
    pub base_item: String,
    pub name: String,
}

pub fn parse_items(entries: &[Value]) -> Result<BTreeMap<String, ItemDefinition>> {
    if entries.len() > 128 {
        return Err("Loader activated item count exceeds 128".into());
    }
    let mut items = BTreeMap::new();
    for entry in entries {
        fields(entry, &["id", "base_item", "name"], "Loader item")?;
        let id = identifier(&entry["id"], "Loader item id")?;
        let base_item = identifier(&entry["base_item"], "Loader base item")?;
        let name = entry["name"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .ok_or("Loader item name must be 1..128 UTF-8 bytes")?;
        let item = ItemDefinition {
            id: id.clone(),
            base_item,
            name: name.into(),
        };
        if items.insert(id.clone(), item).is_some() {
            return Err(format!("duplicate Loader item {id}"));
        }
    }
    Ok(items)
}

fn fields(value: &Value, allowed: &[&str], context: &str) -> Result<()> {
    let fields = value
        .as_object()
        .ok_or_else(|| format!("{context} must be an object"))?;
    for field in fields.keys() {
        if !allowed.contains(&field.as_str()) {
            return Err(format!("unsupported {context} field {field}"));
        }
    }
    Ok(())
}

fn identifier(value: &Value, context: &str) -> Result<String> {
    let id = value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(|| format!("{context} must be 1..128 UTF-8 bytes"))?;
    let (ns, path) = id
        .split_once(':')
        .ok_or_else(|| format!("{context} requires namespace: {id}"))?;
    if ns.is_empty()
        || !ns
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"_.-".contains(&c))
        || path.is_empty()
        || !path
            .bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || b"/_.-".contains(&c))
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return Err(format!("invalid {context} {id}"));
    }
    Ok(id.into())
}

/// Enumerates every model in a static graph before activation, never just the currently
/// selected branch. Dynamic/special nodes need an explicit admission contract and fail closed.
pub fn model_ids(definition: &Value) -> Result<BTreeSet<String>> {
    fields(definition, &["model"], "Loader item definition")?;
    let mut models = BTreeSet::new();
    let mut nodes = 0;
    visit(&definition["model"], 0, &mut nodes, &mut models)?;
    Ok(models)
}

fn visit(node: &Value, depth: usize, nodes: &mut usize, out: &mut BTreeSet<String>) -> Result<()> {
    *nodes += 1;
    if depth > 64 || *nodes > 256 {
        return Err("Loader item graph depth/node bound".into());
    }
    let kind = node["type"]
        .as_str()
        .ok_or("Loader item graph type missing")?;
    match kind.strip_prefix("minecraft:").unwrap_or(kind) {
        "model" => {
            fields(node, &["type", "model"], "Loader model node")?;
            out.insert(identifier(&node["model"], "Loader model resource")?);
        }
        "empty" => fields(node, &["type"], "Loader empty node")?,
        "composite" => {
            fields(node, &["type", "models"], "Loader composite node")?;
            let children = node["models"]
                .as_array()
                .ok_or("Loader composite models must be an array")?;
            if children.len() > 256 {
                return Err("Loader item graph child bound".into());
            }
            for child in children {
                visit(child, depth + 1, nodes, out)?;
            }
        }
        other => return Err(format!("unsupported Loader item graph node {other}")),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn declarations_reject_duplicates_unknown_fields_and_invalid_identifiers() {
        let item = json!({"id":"example:sword","base_item":"minecraft:paper","name":"Меч"});
        assert_eq!(
            parse_items(&[item.clone()]).unwrap()["example:sword"].base_item,
            "minecraft:paper"
        );
        assert!(
            parse_items(&[item.clone(), item.clone()])
                .unwrap_err()
                .contains("duplicate")
        );
        for (field, value) in [
            ("id", json!("example:../escape")),
            ("base_item", json!("paper")),
            ("name", json!("я".repeat(65))),
            ("model", json!("example:item/sword")),
        ] {
            let mut invalid = item.clone();
            invalid[field] = value;
            assert!(parse_items(&[invalid]).is_err(), "accepted {field}");
        }
        let unique = (0..128).map(|n| json!({"id":format!("example:item{n}"),"base_item":"minecraft:paper","name":"Item"})).collect::<Vec<_>>();
        assert_eq!(parse_items(&unique).unwrap().len(), 128);
        let mut excess = unique;
        excess.push(item);
        assert!(parse_items(&excess).unwrap_err().contains("count"));
    }

    #[test]
    fn static_graph_enumerates_all_models_and_rejects_hidden_unsupported_nodes() {
        let graph = json!({"model":{"type":"minecraft:composite","models":[
            {"type":"model","model":"example:item/sword"},
            {"type":"empty"}, {"type":"model","model":"example:item/shield"}]}});
        assert_eq!(
            model_ids(&graph).unwrap(),
            BTreeSet::from(["example:item/shield".into(), "example:item/sword".into()])
        );
        let mut invalid = graph.clone();
        invalid["model"]["models"][2] = json!({"type":"special","model":{}});
        assert!(model_ids(&invalid).unwrap_err().contains("special"));
        invalid = graph;
        invalid["model"]["models"][0]["tints"] = json!([]);
        assert!(model_ids(&invalid).unwrap_err().contains("tints"));
    }

    #[test]
    fn graphs_have_total_node_and_depth_bounds() {
        let leaf = json!({"type":"empty"});
        assert!(
            model_ids(&json!({"model":{"type":"composite","models":vec![leaf.clone();255]}}))
                .is_ok()
        );
        assert!(
            model_ids(&json!({"model":{"type":"composite","models":vec![leaf.clone();256]}}))
                .is_err()
        );
        let mut deep = leaf;
        for _ in 0..65 {
            deep = json!({"type":"composite","models":[deep]});
        }
        assert!(
            model_ids(&json!({"model":deep}))
                .unwrap_err()
                .contains("depth")
        );
    }
}
