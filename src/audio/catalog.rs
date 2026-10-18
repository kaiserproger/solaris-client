//! Sound selection semantics observed in 26.1.2 SoundManager.Preparations and WeighedSoundEvents.
use crate::protocol::Result;
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) fn identifier(name: &str) -> Result<String> {
    let (namespace, path) = name.split_once(':').unwrap_or(("minecraft", name));
    if namespace.is_empty()
        || path.is_empty()
        || namespace.len() + path.len() > 512
        || !namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
        || !path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_./-".contains(&b))
        || path.split('/').any(|p| p == ".." || p.is_empty())
    {
        return Err(format!("invalid sound identifier {name}"));
    }
    Ok(format!("{namespace}:{path}"))
}
#[derive(Clone, Debug)]
pub(super) struct Selection {
    pub name: String,
    pub volume: f32,
    pub pitch: f32,
    pub stream: bool,
    pub preload: bool,
    pub attenuation: f32,
}
struct Entry {
    sound: Selection,
    event: bool,
    weight: u32,
    effective_weight: u32,
}
struct Event {
    entries: Vec<Entry>,
    subtitle: Option<String>,
    total: u32,
}
pub(super) struct Catalog {
    events: BTreeMap<String, Event>,
}
fn positive(v: &Value, key: &str, default: f32) -> Result<f32> {
    let n = match v.get(key) {
        None => default,
        Some(n) => n.as_f64().ok_or_else(|| format!("invalid sound {key}"))? as f32,
    };
    if !n.is_finite() || n <= 0. || n > 65536. {
        return Err(format!("sound {key} outside bound"));
    }
    Ok(n)
}
impl Catalog {
    pub fn parse(value: &Value) -> Result<Self> {
        let object = value.as_object().ok_or("sound catalog must be an object")?;
        if object.len() > 16384 {
            return Err("sound event count bound".into());
        }
        let mut events = BTreeMap::new();
        let mut count = 0usize;
        for (key, value) in object {
            value.as_object().ok_or("invalid sound event definition")?;
            let name = identifier(key)?;
            let mut entries = Vec::new();
            if let Some(list) = value.get("sounds") {
                let list = list.as_array().ok_or("invalid sound list")?;
                count += list.len();
                if list.len() > 4096 || count > 131072 {
                    return Err("sound entry count bound".into());
                }
                for entry in list {
                    let name = identifier(
                        entry
                            .as_str()
                            .or_else(|| entry["name"].as_str())
                            .ok_or("sound entry name missing")?,
                    )?;
                    let kind = entry
                        .get("type")
                        .map_or(Ok("file"), |v| v.as_str().ok_or("invalid sound entry type"))?;
                    let event = match kind {
                        "file" => false,
                        "event" => true,
                        _ => return Err("invalid sound entry type".into()),
                    };
                    let weight = entry
                        .get("weight")
                        .map_or(Some(1), Value::as_u64)
                        .ok_or("invalid sound weight")?;
                    if weight == 0 || weight > i32::MAX as u64 {
                        return Err("sound weight bound".into());
                    }
                    let boolean = |key| -> Result<bool> {
                        entry.get(key).map_or(Ok(false), |v| {
                            v.as_bool().ok_or_else(|| format!("invalid sound {key}"))
                        })
                    };
                    entries.push(Entry {
                        sound: Selection {
                            name,
                            volume: positive(entry, "volume", 1.)?,
                            pitch: positive(entry, "pitch", 1.)?,
                            stream: boolean("stream")?,
                            preload: boolean("preload")?,
                            attenuation: positive(entry, "attenuation_distance", 16.)?,
                        },
                        event,
                        weight: weight as u32,
                        effective_weight: 0,
                    });
                }
            }
            let subtitle = value
                .get("subtitle")
                .map(|v| {
                    v.as_str()
                        .filter(|s| s.len() <= 1024)
                        .map(str::to_owned)
                        .ok_or("invalid subtitle")
                })
                .transpose()?;
            if events
                .insert(
                    name,
                    Event {
                        entries,
                        subtitle,
                        total: 0,
                    },
                )
                .is_some()
            {
                return Err("duplicate namespaced sound event".into());
            }
        }
        let mut catalog = Self { events };
        let mut totals = BTreeMap::new();
        for name in catalog.events.keys() {
            catalog.weight(name, &mut Vec::new(), &mut totals)?;
        }
        for event in catalog.events.values_mut() {
            for entry in &mut event.entries {
                // An event reference's configured weight is NOT used by vanilla getWeight.
                entry.effective_weight = if entry.event {
                    totals.get(&entry.sound.name).copied().unwrap_or(0)
                } else {
                    entry.weight
                };
            }
            event.total = event.entries.iter().map(|e| e.effective_weight).sum();
        }
        Ok(catalog)
    }
    fn weight(
        &self,
        name: &str,
        stack: &mut Vec<String>,
        totals: &mut BTreeMap<String, u32>,
    ) -> Result<u32> {
        if let Some(total) = totals.get(name) {
            return Ok(*total);
        }
        if stack.len() >= 32 || stack.iter().any(|s| s == name) {
            return Err(format!("sound reference cycle/depth: {name}"));
        }
        let Some(event) = self.events.get(name) else {
            return Ok(0);
        };
        stack.push(name.to_owned());
        let mut total = 0u32;
        for entry in &event.entries {
            let n = if entry.event {
                self.weight(&entry.sound.name, stack, totals)?
            } else {
                entry.weight
            };
            total = total
                .checked_add(n)
                .filter(|n| *n <= i32::MAX as u32)
                .ok_or("sound total weight bound")?;
        }
        stack.pop();
        totals.insert(name.to_owned(), total);
        Ok(total)
    }
    pub fn select(&self, event: &str, seed: u64) -> Result<Selection> {
        self.choose(&identifier(event)?, &mut Random::new(seed), 0)
    }
    fn choose(&self, name: &str, random: &mut Random, depth: usize) -> Result<Selection> {
        if depth >= 32 {
            return Err("sound selection depth".into());
        }
        let event = self
            .events
            .get(name)
            .ok_or_else(|| format!("missing sound event {name}"))?;
        if event.total == 0 {
            return Err(format!("empty sound event {name}"));
        }
        let mut pick = random.bounded(event.total);
        for entry in &event.entries {
            if pick < entry.effective_weight {
                if !entry.event {
                    return Ok(entry.sound.clone());
                }
                let mut sound = self.choose(&entry.sound.name, random, depth + 1)?;
                sound.volume *= entry.sound.volume;
                sound.pitch *= entry.sound.pitch;
                sound.stream |= entry.sound.stream;
                if !sound.volume.is_finite() || !sound.pitch.is_finite() {
                    return Err("nested sound gain overflow".into());
                }
                return Ok(sound);
            }
            pick -= entry.effective_weight;
        }
        Err("invalid sound weights".into())
    }
    pub fn subtitle(&self, event: &str) -> Option<&str> {
        self.events.get(event)?.subtitle.as_deref()
    }
    pub fn preloads(&self) -> impl Iterator<Item = &str> {
        self.events
            .values()
            .flat_map(|e| &e.entries)
            .filter(|e| !e.event && e.sound.preload && !e.sound.stream)
            .map(|e| e.sound.name.as_str())
    }
    pub fn len(&self) -> usize {
        self.events.len()
    }
}
// LegacyRandomSource, used by the version's SoundInstance.createUnseededRandom/create(seed).
struct Random(u64);
impl Random {
    fn new(seed: u64) -> Self {
        Self((seed ^ 0x5deece66d) & ((1u64 << 48) - 1))
    }
    fn bits(&mut self) -> u32 {
        self.0 = (self.0.wrapping_mul(0x5deece66d).wrapping_add(11)) & ((1u64 << 48) - 1);
        (self.0 >> 17) as u32
    }
    fn bounded(&mut self, bound: u32) -> u32 {
        if bound.is_power_of_two() {
            return ((bound as u64 * self.bits() as u64) >> 31) as u32;
        }
        loop {
            let bits = self.bits();
            let value = bits % bound;
            if bits.wrapping_sub(value).wrapping_add(bound - 1) <= i32::MAX as u32 {
                return value;
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn nested_weights_inherit_leaf_total_and_gain_not_reference_weight() {
        let c=Catalog::parse(&json!({"root":{"sounds":[{"name":"nested","type":"event","weight":999,"volume":0.5,"pitch":2.,"stream":true},"direct"]},"nested":{"sounds":[{"name":"a","weight":2,"volume":0.4,"attenuation_distance":12},{"name":"b","weight":1}]}})).unwrap();
        assert_eq!(c.events["minecraft:root"].total, 4);
        let mut counts = [0; 3];
        for seed in 0..10000 {
            let s = c.select("root", seed).unwrap();
            match s.name.as_str() {
                "minecraft:a" => {
                    counts[0] += 1;
                    assert_eq!(s.volume, 0.2);
                    assert_eq!(s.pitch, 2.);
                    assert_eq!(s.attenuation, 12.);
                    assert!(s.stream);
                }
                "minecraft:b" => counts[1] += 1,
                "minecraft:direct" => counts[2] += 1,
                _ => panic!(),
            }
        }
        // Independent java.util.Random(seed) oracle. Adjacent seeds deliberately are
        // not statistically uniform; exact choices catch PRNG/recursive-draw drift.
        assert_eq!(counts, [3839, 1932, 4229]);
    }
    #[test]
    fn invalid_cycles_and_weights_reject_without_clamping() {
        assert!(Catalog::parse(&json!({"a":{"sounds":[{"name":"a","type":"event"}]}})).is_err());
        assert!(Catalog::parse(&json!({"a":{"sounds":[{"name":"a","weight":0}]}})).is_err());
        assert!(
            Catalog::parse(&json!({"a":{"sounds":[{"name":"a","weight":2147483647},"b"]}}))
                .is_err()
        );
    }
    #[test]
    fn namespaces_do_not_alias_minecraft() {
        let c = Catalog::parse(&json!({"other:a":{"sounds":["other:x"]},"a":{"sounds":["x"]}}))
            .unwrap();
        assert_eq!(c.select("other:a", 3).unwrap().name, "other:x");
        assert_eq!(c.select("a", 3).unwrap().name, "minecraft:x");
    }
}
