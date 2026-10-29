//! Item placement components project over received block data until its next NBT update.
//! Source: pinned 26.1.2 BlockEntity.applyComponents and the twelve implicit overrides.
use super::{BlockEntity, MAX_BLOCK_ENTITIES, Store};
use crate::{inventory::ComponentPatch, protocol::Result};
use serde_json::{Map, Value};
use std::{collections::BTreeMap, sync::Arc};

impl BlockEntity {
    pub(crate) fn data_value(&self) -> &Value {
        self.predicted_data.as_deref().unwrap_or(self.data.as_ref())
    }
}

impl Store {
    /// Level.getBlockEntity(IMMEDIATE) creates a sign from its block state even
    /// when no block-entity packet has arrived. Empty raw NBT is not a receipt.
    pub(crate) fn ensure_sign_block_entity(
        &mut self,
        position: [i32; 3],
        state: u32,
        content: &crate::protocol::content::Content,
        tick: u64,
    ) -> Result<bool> {
        let metadata = content.placement_metadata(state)?;
        if !matches!(
            metadata["block_entity_type"].as_str(),
            Some("minecraft:sign" | "minecraft:hanging_sign")
        ) {
            return Ok(false);
        }
        self.ensure_block_entity(position, state, content, tick)
    }
    /// Sign edits project over received NBT, just like placement components.
    pub(crate) fn predict_sign_text(
        &mut self,
        position: [i32; 3],
        front: bool,
        lines: &[String; 4],
    ) -> Result<()> {
        let Some(entity) = self.block_entities.get(&position).filter(|entity| {
            matches!(
                entity.type_name.as_str(),
                "minecraft:sign" | "minecraft:hanging_sign"
            )
        }) else {
            return Ok(());
        };
        if self.predicted_block_data.len() >= MAX_BLOCK_ENTITIES
            && !self.predicted_block_data.contains_key(&position)
        {
            return Err("block entity prediction capacity".into());
        }
        let mut data = entity.data_value().clone();
        let side = if front { "front_text" } else { "back_text" };
        if !data[side].is_object() {
            data[side] = serde_json::json!({"color":"black","has_glowing_text":false});
        }
        data[side]["messages"] = serde_json::json!(lines);
        // SignText.setMessage updates both unfiltered and filtered messages.
        if data[side].get("filtered_messages").is_some() {
            data[side]["filtered_messages"] = serde_json::json!(lines);
        }
        let data = Arc::new(data);
        self.predicted_block_data.insert(position, data.clone());
        self.revision += 1;
        let entity = self.block_entities.get_mut(&position).unwrap();
        entity.predicted_data = Some(data);
        entity.revision = self.revision;
        Ok(())
    }
    pub(crate) fn predict_block_entity_components(
        &mut self,
        position: [i32; 3],
        components: Arc<BTreeMap<String, Value>>,
        patch: Arc<ComponentPatch>,
        type_name: &str,
    ) -> Result<()> {
        if self.predicted_block_data.len() >= MAX_BLOCK_ENTITIES
            && !self.predicted_block_data.contains_key(&position)
        {
            return Err("block entity prediction capacity".into());
        }
        let received = self
            .block_entities
            .get(&position)
            .filter(|e| e.type_name == type_name);
        let mut data = match received {
            Some(e) => e
                .data_value()
                .as_object()
                .ok_or("block entity data is not a compound")?
                .clone(),
            None => Map::new(),
        };
        let randomizable = matches!(
            type_name,
            "minecraft:barrel"
                | "minecraft:chest"
                | "minecraft:trapped_chest"
                | "minecraft:crafter"
                | "minecraft:dispenser"
                | "minecraft:dropper"
                | "minecraft:hopper"
                | "minecraft:shulker_box"
        );
        let slots = match type_name {
            "minecraft:barrel"
            | "minecraft:chest"
            | "minecraft:trapped_chest"
            | "minecraft:shulker_box" => Some(27),
            "minecraft:crafter" | "minecraft:dispenser" | "minecraft:dropper" => Some(9),
            "minecraft:hopper" | "minecraft:brewing_stand" => Some(5),
            "minecraft:furnace"
            | "minecraft:blast_furnace"
            | "minecraft:smoker"
            | "minecraft:shelf" => Some(3),
            "minecraft:campfire" => Some(4),
            "minecraft:chiseled_bookshelf" => Some(6),
            "minecraft:decorated_pot" => Some(1),
            _ => None,
        };
        let base_container = randomizable
            || matches!(
                type_name,
                "minecraft:brewing_stand"
                    | "minecraft:furnace"
                    | "minecraft:blast_furnace"
                    | "minecraft:smoker"
            );
        let fields: &[(&str, &str)] = if base_container || type_name == "minecraft:beacon" {
            &[
                ("minecraft:custom_name", "CustomName"),
                ("minecraft:lock", "lock"),
            ]
        } else {
            match type_name {
                "minecraft:banner" => &[
                    ("minecraft:banner_patterns", "patterns"),
                    ("minecraft:custom_name", "CustomName"),
                ],
                "minecraft:skull" => &[
                    ("minecraft:profile", "profile"),
                    ("minecraft:note_block_sound", "note_block_sound"),
                    ("minecraft:custom_name", "custom_name"),
                ],
                "minecraft:command_block" | "minecraft:enchanting_table" => {
                    &[("minecraft:custom_name", "CustomName")]
                }
                "minecraft:beehive" => &[("minecraft:bees", "bees")],
                "minecraft:decorated_pot" => &[("minecraft:pot_decorations", "sherds")],
                _ => &[],
            }
        };
        for &(component, field) in fields {
            if let Some(value) = components.get(component) {
                data.insert(field.to_owned(), value.clone());
            } else {
                data.remove(field);
            }
        }
        if let Some(slots) = slots {
            let entries = components
                .get("minecraft:container")
                .map(|v| v.as_array().ok_or("container component is not a list"))
                .transpose()?;
            if type_name == "minecraft:decorated_pot" {
                let first = entries
                    .and_then(|entries| entries.iter().find(|v| v["slot"].as_u64() == Some(0)));
                if let Some(first) = first {
                    data.insert("item".into(), first["item"].clone());
                } else {
                    data.remove("item");
                }
            } else {
                let mut items = Vec::with_capacity(entries.map_or(0, Vec::len).min(slots));
                for entry in entries.into_iter().flatten() {
                    let slot = entry["slot"]
                        .as_u64()
                        .ok_or("container component slot missing")?;
                    if slot >= slots as u64 {
                        continue;
                    }
                    let mut item = entry["item"]
                        .as_object()
                        .ok_or("container component item missing")?
                        .clone();
                    item.insert("Slot".into(), Value::from(slot));
                    items.push(Value::Object(item));
                }
                data.insert("Items".into(), Value::Array(items));
            }
        }
        if randomizable {
            if let Some(loot) = components.get("minecraft:container_loot") {
                data.insert(
                    "LootTable".into(),
                    loot.get("loot_table")
                        .ok_or("container loot table missing")?
                        .clone(),
                );
                data.insert(
                    "LootTableSeed".into(),
                    loot.get("seed").cloned().unwrap_or_else(|| Value::from(0)),
                );
            }
        }
        // applyComponents keeps added patch entries, not effective item defaults.
        let retained = patch
            .added
            .values()
            .filter(|value| {
                let name = value.name.as_str();
                !matches!(
                    name,
                    "minecraft:block_entity_data" | "minecraft:block_state"
                ) && !fields.iter().any(|&(component, _)| component == name)
                    && !(slots.is_some() && name == "minecraft:container")
                    && !(randomizable && name == "minecraft:container_loot")
            })
            .map(|value| Ok((value.name.clone(), value.value.json()?)))
            .collect::<Result<Map<String, Value>>>()?;
        data.insert("components".into(), Value::Object(retained));
        let data = Arc::new(Value::Object(data));
        self.predicted_block_data.insert(position, data.clone());
        self.revision += 1;
        if let Some(entity) = self
            .block_entities
            .get_mut(&position)
            .filter(|e| e.type_name == type_name)
        {
            entity.predicted_data = Some(data);
            entity.revision = self.revision;
        }
        Ok(())
    }
}
