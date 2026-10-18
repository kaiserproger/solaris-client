use super::{Handle, Metadata, Store, presentation as p};
use crate::{
    inventory::{self, Stack},
    protocol::{Reader, Result, Writer, content::Content},
    ui::hash::Value as Tag,
};
use std::collections::BTreeMap;

impl Store {
    pub(crate) fn has_fishing_hook(&self, owner_id: i32) -> Result<bool> {
        Ok(self.fishing_hooks.get(&owner_id).is_some_and(|handle| {
            self.resolve(*handle).is_ok_and(|entity| {
                !entity
                    .source
                    .as_ref()
                    .is_some_and(|source| source.discarded || source.special.discarded)
            })
        }))
    }
    pub(crate) fn nearest_visible_item(
        &self,
        item_name: &str,
        near: [i32; 3],
        content: &Content,
        _tick: u64,
    ) -> Result<Option<[f64; 3]>> {
        let mut nearest = None;
        let mut nearest_distance = f64::MAX;
        let mut nearest_generation = u64::MAX;
        for entity in self
            .received_entities()
            .filter(|entity| entity.type_name == "minecraft:item")
        {
            let Some(Metadata::Item(stack)) = self.received_metadata(entity.handle, "DATA_ITEM")?
            else {
                continue;
            };
            if stack.empty()
                || content.item_names.get(&stack.id).map(String::as_str) != Some(item_name)
            {
                continue;
            }
            let position = entity.position;
            let distance = (0..3)
                .map(|axis| (position[axis] - f64::from(near[axis]) - 0.5).powi(2))
                .sum::<f64>();
            if distance <= 16.
                && (distance < nearest_distance
                    || (distance == nearest_distance
                        && entity.handle.generation < nearest_generation))
            {
                nearest = Some(position);
                nearest_distance = distance;
                nearest_generation = entity.handle.generation;
            }
        }
        Ok(nearest)
    }

    pub(crate) fn bucket_item_stack(&self, handle: Handle, content: &Content) -> Result<Stack> {
        let entity = self.resolve(handle)?;
        if entity.handle != handle {
            return Err("dragon part is not bucketable".into());
        }
        let item = match entity.type_name.as_str() {
            "minecraft:cod" => "minecraft:cod_bucket",
            "minecraft:salmon" => "minecraft:salmon_bucket",
            "minecraft:pufferfish" => "minecraft:pufferfish_bucket",
            "minecraft:tropical_fish" => "minecraft:tropical_fish_bucket",
            "minecraft:axolotl" => "minecraft:axolotl_bucket",
            "minecraft:tadpole" => "minecraft:tadpole_bucket",
            _ => return Err("entity has no Source bucket item".into()),
        };
        let models = self
            .models_ref()
            .ok_or("entity Source models not activated")?;
        let health = match p::value(models, entity, "DATA_HEALTH_ID") {
            Some(Metadata::Float(value)) => *value,
            None => self.attribute(handle, "minecraft:max_health")? as f32,
            _ => return Err("invalid Source health accessor".into()),
        };
        let mut data = vec![(Tag::string("Health"), Tag::Float(health))];
        for (name, enabled) in [
            (
                "NoAI",
                p::number(models, entity, "DATA_MOB_FLAGS_ID").unwrap_or(0) & 1 != 0,
            ),
            ("Silent", p::boolean(models, entity, "DATA_SILENT")),
            ("NoGravity", p::boolean(models, entity, "DATA_NO_GRAVITY")),
            (
                "Glowing",
                p::number(models, entity, "DATA_SHARED_FLAGS_ID").unwrap_or(0) & 64 != 0,
            ),
        ] {
            if enabled {
                data.push((Tag::string(name), Tag::Byte(1)));
            }
        }
        // Invulnerable and axolotl brain hunting memory are unsynced client fields;
        // their vanilla client constructors leave them false/absent.
        if entity.type_name == "minecraft:axolotl" || entity.type_name == "minecraft:tadpole" {
            let age = if entity.type_name == "minecraft:tadpole" {
                self.source_use_state(handle)?.tadpole_age
            } else if p::boolean(models, entity, "DATA_BABY_ID") {
                -1
            } else {
                1
            };
            data.push((Tag::string("Age"), Tag::Int(age)));
            data.push((
                Tag::string("AgeLocked"),
                Tag::Byte(i8::from(p::boolean(models, entity, "AGE_LOCKED"))),
            ));
        }
        let mut variants = Vec::with_capacity(3);
        match entity.type_name.as_str() {
            "minecraft:salmon" => variants.push((
                "minecraft:salmon_size",
                p::number(models, entity, "DATA_TYPE")
                    .unwrap_or(1)
                    .clamp(0, 2),
            )),
            "minecraft:axolotl" => {
                let value = p::number(models, entity, "DATA_VARIANT").unwrap_or(0);
                variants.push((
                    "minecraft:axolotl_variant",
                    if (0..5).contains(&value) { value } else { 0 },
                ));
            }
            "minecraft:tropical_fish" => {
                let packed = p::number(models, entity, "DATA_ID_TYPE_VARIANT").unwrap_or(0);
                let pattern = packed & 65535;
                let pattern = if pattern & 255 <= 1 && pattern >> 8 < 6 {
                    pattern
                } else {
                    0
                };
                variants.push(("minecraft:tropical_fish_pattern", pattern));
                let base = packed >> 16 & 255;
                let pattern_color = packed >> 24 & 255;
                variants.push((
                    "minecraft:tropical_fish_base_color",
                    if base < 16 { base } else { 0 },
                ));
                variants.push((
                    "minecraft:tropical_fish_pattern_color",
                    if pattern_color < 16 { pattern_color } else { 0 },
                ));
            }
            _ => {}
        }
        let item_id = content
            .item_names
            .iter()
            .find_map(|(id, name)| (name == item).then_some(*id))
            .ok_or("Source bucket item missing")?;
        let component = |name: &str| {
            content
                .components
                .iter()
                .find_map(|(id, observed)| (observed == name).then_some(*id))
                .ok_or_else(|| format!("Source bucket component missing {name}"))
        };
        let mut wire = Writer::default();
        wire.var(1);
        wire.var(item_id as i32);
        wire.var(1 + variants.len() as i32);
        wire.var(0);
        wire.var(component("minecraft:bucket_entity_data")?);
        inventory::nbt::write(&Tag::Map(data), &mut wire)?;
        for (name, value) in variants {
            wire.var(component(name)?);
            wire.var(value);
        }
        let mut reader = Reader::new(&wire.0);
        let mut stack = inventory::read_stack(&mut reader, content)?;
        reader.done()?;
        if let Some(Metadata::OptionalComponent(Some(name))) =
            p::value(models, entity, "DATA_CUSTOM_NAME")
        {
            stack = inventory::stack_with_components(
                content,
                &stack,
                &BTreeMap::from([("minecraft:custom_name".into(), name.clone())]),
            )?;
        }
        Ok(stack)
    }

    pub(crate) fn predict_swing(&mut self, handle: Handle, hand: u8, tick: u64) -> Result<()> {
        if hand > 1 {
            return Err("entity swing hand bound".into());
        }
        if self.resolve(handle)?.handle != handle {
            return Err("dragon part swing prediction".into());
        }
        self.get_mut(handle.network_id).unwrap().animation =
            Some((if hand == 0 { 0 } else { 3 }, tick));
        self.revision += 1;
        Ok(())
    }

    pub(crate) fn predict_effect(
        &mut self,
        handle: Handle,
        effect: i32,
        duration: i32,
        amplifier: u8,
        flags: u8,
        tick: u64,
    ) -> Result<()> {
        let effect = u32::try_from(effect).map_err(|_| "entity effect registry ID bound")?;
        if self.resolve(handle)?.handle != handle {
            return Err("dragon part effect prediction".into());
        }
        self.get_mut(handle.network_id).unwrap().effects.insert(
            effect,
            super::Effect {
                amplifier: u32::from(amplifier),
                duration,
                flags,
                applied_tick: tick,
            },
        );
        self.revision += 1;
        Ok(())
    }

    pub(crate) fn predict_comparator_output(
        &mut self,
        position: [i32; 3],
        signal: i32,
    ) -> Result<()> {
        let block = self
            .block_entities
            .get_mut(&position)
            .ok_or("comparator block entity not observed")?;
        if block.type_name != "minecraft:comparator" {
            return Err("comparator prediction owner mismatch".into());
        }
        block.animation.signals.comparator_output = signal;
        self.revision += 1;
        Ok(())
    }
}
