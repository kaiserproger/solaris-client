//! Comparator prediction reads the client's received block/entity state, never server internals.
use super::{Metadata, Store};
use crate::protocol::{Result, content::Content, world::World};
use serde_json::Value;

fn integer(data: &Value, key: &str, default: i32) -> Result<i32> {
    match data.get(key) {
        None => Ok(default),
        Some(value) => value
            .as_i64()
            .and_then(|v| i32::try_from(v).ok())
            .ok_or_else(|| format!("invalid received analogue field {key}")),
    }
}
fn item_count(item: &Value) -> Result<i32> {
    integer(item, "count", 1).map(|count| {
        if item["id"].as_str().is_none_or(|id| id == "minecraft:air") {
            0
        } else {
            count.max(0)
        }
    })
}
fn item_fraction(item: &Value, content: &Content) -> Result<f32> {
    let count = item_count(item)?;
    if count == 0 {
        return Ok(0.);
    }
    let name = item["id"]
        .as_str()
        .ok_or("received analogue item lacks id")?;
    let id = content
        .item_names
        .iter()
        .find(|(_, value)| value.as_str() == name)
        .map(|(id, _)| *id)
        .ok_or("unknown received analogue item")?;
    let components = content.item_components(id)?;
    let max = item["components"]
        .get("minecraft:max_stack_size")
        .or_else(|| components.get("minecraft:max_stack_size"))
        .and_then(Value::as_i64)
        .filter(|value| (1..=99).contains(value))
        .ok_or("received analogue item max stack size unavailable")? as i32;
    Ok(count as f32 / max.min(64) as f32)
}
fn container_sum(
    data: &Value,
    slots: usize,
    content: &Content,
    sum: &mut f32,
    occupied: &mut bool,
) -> Result<()> {
    let Some(items) = data.get("Items") else {
        return Ok(());
    };
    let items = items
        .as_array()
        .ok_or("received container Items is not a list")?;
    let mut seen = 0u64;
    for item in items {
        let slot = integer(item, "Slot", -1)?;
        if slot < 0 || slot as usize >= slots || seen & (1 << slot) != 0 {
            return Err("invalid received analogue container slot".into());
        }
        seen |= 1 << slot;
        let fraction = item_fraction(item, content)?;
        *sum += fraction;
        *occupied |= fraction > 0.;
    }
    Ok(())
}
fn fullness(data: &Value, slots: usize, content: &Content) -> Result<i32> {
    let mut sum = 0.;
    let mut occupied = false;
    container_sum(data, slots, content, &mut sum, &mut occupied)?;
    Ok((sum / slots as f32 * 14.).floor() as i32 + i32::from(occupied))
}
impl Store {
    pub(crate) fn block_analog_output(
        &self,
        position: [i32; 3],
        state: u32,
        content: &Content,
        _face: u8,
        world: &World,
        _tick: u64,
    ) -> Result<Option<i32>> {
        let Some(owner) = content.block_analog_output_owner(state)? else {
            return Ok(None);
        };
        let owner = owner.rsplit('.').next().unwrap_or(owner);
        let properties = content
            .state_properties(state)
            .ok_or("analogue state properties unavailable")?;
        match owner {
            "CopperBulbBlock" => {
                return Ok(Some(
                    if properties.get("lit").map(String::as_str) == Some("true") {
                        15
                    } else {
                        0
                    },
                ));
            }
            "LayeredCauldronBlock" => {
                return Ok(Some(
                    properties
                        .get("level")
                        .ok_or("cauldron level unavailable")?
                        .parse::<i32>()
                        .map_err(|_| "invalid cauldron level")?,
                ));
            }
            "LavaCauldronBlock" => return Ok(Some(3)),
            "DetectorRailBlock" => {
                if properties.get("powered").map(String::as_str) != Some("true") {
                    return Ok(Some(0));
                }
                let area = crate::player::Bounds::new([0.2, 0., 0.2, 0.8, 0.8, 0.8])
                    .moved(position.map(f64::from));
                for entity in self.client_entities().filter(|entity| {
                    matches!(
                        entity.type_name.as_str(),
                        "minecraft:command_block_minecart"
                            | "minecraft:chest_minecart"
                            | "minecraft:hopper_minecart"
                    )
                }) {
                    if self.entity_bounds(entity.handle, _tick)?.intersects(area) {
                        return Err("detector rail minecart analogue state not received".into());
                    }
                }
                return Ok(Some(0));
            }
            _ => {}
        }
        let block = self
            .block_entities
            .get(&position)
            .ok_or("analogue block entity not received")?;
        let data = block.data_value();
        let output = match owner {
            "ComparatorBlock" => block.animation.signals.comparator_output,
            "CommandBlock" => integer(data, "SuccessCount", 0)?,
            // Source initializes this field to zero and updates it only in serverTick;
            // its load/update NBT never contains the server's outputSignal.
            "CreakingHeartBlock" => 0,
            "SculkSensorBlock" | "CalibratedSculkSensorBlock" => {
                if properties.get("sculk_sensor_phase").map(String::as_str) == Some("active") {
                    integer(data, "last_vibration_frequency", 0)?
                } else {
                    0
                }
            }
            "JukeboxBlock" => {
                let item = &data["RecordItem"];
                if item_count(item)? == 0 {
                    0
                } else {
                    let name = item["id"].as_str().ok_or("jukebox record lacks id")?;
                    let id = content
                        .item_names
                        .iter()
                        .find(|(_, value)| value.as_str() == name)
                        .map(|(id, _)| *id)
                        .ok_or("unknown jukebox item")?;
                    let defaults = content.item_components(id)?;
                    let playable = item["components"]
                        .get("minecraft:jukebox_playable")
                        .or_else(|| defaults.get("minecraft:jukebox_playable"));
                    match playable {
                        None => 0,
                        Some(song) => {
                            let song = song.as_str().ok_or("inline jukebox song not supported")?;
                            let entry = content
                                .registry_entries("minecraft:jukebox_song")?
                                .iter()
                                .find(|(name, _)| name == song)
                                .map(|(_, value)| value)
                                .ok_or("jukebox song not received")?;
                            integer(entry, "comparator_output", 0)?
                        }
                    }
                }
            }
            "LecternBlock" => {
                let book = &data["Book"];
                let has_book = item_count(book)? > 0;
                let pages = book["components"]["minecraft:written_book_content"]["pages"]
                    .as_array()
                    .or_else(|| {
                        book["components"]["minecraft:writable_book_content"]["pages"].as_array()
                    })
                    .map_or(0, Vec::len);
                let fraction = if pages > 1 {
                    integer(data, "Page", 0)? as f32 / (pages - 1) as f32
                } else {
                    1.
                };
                (fraction * 14.).floor() as i32 + i32::from(has_book)
            }
            "CrafterBlock" => {
                let mut occupied = [false; 9];
                if let Some(items) = data.get("Items") {
                    for item in items.as_array().ok_or("crafter Items is not a list")? {
                        let slot = integer(item, "Slot", -1)?;
                        if !(0..9).contains(&slot) {
                            return Err("crafter slot outside bounds".into());
                        }
                        occupied[slot as usize] |= item_count(item)? > 0;
                    }
                }
                if let Some(disabled) = data.get("disabled_slots") {
                    for slot in disabled
                        .as_array()
                        .ok_or("crafter disabled slots is not a list")?
                    {
                        let slot = slot
                            .as_u64()
                            .filter(|slot| *slot < 9)
                            .ok_or("crafter disabled slot outside bounds")?;
                        occupied[slot as usize] = true;
                    }
                }
                occupied.into_iter().filter(|value| *value).count() as i32
            }
            "DecoratedPotBlock" => {
                let fraction = data
                    .get("item")
                    .map(|item| item_fraction(item, content))
                    .transpose()?
                    .unwrap_or(0.);
                (fraction * 14.).floor() as i32 + i32::from(fraction > 0.)
            }
            "ChestBlock" => {
                let mut output = fullness(data, 27, content)?;
                if let Some(kind) = content
                    .state_properties(state)
                    .and_then(|props| props.get("type"))
                {
                    if kind != "single" {
                        let facing = content
                            .state_properties(state)
                            .and_then(|props| props.get("facing"))
                            .ok_or("double chest facing missing")?;
                        let [dx, dz] = match (facing.as_str(), kind.as_str()) {
                            ("north", "left") | ("south", "right") => [1, 0],
                            ("north", "right") | ("south", "left") => [-1, 0],
                            ("east", "left") | ("west", "right") => [0, 1],
                            ("east", "right") | ("west", "left") => [0, -1],
                            _ => return Err("invalid double chest properties".into()),
                        };
                        let neighbor = [
                            position[0]
                                .checked_add(dx)
                                .ok_or("chest position overflow")?,
                            position[1],
                            position[2]
                                .checked_add(dz)
                                .ok_or("chest position overflow")?,
                        ];
                        let other_state = world
                            .block(neighbor)
                            .ok_or("double chest neighbor not loaded")?;
                        if content.name(other_state) != content.name(state) {
                            return Err("double chest neighbor mismatch".into());
                        }
                        let other = self
                            .block_entity_data(neighbor)
                            .ok_or("double chest neighbor not received")?;
                        let mut sum = 0.;
                        let mut occupied = false;
                        container_sum(data, 27, content, &mut sum, &mut occupied)?;
                        container_sum(other, 27, content, &mut sum, &mut occupied)?;
                        output = (sum / 54. * 14.).floor() as i32 + i32::from(occupied);
                    }
                }
                output
            }
            "AbstractFurnaceBlock" | "FurnaceBlock" | "BlastFurnaceBlock" | "SmokerBlock" => {
                fullness(data, 3, content)?
            }
            "HopperBlock" | "BrewingStandBlock" => fullness(data, 5, content)?,
            "DispenserBlock" | "DropperBlock" => fullness(data, 9, content)?,
            "BarrelBlock" | "ShulkerBoxBlock" => fullness(data, 27, content)?,
            _ => return Err(format!("unsupported received analogue owner {owner}")),
        };
        Ok(Some(output))
    }
    pub(crate) fn item_frame_analog_output(
        &self,
        position: [i32; 3],
        face: u8,
        _tick: u64,
    ) -> Result<Option<i32>> {
        if face > 5 {
            return Err("item frame comparator direction outside bounds".into());
        }
        let mut found = None;
        for entity in self.client_entities().filter(|entity| {
            matches!(
                entity.type_name.as_str(),
                "minecraft:item_frame" | "minecraft:glow_item_frame"
            )
        }) {
            if entity.spawn_data != i32::from(face)
                || entity.position.map(|v| v.floor() as i32) != position
            {
                continue;
            }
            if found.is_some() {
                return Ok(None);
            }
            let signal = match self.metadata(entity.handle, "DATA_ITEM")? {
                Some(Metadata::Item(stack)) if !stack.empty() => {
                    self.metadata(entity.handle, "DATA_ROTATION")?
                        .and_then(Metadata::int)
                        .ok_or("item frame rotation unavailable")?
                        % 8
                        + 1
                }
                Some(Metadata::Item(_)) => 0,
                _ => return Err("item frame item unavailable".into()),
            };
            found = Some(signal);
        }
        Ok(found)
    }
}
