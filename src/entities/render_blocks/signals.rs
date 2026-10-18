use crate::{
    entities::blocks::SignalState,
    protocol::{Result, content::Content},
};
use serde_json::Value;
pub(crate) fn parse(kind: &str, data: &Value, content: &Content) -> Result<SignalState> {
    let mut state = SignalState::default();
    if kind == "minecraft:comparator" {
        state.comparator_output = data["OutputSignal"].as_i64().unwrap_or(0) as i32;
    }
    if kind == "minecraft:test_block" {
        state.test_powered = data["powered"]
            .as_bool()
            .unwrap_or_else(|| data["powered"].as_i64() == Some(1));
    }
    if kind == "minecraft:jukebox" {
        if let Some(ticks) = data["ticks_since_song_started"].as_i64() {
            let item = &data["RecordItem"];
            if let Some(name) = item["id"].as_str() {
                let id = content
                    .item_names
                    .iter()
                    .find(|(_, n)| n.as_str() == name)
                    .map(|(id, _)| *id)
                    .ok_or("unknown observed jukebox record item")?;
                let defaults = content.item_components(id)?;
                let components = &item["components"];
                let playable = if components.get("!minecraft:jukebox_playable").is_some() {
                    None
                } else {
                    components
                        .get("minecraft:jukebox_playable")
                        .or_else(|| defaults.get("minecraft:jukebox_playable"))
                };
                if let Some(playable) = playable {
                    let name = playable
                        .as_str()
                        .ok_or("source jukebox playable holder is not an identifier")?;
                    let song = content
                        .registry_entries("minecraft:jukebox_song")?
                        .iter()
                        .find(|(n, _)| n == name)
                        .map(|(_, v)| v)
                        .ok_or("observed jukebox song holder not registered")?;
                    let seconds = song["length_in_seconds"]
                        .as_f64()
                        .ok_or("source jukebox song length not observed")?
                        as f32;
                    if !seconds.is_finite() || seconds <= 0. {
                        return Err("source jukebox song length invalid".into());
                    }
                    let duration = (seconds * 20.).ceil() as i64 + 20;
                    if ticks < duration {
                        state.jukebox_elapsed = Some(ticks);
                        state.jukebox_duration = Some(duration);
                    }
                }
            }
        }
    }
    Ok(state)
}
