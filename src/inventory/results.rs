//! Client-side result updates; server-only recipe resolution is deliberately not synthesized.
use super::Prediction;
use crate::{
    inventory::{MenuKind, Stack, Tag, persistent},
    protocol::{Result, content::Content},
};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
fn number(s: &Stack, name: &str) -> i32 {
    s.component(name).and_then(Value::as_i64).unwrap_or(0) as i32
}
fn damageable(s: &Stack) -> bool {
    s.component("minecraft:max_damage").is_some()
        && s.component("minecraft:damage").is_some()
        && s.component("minecraft:unbreakable").is_none()
}
fn damage(s: &Stack) -> i32 {
    number(s, "minecraft:damage").clamp(0, number(s, "minecraft:max_damage").max(0))
}
fn chant_key(s: &Stack, c: &Content) -> &'static str {
    if c.item_names
        .get(&s.id)
        .is_some_and(|n| n == "minecraft:enchanted_book")
    {
        "minecraft:stored_enchantments"
    } else {
        "minecraft:enchantments"
    }
}
fn chants(s: &Stack, c: &Content) -> Map<String, Value> {
    s.component(chant_key(s, c))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}
// Entry order is retained from the component, not the sorted JSON projection.
// Java's holder-identity hash iteration is process-local, not protocol data.
fn chant_entries<'a>(
    stack: &'a Stack,
    content: &Content,
) -> impl Iterator<Item = Result<(&'a str, i32)>> {
    let key = chant_key(stack, content);
    let typed = match stack.typed_component(key) {
        Some(Tag::Map(entries)) => Some(entries),
        _ => None,
    };
    let retained = typed
        .into_iter()
        .flatten()
        .map(|(key, value)| match (key, value) {
            (Tag::String(name), Tag::Int(level)) => Ok((name.as_str(), *level)),
            _ => Err("enchantment component entry shape".into()),
        });
    let defaults = stack
        .component(key)
        .and_then(Value::as_object)
        .filter(|_| typed.is_none())
        .into_iter()
        .flatten()
        .map(|(name, level)| {
            Ok((
                name.as_str(),
                level.as_i64().ok_or("enchantment level")? as i32,
            ))
        });
    retained.chain(defaults)
}
fn contains(c: &Content, registry: &str, set: &Value, name: &str) -> Result<bool> {
    Ok(match set {
        Value::String(s) if s.starts_with('#') => {
            let id = c
                .registry_entries(registry)?
                .iter()
                .position(|(n, _)| n == name)
                .ok_or("holder missing")?;
            c.registry_tag(registry, s)?.contains(&(id as i32))
        }
        Value::String(s) => s == name,
        Value::Array(a) => a.iter().any(|v| v.as_str() == Some(name)),
        _ => return Err("holder set shape".into()),
    })
}
impl Prediction<'_> {
    fn changed(&self, before: &[Option<Stack>], range: std::ops::Range<usize>) -> bool {
        range.into_iter().any(|i| before[i] != self.menu.slots[i])
    }
    pub(super) fn reconcile_input_changes(&mut self, before: &[Option<Stack>]) -> Result<()> {
        match self.menu.kind {
            MenuKind::Anvil if self.changed(before, 0..2) => self.recompute_anvil()?,
            MenuKind::Grindstone if self.changed(before, 0..2) => self.recompute_grindstone()?,
            MenuKind::Smithing if self.changed(before, 0..3) => self.set(3, Stack::default(), true),
            MenuKind::Cartography if self.changed(before, 0..2) => {
                if self.slot(0)?.empty() || self.slot(1)?.empty() {
                    self.set(2, Stack::default(), true);
                }
            }
            MenuKind::Stonecutter if self.changed(before, 0..1) => {
                if before[0].as_ref().map(|s| s.id) != Some(self.slot(0)?.id) {
                    self.menu.data.insert(0, -1);
                }
                self.set(1, Stack::default(), true);
            }
            MenuKind::Loom if self.changed(before, 0..3) => self.recompute_loom(before)?,
            MenuKind::Merchant if self.changed(before, 0..2) => self.refresh_trade()?,
            _ => {}
        }
        Ok(())
    }
    pub(super) fn loom_set(content: &Content, pattern: &Stack) -> Result<Vec<String>> {
        let names = content.registry_entries("minecraft:banner_pattern")?;
        if pattern.empty() {
            return content
                .registry_tag("minecraft:banner_pattern", "minecraft:no_item_required")?
                .iter()
                .map(|id| {
                    content
                        .registry_name("minecraft:banner_pattern", *id)
                        .map(str::to_owned)
                })
                .collect();
        }
        let Some(set) = pattern.component("minecraft:provides_banner_patterns") else {
            return Ok(vec![]);
        };
        match set {
            Value::String(s) if s.starts_with('#') => content
                .registry_tag("minecraft:banner_pattern", s)?
                .iter()
                .map(|id| {
                    content
                        .registry_name("minecraft:banner_pattern", *id)
                        .map(str::to_owned)
                })
                .collect(),
            Value::String(s) => {
                if !names.iter().any(|(n, _)| n == s) {
                    return Err("loom holder missing".into());
                }
                Ok(vec![s.clone()])
            }
            Value::Array(a) => a
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or("loom holder shape".into())
                })
                .collect(),
            _ => Err("loom holder set shape".into()),
        }
    }
    fn recompute_loom(&mut self, before: &[Option<Stack>]) -> Result<()> {
        let banner = self.slot(0)?.clone();
        let dye = self.slot(1)?.clone();
        if banner.empty() || dye.empty() {
            self.menu.data.insert(0, -1);
            self.set(3, Stack::default(), true);
            return Ok(());
        }
        let old = Self::loom_set(
            self.content,
            before[2].as_ref().ok_or("loom pattern not observed")?,
        )?;
        let next = Self::loom_set(self.content, self.slot(2)?)?;
        let selected = self.menu.data.get(&0).copied().unwrap_or(-1);
        let selected = if next.len() == 1 {
            0
        } else {
            usize::try_from(selected)
                .ok()
                .and_then(|i| old.get(i))
                .and_then(|name| next.iter().position(|n| n == name))
                .map_or(-1, |i| i as i32)
        };
        let mut patterns = banner
            .component("minecraft:banner_patterns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if patterns.len() >= 6 {
            self.menu.data.insert(0, -1);
            self.set(3, Stack::default(), true);
            return Ok(());
        }
        self.menu.data.insert(0, selected);
        if selected < 0 {
            self.set(3, Stack::default(), true);
            return Ok(());
        }
        let color = dye
            .component("minecraft:dye")
            .ok_or("loom dye component missing")?;
        patterns.push(json!({"pattern":next[selected as usize],"color":color}));
        let mut output = persistent::with_components(
            self.content,
            &banner,
            &BTreeMap::from([("minecraft:banner_patterns".into(), Value::Array(patterns))]),
        )?;
        output.count = 1;
        self.set(3, output, true);
        Ok(())
    }
    fn curse(&self, name: &str) -> Result<bool> {
        let entries = self.content.registry_entries("minecraft:enchantment")?;
        let id = entries
            .iter()
            .position(|(n, _)| n == name)
            .ok_or("enchantment missing")?;
        Ok(self
            .content
            .registry_tag("minecraft:enchantment", "minecraft:curse")?
            .contains(&(id as i32)))
    }
    fn recompute_grindstone(&mut self) -> Result<()> {
        let a = self.slot(0)?.clone();
        let b = self.slot(1)?.clone();
        if a.empty() && b.empty() || a.count > 1 || b.count > 1 {
            self.set(2, Stack::default(), true);
            return Ok(());
        }
        let mut output = if a.empty() { b.clone() } else { a.clone() };
        let mut merged = chants(&output, self.content);
        let mut changes = BTreeMap::new();
        if a.empty() || b.empty() {
            if !["minecraft:enchantments", "minecraft:stored_enchantments"]
                .iter()
                .any(|key| {
                    output
                        .component(key)
                        .and_then(Value::as_object)
                        .is_some_and(|v| !v.is_empty())
                })
            {
                self.set(2, Stack::default(), true);
                return Ok(());
            }
        } else {
            if a.id != b.id {
                self.set(2, Stack::default(), true);
                return Ok(());
            }
            let max = number(&a, "minecraft:max_damage").max(number(&b, "minecraft:max_damage"));
            if damageable(&a) {
                let remaining = number(&a, "minecraft:max_damage") - damage(&a)
                    + number(&b, "minecraft:max_damage")
                    - damage(&b)
                    + (max as i64 * 5 / 100) as i32;
                output.count = 1;
                changes.insert("minecraft:max_damage".into(), json!(max));
                changes.insert("minecraft:damage".into(), json!((max - remaining).max(0)));
            } else if a.max_count()? >= 2 && a == b {
                output.count = 2;
            } else {
                self.set(2, Stack::default(), true);
                return Ok(());
            }
            for (name, level) in chants(&b, self.content) {
                let old = merged.get(&name).and_then(Value::as_i64).unwrap_or(0);
                if !self.curse(&name)? || old == 0 {
                    merged.insert(name, json!(old.max(level.as_i64().unwrap_or(0))));
                }
            }
        }
        let mut kept = Map::new();
        let mut repair = 0i32;
        for (name, level) in merged {
            if self.curse(&name)? {
                kept.insert(name, level);
                repair = repair.saturating_mul(2).saturating_add(1);
            }
        }
        changes.insert(
            chant_key(&output, self.content).into(),
            Value::Object(kept.clone()),
        );
        changes.insert("minecraft:repair_cost".into(), json!(repair));
        output = persistent::with_components(self.content, &output, &changes)?;
        if self
            .content
            .item_names
            .get(&output.id)
            .is_some_and(|n| n == "minecraft:enchanted_book")
            && kept.is_empty()
        {
            let id = self
                .content
                .item_names
                .iter()
                .find_map(|(id, n)| (n == "minecraft:book").then_some(*id))
                .ok_or("book item missing")?;
            let mut wire = crate::protocol::Writer::default();
            wire.var(output.count);
            wire.var(id as i32);
            let mut reader = crate::protocol::Reader::new(&output.wire);
            reader.var()?;
            reader.var()?;
            wire.0.extend_from_slice(&output.wire[reader.offset..]);
            output = crate::inventory::read_stack(
                &mut crate::protocol::Reader::new(&wire.0),
                self.content,
            )?;
        }
        self.set(2, output, true);
        Ok(())
    }
    pub(super) fn recompute_anvil(&mut self) -> Result<()> {
        let a = self.slot(0)?.clone();
        let b = self.slot(1)?.clone();
        self.menu.only_renaming = false;
        self.menu.repair_material_count = 0;
        self.menu.data.insert(0, 1);
        if a.empty() || a.component(chant_key(&a, self.content)).is_none() {
            self.menu.data.insert(0, 0);
            self.set(2, Stack::default(), true);
            return Ok(());
        }
        let prior =
            number(&a, "minecraft:repair_cost") as i64 + number(&b, "minecraft:repair_cost") as i64;
        let mut output = a.clone();
        let mut changes = BTreeMap::new();
        let mut merged = chants(&a, self.content);
        let mut cost = 0i32;
        let mut rename_cost = 0;
        if !b.empty() {
            let book = b.component("minecraft:stored_enchantments").is_some();
            let material = if damageable(&a) {
                match a
                    .component("minecraft:repairable")
                    .and_then(|v| v.get("items"))
                {
                    Some(set) => contains(
                        self.content,
                        "minecraft:item",
                        set,
                        self.content
                            .item_names
                            .get(&b.id)
                            .ok_or("repair item missing")?,
                    )?,
                    None => false,
                }
            } else {
                false
            };
            if material {
                let mut dmg = damage(&a);
                let max = number(&a, "minecraft:max_damage");
                let mut per = dmg.min(max / 4);
                if per <= 0 {
                    self.menu.data.insert(0, 0);
                    self.set(2, Stack::default(), true);
                    return Ok(());
                }
                while per > 0 && self.menu.repair_material_count < b.count {
                    dmg -= per;
                    cost = cost.wrapping_add(1);
                    self.menu.repair_material_count += 1;
                    per = dmg.min(max / 4);
                }
                changes.insert("minecraft:damage".into(), json!(dmg));
            } else {
                if !book && !(a.id == b.id && damageable(&a)) {
                    self.menu.data.insert(0, 0);
                    self.set(2, Stack::default(), true);
                    return Ok(());
                }
                if damageable(&a) && !book {
                    let max = number(&a, "minecraft:max_damage");
                    let remaining = max
                        .wrapping_sub(damage(&a))
                        .wrapping_add(number(&b, "minecraft:max_damage").wrapping_sub(damage(&b)))
                        .wrapping_add(max.wrapping_mul(12) / 100);
                    let dmg = max.wrapping_sub(remaining).max(0);
                    if dmg < damage(&a) {
                        changes.insert("minecraft:damage".into(), json!(dmg));
                        cost = cost.wrapping_add(2);
                    }
                }
                let mut accepted = false;
                let mut rejected = false;
                for entry in chant_entries(&b, self.content) {
                    let (name, incoming) = entry?;
                    let definition = &self
                        .content
                        .registry_entries("minecraft:enchantment")?
                        .iter()
                        .find(|(n, _)| n == name)
                        .ok_or("enchantment missing")?
                        .1;
                    let old = merged.get(name).and_then(Value::as_i64).unwrap_or(0) as i32;
                    let mut level = if old == incoming {
                        incoming.wrapping_add(1)
                    } else {
                        old.max(incoming)
                    };
                    let mut applicable = self.source.game_mode == 1
                        || self
                            .content
                            .item_names
                            .get(&a.id)
                            .is_some_and(|n| n == "minecraft:enchanted_book")
                        || contains(
                            self.content,
                            "minecraft:item",
                            definition
                                .get("supported_items")
                                .ok_or("enchantment supported items")?,
                            self.content
                                .item_names
                                .get(&a.id)
                                .ok_or("anvil item missing")?,
                        )?;
                    for existing in merged.keys() {
                        if existing == name {
                            continue;
                        }
                        let other = &self
                            .content
                            .registry_entries("minecraft:enchantment")?
                            .iter()
                            .find(|(n, _)| n == existing)
                            .ok_or("existing enchantment missing")?
                            .1;
                        let incompatible = definition
                            .get("exclusive_set")
                            .map(|set| {
                                contains(self.content, "minecraft:enchantment", set, existing)
                            })
                            .transpose()?
                            .unwrap_or(false)
                            || other
                                .get("exclusive_set")
                                .map(|set| {
                                    contains(self.content, "minecraft:enchantment", set, name)
                                })
                                .transpose()?
                                .unwrap_or(false);
                        if incompatible {
                            applicable = false;
                            cost = cost.wrapping_add(1);
                        }
                    }
                    if !applicable {
                        rejected = true;
                        continue;
                    }
                    accepted = true;
                    level = level.min(
                        definition
                            .get("max_level")
                            .and_then(Value::as_i64)
                            .ok_or("enchantment max level")? as i32,
                    );
                    if level <= 0 {
                        merged.remove(name);
                    } else {
                        merged.insert(name.to_owned(), json!(level.min(255)));
                    }
                    let mut price = definition
                        .get("anvil_cost")
                        .and_then(Value::as_i64)
                        .ok_or("enchantment anvil cost")?
                        as i32;
                    if book {
                        price = (price / 2).max(1)
                    }
                    cost = cost.wrapping_add(price.wrapping_mul(level));
                    if a.count > 1 {
                        cost = 40;
                    }
                }
                if rejected && !accepted {
                    self.menu.data.insert(0, 0);
                    self.set(2, Stack::default(), true);
                    return Ok(());
                }
            }
        }
        let name = self.menu.anvil_name.as_deref();
        let blank=name.is_none_or(|s|s.chars().all(|c|matches!(c,'\u{9}'..='\u{d}'|'\u{1c}'..='\u{20}'|'\u{a0}'|'\u{1680}'|'\u{2000}'..='\u{200a}'|'\u{2028}'|'\u{2029}'|'\u{202f}'|'\u{205f}'|'\u{3000}')));
        if blank {
            if a.component("minecraft:custom_name").is_some() {
                rename_cost = 1;
                cost = cost.wrapping_add(1);
                changes.insert("!minecraft:custom_name".into(), Value::Null);
            }
        } else {
            let hover = self
                .menu
                .anvil_hover_name
                .as_deref()
                .ok_or("actual anvil hover name required")?;
            if name.unwrap() != hover {
                rename_cost = 1;
                cost = cost.wrapping_add(1);
                changes.insert(
                    "minecraft:custom_name".into(),
                    json!({"text":name.unwrap()}),
                );
            }
        }
        let mut xp = if cost <= 0 {
            0
        } else {
            (prior + cost as i64).clamp(0, i32::MAX as i64) as i32
        };
        if rename_cost == cost && rename_cost > 0 {
            self.menu.only_renaming = true;
            xp = xp.min(39)
        }
        self.menu.data.insert(0, xp);
        if cost <= 0 || xp >= 40 && self.source.game_mode != 1 {
            self.set(2, Stack::default(), true);
            return Ok(());
        }
        let mut repair =
            number(&output, "minecraft:repair_cost").max(number(&b, "minecraft:repair_cost"));
        if !self.menu.only_renaming {
            repair = (repair as i64 * 2 + 1).min(i32::MAX as i64) as i32
        }
        changes.insert("minecraft:repair_cost".into(), json!(repair));
        changes.insert(
            chant_key(&output, self.content).into(),
            Value::Object(merged),
        );
        output = persistent::with_components(self.content, &output, &changes)?;
        self.set(2, output, true);
        Ok(())
    }
}
