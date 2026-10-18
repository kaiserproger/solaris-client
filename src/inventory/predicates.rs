//! Pinned BlockPredicate/AdventureModePredicate codecs. Adventure test uses the
//! BlockInWorld overload, which intentionally does not test component matchers.
use super::{Tag as H, components, nbt, snbt};
use crate::protocol::{Reader, Result, content::Content};
use std::collections::{BTreeMap, BTreeSet};

pub struct AdventureBlock<'a> {
    pub state: u32,
    /// saveWithFullMetadata, preserving actual NBT numeric classes.
    pub nbt: Option<&'a H>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct AdventurePredicate {
    pub predicates: Vec<BlockPredicate>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct BlockPredicate {
    pub blocks: Option<H>,
    pub state: Option<H>,
    pub nbt: Option<H>,
    pub components: BTreeMap<String, H>,
    pub predicates: BTreeMap<String, H>,
    /// Network AnyValueType is distinct from a concrete predicate whose codec
    /// happens to encode an empty record.
    pub presence_predicates: BTreeSet<String>,
}
fn field<'a>(value: &'a H, key: &str) -> Option<&'a H> {
    match value {
        H::Map(fields) => fields
            .iter()
            .find_map(|(k, v)| matches!(k,H::String(s) if s==key).then_some(v)),
        _ => None,
    }
}
fn text(value: &H) -> Result<&str> {
    match value {
        H::String(s) => Ok(s),
        _ => Err("predicate string value".into()),
    }
}
fn fields(value: &H) -> Result<&[(H, H)]> {
    match value {
        H::Map(f) => Ok(f),
        _ => Err("predicate record value".into()),
    }
}
fn put(fields: &mut Vec<(H, H)>, key: &str, value: H) {
    fields.push((H::string(key), value));
}
fn compact(values: Vec<H>) -> H {
    if values.len() == 1 {
        values.into_iter().next().unwrap()
    } else {
        H::List(values)
    }
}
impl AdventurePredicate {
    pub fn read(r: &mut Reader<'_>, content: &Content, depth: u8) -> Result<Self> {
        if depth > 32 {
            return Err("adventure predicate depth bound".into());
        }
        let count = r.count(65536)?;
        let mut predicates = Vec::with_capacity(count);
        for _ in 0..count {
            predicates.push(BlockPredicate::read(r, content, depth + 1)?);
        }
        Ok(Self { predicates })
    }
    pub fn persistent(&self) -> Result<H> {
        Ok(compact(
            self.predicates
                .iter()
                .map(BlockPredicate::persistent)
                .collect::<Result<_>>()?,
        ))
    }
    pub fn matches(&self, block: &AdventureBlock<'_>, content: &Content) -> Result<bool> {
        for predicate in &self.predicates {
            if predicate.matches_adventure(block, content)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
pub(super) fn read_adventure(r: &mut Reader<'_>, content: &Content, depth: u8) -> Result<H> {
    AdventurePredicate::read(r, content, depth)?.persistent()
}
impl BlockPredicate {
    fn read(r: &mut Reader<'_>, content: &Content, depth: u8) -> Result<Self> {
        let blocks = if r.bool()? {
            Some(components::holder_set(r, content, "minecraft:block")?)
        } else {
            None
        };
        let state = if r.bool()? {
            let count = r.count(65536)?;
            let mut properties = Vec::with_capacity(count);
            for _ in 0..count {
                let name = r.string(32767)?;
                let value = if r.bool()? {
                    H::string(r.string(32767)?)
                } else {
                    let mut range = Vec::new();
                    if r.bool()? {
                        put(&mut range, "min", H::string(r.string(32767)?));
                    }
                    if r.bool()? {
                        put(&mut range, "max", H::string(r.string(32767)?));
                    }
                    H::Map(range)
                };
                put(&mut properties, &name, value);
            }
            Some(H::Map(properties))
        } else {
            None
        };
        let nbt = if r.bool()? {
            let value = nbt::read(r)?;
            if !matches!(value, H::Map(_)) {
                return Err("NbtPredicate requires compound tag".into());
            }
            Some(value)
        } else {
            None
        };
        let mut components = BTreeMap::new();
        for _ in 0..r.count(65536)? {
            let id = r.var()?;
            let name = content
                .components
                .get(&id)
                .ok_or("exact predicate component ID")?;
            let value = components::read(r, content, name, depth)?;
            if components.insert(name.clone(), value).is_some() {
                return Err("duplicate exact predicate component".into());
            }
        }
        let mut predicates = BTreeMap::new();
        let mut presence_predicates = BTreeSet::new();
        for _ in 0..r.count(64)? {
            let concrete = r.bool()?;
            let id = r.var()?;
            let name = if concrete {
                content
                    .registry_name("minecraft:data_component_predicate_type", id)?
                    .to_owned()
            } else {
                content
                    .components
                    .get(&id)
                    .ok_or("presence predicate component ID")?
                    .clone()
            };
            let value = if concrete {
                normalize_predicate(&name, nbt::read(r)?)?
            } else {
                presence_predicates.insert(name.clone());
                nbt::read(r)?
            };
            if predicates.insert(name, value).is_some() {
                return Err("duplicate component predicate type".into());
            }
        }
        Ok(Self {
            blocks,
            state,
            nbt,
            components,
            predicates,
            presence_predicates,
        })
    }
    pub fn persistent(&self) -> Result<H> {
        let mut f = Vec::new();
        if let Some(v) = &self.blocks {
            put(&mut f, "blocks", v.clone());
        }
        if let Some(v) = &self.state {
            put(&mut f, "state", v.clone());
        }
        if let Some(v) = &self.nbt {
            put(&mut f, "nbt", H::string(snbt::encode(v)?));
        }
        if !self.components.is_empty() {
            put(
                &mut f,
                "components",
                H::record(self.components.iter().map(|(k, v)| (k, v.clone()))),
            );
        }
        if !self.predicates.is_empty() {
            put(
                &mut f,
                "predicates",
                H::record(self.predicates.iter().map(|(k, v)| (k, v.clone()))),
            );
        }
        Ok(H::Map(f))
    }
    pub fn matches_adventure(&self, block: &AdventureBlock<'_>, content: &Content) -> Result<bool> {
        if let Some(holders) = &self.blocks {
            if !content.holder_set_matches_block(&holders.json()?, block.state)? {
                return Ok(false);
            }
        }
        if let Some(properties) = &self.state {
            let Some(actual) = content.state_properties(block.state) else {
                return Ok(false);
            };
            for (key, predicate) in fields(properties)? {
                let key = text(key)?;
                let Some(value) = actual.get(key) else {
                    return Ok(false);
                };
                match predicate {
                    H::String(expected) => {
                        if content.compare_property(block.state, key, value, expected)?
                            != Some(std::cmp::Ordering::Equal)
                        {
                            return Ok(false);
                        }
                    }
                    H::Map(range) => {
                        for (bound, expected) in range {
                            let Some(order) = content.compare_property(
                                block.state,
                                key,
                                value,
                                text(expected)?,
                            )?
                            else {
                                return Ok(false);
                            };
                            if (text(bound)? == "min" && order.is_lt())
                                || (text(bound)? == "max" && order.is_gt())
                            {
                                return Ok(false);
                            }
                        }
                    }
                    _ => return Err("state predicate value".into()),
                }
            }
        }
        if let Some(pattern) = &self.nbt {
            if !block
                .nbt
                .is_some_and(|actual| nbt::matches(pattern, actual))
            {
                return Ok(false);
            }
        }
        Ok(true)
    }
    /// ServerLevel overload: unlike adventure, this additionally tests collectComponents.
    pub fn matches_block(
        &self,
        block: &AdventureBlock<'_>,
        components: Option<&BTreeMap<String, H>>,
        content: &Content,
    ) -> Result<bool> {
        if !self.matches_adventure(block, content)? {
            return Ok(false);
        }
        if self.components.is_empty() && self.predicates.is_empty() {
            return Ok(true);
        }
        let Some(components) = components else {
            return Ok(false);
        };
        self.matches_components(components, content)
    }
    pub fn matches_components(
        &self,
        actual: &BTreeMap<String, H>,
        content: &Content,
    ) -> Result<bool> {
        match_components(
            &self.components,
            &self.predicates,
            &self.presence_predicates,
            actual,
            content,
            0,
        )
    }
}
fn normalize_predicate(name: &str, mut value: H) -> Result<H> {
    if name == "minecraft:custom_data" {
        return match value {
            H::Map(_) => Ok(H::string(snbt::encode(&value)?)),
            H::String(_) => Ok(value),
            _ => Err("custom data predicate NBT".into()),
        };
    }
    if name == "minecraft:written_book_content" {
        if let H::Map(fields) = &mut value {
            if let Some((_, H::Map(collection))) = fields
                .iter_mut()
                .find(|(key, _)| *key == H::string("pages"))
            {
                for (key, values) in collection {
                    if *key == H::string("contains") {
                        if let H::List(values) = values {
                            for value in values {
                                *value =
                                    components::normalize_text(std::mem::replace(value, H::Empty))?;
                            }
                        }
                    }
                    if *key == H::string("count") {
                        if let H::List(values) = values {
                            for value in values {
                                if let H::Map(fields) = value {
                                    if let Some((_, value)) =
                                        fields.iter_mut().find(|(key, _)| *key == H::string("test"))
                                    {
                                        *value = components::normalize_text(std::mem::replace(
                                            value,
                                            H::Empty,
                                        ))?;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    fn normalize(value: &mut H) {
        match value {
            H::Map(fields) => {
                for (key, value) in fields {
                    if matches!(key,H::String(k) if matches!(k.as_str(),"twinkle"|"trail"|"has_twinkle"|"has_trail"|"resolved"))
                    {
                        if let H::Byte(n) = value {
                            *value = H::Bool(*n != 0);
                        }
                    } else {
                        normalize(value);
                    }
                }
            }
            H::List(values) => {
                for value in values {
                    normalize(value);
                }
            }
            _ => {}
        }
    }
    normalize(&mut value);
    Ok(value)
}
fn number(value: &H) -> Result<f64> {
    match value {
        H::Byte(n) => Ok(f64::from(*n)),
        H::Short(n) => Ok(f64::from(*n)),
        H::Int(n) => Ok(f64::from(*n)),
        H::Long(n) => Ok(*n as f64),
        H::Float(n) => Ok(f64::from(*n)),
        H::Double(n) => Ok(*n),
        _ => Err("numeric predicate value".into()),
    }
}
fn bound(predicate: Option<&H>, actual: f64) -> Result<bool> {
    let Some(predicate) = predicate else {
        return Ok(true);
    };
    match predicate {
        H::Map(fields) => {
            for (k, v) in fields {
                let n = number(v)?;
                if (text(k)? == "min" && actual < n) || (text(k)? == "max" && actual > n) {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        _ => Ok(number(predicate)? == actual),
    }
}
fn holder_matches(holders: &H, actual: &H, registry: &str, content: &Content) -> Result<bool> {
    match holders {
        H::List(values) => {
            for holder in values {
                if holder_matches(holder, actual, registry, content)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        H::String(tag) if tag.starts_with('#') => {
            let H::String(name) = actual else {
                return Ok(false);
            };
            for id in content.registry_tag(registry, tag)? {
                if content.registry_name(registry, *id)? == name {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        _ => Ok(holders == actual),
    }
}
fn match_components(
    exact: &BTreeMap<String, H>,
    predicates: &BTreeMap<String, H>,
    presence: &BTreeSet<String>,
    actual: &BTreeMap<String, H>,
    content: &Content,
    depth: u8,
) -> Result<bool> {
    if depth > 32 {
        return Err("nested component predicate depth".into());
    }
    for (name, value) in exact {
        if actual.get(name) != Some(value) {
            return Ok(false);
        }
    }
    for (name, predicate) in predicates {
        if presence.contains(name) {
            if !actual.contains_key(name) {
                return Ok(false);
            }
            continue;
        }
        if !match_component(name, predicate, actual, content, depth + 1)? {
            return Ok(false);
        }
    }
    Ok(true)
}
fn match_component(
    name: &str,
    predicate: &H,
    actual: &BTreeMap<String, H>,
    content: &Content,
    depth: u8,
) -> Result<bool> {
    let Some(value) = actual.get(name) else {
        return Ok(false);
    };
    match name {
        "minecraft:damage" => {
            let damage = number(value)?;
            let maximum = actual
                .get("minecraft:max_damage")
                .map(number)
                .transpose()?
                .unwrap_or(0.0);
            Ok(bound(field(predicate, "damage"), damage)?
                && bound(field(predicate, "durability"), maximum - damage)?)
        }
        "minecraft:enchantments" | "minecraft:stored_enchantments" => {
            let H::List(predicates) = predicate else {
                return Err("enchantment predicate list".into());
            };
            for predicate in predicates {
                let holders = field(predicate, "enchantments");
                let levels = field(predicate, "levels");
                let mut found = false;
                for (name, level) in fields(value)? {
                    if number(level)? != 0.0
                        && holders
                            .map(|h| holder_matches(h, name, "minecraft:enchantment", content))
                            .transpose()?
                            .unwrap_or(true)
                        && bound(levels, number(level)?)?
                    {
                        found = true;
                        break;
                    }
                }
                if !found {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        "minecraft:potion_contents" => match field(value, "potion") {
            Some(potion) => holder_matches(predicate, potion, "minecraft:potion", content),
            None => Ok(false),
        },
        "minecraft:villager/variant" => {
            holder_matches(predicate, value, "minecraft:villager_type", content)
        }
        "minecraft:custom_data" => {
            let parsed = match predicate {
                H::String(s) => snbt::decode(s)?,
                _ => predicate.clone(),
            };
            Ok(nbt::matches(&parsed, value))
        }
        "minecraft:trim" => {
            for (key, registry) in [
                ("material", "minecraft:trim_material"),
                ("pattern", "minecraft:trim_pattern"),
            ] {
                if let Some(holders) = field(predicate, key) {
                    let Some(value) = field(value, key) else {
                        return Ok(false);
                    };
                    if !holder_matches(holders, value, registry, content)? {
                        return Ok(false);
                    }
                }
            }
            Ok(true)
        }
        "minecraft:jukebox_playable" => {
            if let Some(song) = field(predicate, "song") {
                holder_matches(
                    song,
                    field(value, "song").unwrap_or(value),
                    "minecraft:jukebox_song",
                    content,
                )
            } else {
                Ok(true)
            }
        }
        "minecraft:firework_explosion" => match_explosion(predicate, value),
        "minecraft:fireworks" => {
            if !bound(
                field(predicate, "flight_duration"),
                field(value, "flight_duration")
                    .map(number)
                    .transpose()?
                    .unwrap_or(0.0),
            )? {
                return Ok(false);
            }
            collection(
                field(predicate, "explosions"),
                field(value, "explosions"),
                |p, v| match_explosion(p, v),
            )
        }
        "minecraft:container" | "minecraft:bundle_contents" => {
            collection(field(predicate, "items"), Some(value), |p, v| {
                let item = if name == "minecraft:container" {
                    field(v, "item").ok_or("container item entry")?
                } else {
                    v
                };
                match_item(p, item, content, depth)
            })
        }
        "minecraft:writable_book_content" | "minecraft:written_book_content" => {
            if name == "minecraft:written_book_content" {
                for key in ["author", "title", "resolved"] {
                    if let Some(expected) = field(predicate, key) {
                        let Some(raw) = field(value, key) else {
                            if key == "resolved" && expected == &H::Bool(false) {
                                continue;
                            }
                            return Ok(false);
                        };
                        if field(raw, "raw").unwrap_or(raw) != expected {
                            return Ok(false);
                        }
                    }
                }
                if !bound(
                    field(predicate, "generation"),
                    field(value, "generation")
                        .map(number)
                        .transpose()?
                        .unwrap_or(0.0),
                )? {
                    return Ok(false);
                }
            }
            collection(field(predicate, "pages"), field(value, "pages"), |p, v| {
                Ok(field(v, "raw").unwrap_or(v) == field(p, "contents").unwrap_or(p))
            })
        }
        "minecraft:attribute_modifiers" => {
            collection(field(predicate, "modifiers"), Some(value), |p, v| {
                for (key, registry) in [("attribute", "minecraft:attribute")] {
                    if let Some(h) = field(p, key) {
                        let Some(a) = field(v, "type") else {
                            return Ok(false);
                        };
                        if !holder_matches(h, a, registry, content)? {
                            return Ok(false);
                        }
                    }
                }
                for key in ["id", "operation", "slot"] {
                    if let Some(p) = field(p, key) {
                        if field(v, key) != Some(p) {
                            return Ok(false);
                        }
                    }
                }
                bound(
                    field(p, "amount"),
                    field(v, "amount").map(number).transpose()?.unwrap_or(0.0),
                )
            })
        }
        _ => Err(format!("unknown concrete component predicate {name}")),
    }
}
fn match_explosion(predicate: &H, value: &H) -> Result<bool> {
    for (key, component) in [
        ("shape", "shape"),
        ("has_twinkle", "has_twinkle"),
        ("has_trail", "has_trail"),
    ] {
        if let Some(expected) = field(predicate, key) {
            let default = if key == "shape" {
                H::string("small_ball")
            } else {
                H::Bool(false)
            };
            if field(value, component).unwrap_or(&default) != expected {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
fn collection(
    predicate: Option<&H>,
    values: Option<&H>,
    mut test: impl FnMut(&H, &H) -> Result<bool>,
) -> Result<bool> {
    let Some(predicate) = predicate else {
        return Ok(true);
    };
    let empty = Vec::new();
    let values = match values {
        Some(H::List(values)) => values,
        None => &empty,
        _ => return Err("collection predicate values".into()),
    };
    if !bound(field(predicate, "size"), values.len() as f64)? {
        return Ok(false);
    }
    if let Some(H::List(required)) = field(predicate, "contains") {
        for p in required {
            let mut found = false;
            for value in values {
                if test(p, value)? {
                    found = true;
                    break;
                }
            }
            if !found {
                return Ok(false);
            }
        }
    }
    if let Some(H::List(counts)) = field(predicate, "count") {
        for count in counts {
            let p = field(count, "test").ok_or("collection count test")?;
            let mut matched = 0;
            for value in values {
                if test(p, value)? {
                    matched += 1;
                }
            }
            if !bound(field(count, "count"), f64::from(matched))? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
fn match_item(predicate: &H, item: &H, content: &Content, depth: u8) -> Result<bool> {
    let id = field(item, "id").ok_or("item template ID")?;
    if let Some(holders) = field(predicate, "items") {
        if !holder_matches(holders, id, "minecraft:item", content)? {
            return Ok(false);
        }
    }
    if !bound(
        field(predicate, "count"),
        field(item, "count").map(number).transpose()?.unwrap_or(1.0),
    )? {
        return Ok(false);
    }
    let mut actual = BTreeMap::new();
    let item_name = text(id)?;
    let id = content
        .item_names
        .iter()
        .find_map(|(id, name)| (item_name == name).then_some(*id))
        .ok_or("predicate template unknown item")?;
    for (name, value) in content.item_components(id)?.iter() {
        actual.insert(name.clone(), from_json(value)?);
    }
    if let Some(patch) = field(item, "components") {
        for (name, value) in fields(patch)? {
            let name = text(name)?;
            if let Some(name) = name.strip_prefix('!') {
                actual.remove(name);
            } else {
                actual.insert(name.into(), value.clone());
            }
        }
    }
    let read_map = |key| -> Result<BTreeMap<String, H>> {
        field(predicate, key)
            .map(|value| {
                fields(value)?
                    .iter()
                    .map(|(k, v)| Ok((text(k)?.to_owned(), v.clone())))
                    .collect()
            })
            .unwrap_or_else(|| Ok(BTreeMap::new()))
    };
    let predicates = read_map("predicates")?;
    let types = content.registry_entries("minecraft:data_component_predicate_type")?;
    let presence = predicates
        .iter()
        .filter_map(|(name, value)| {
            (!types.iter().any(|(n, _)| n == name) || matches!(value,H::Map(f)if f.is_empty()))
                .then_some(name.clone())
        })
        .collect();
    match_components(
        &read_map("components")?,
        &predicates,
        &presence,
        &actual,
        content,
        depth + 1,
    )
}
pub(super) fn from_json(value: &serde_json::Value) -> Result<H> {
    use serde_json::Value as J;
    Ok(match value {
        J::Null => H::Empty,
        J::Bool(v) => H::Bool(*v),
        J::String(v) => H::string(v),
        J::Number(n) => {
            if let Some(n) = n.as_i64() {
                if let Ok(n) = i32::try_from(n) {
                    H::Int(n)
                } else {
                    H::Long(n)
                }
            } else {
                H::Double(n.as_f64().ok_or("JSON component number")?)
            }
        }
        J::Array(v) => H::List(v.iter().map(from_json).collect::<Result<_>>()?),
        J::Object(v) => H::record(
            v.iter()
                .map(|(k, v)| Ok((k, from_json(v)?)))
                .collect::<Result<Vec<_>>>()?,
        ),
    })
}
