//! Local oracle data is not shipped: probe Block.BLOCK_STATE_REGISTRY / BlockState.getSoundType
//! in the pinned client JAR, then store its version/hash alongside the ignored table.
use super::{SoundCategory, SoundOrigin, SoundRequest, catalog::identifier};
use crate::protocol::{Result, content::json_file};
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy)]
pub enum BlockSoundAction {
    Hit,
    Break,
    Place,
    Step,
    MuffledStep,
    Fall,
}
struct Material {
    events: [String; 5],
    volume: f32,
    pitch: f32,
}
pub struct BlockSounds {
    materials: Vec<Material>,
    states: Vec<usize>,
}
impl BlockSounds {
    pub fn load(path: &Path, expected_client_sha1: &str) -> Result<Self> {
        Self::parse(&json_file(path, 16 << 20)?, expected_client_sha1)
    }
    fn parse(value: &Value, expected_client_sha1: &str) -> Result<Self> {
        if expected_client_sha1.len() != 40
            || value["version"].as_str() != Some("26.3")
            || value["client_sha1"].as_str() != Some(expected_client_sha1)
        {
            return Err("block sound oracle version/client digest mismatch".into());
        }
        let types = value["types"]
            .as_array()
            .ok_or("block sound type table missing")?;
        let rows = value["states"]
            .as_array()
            .ok_or("block sound state table missing")?;
        if types.is_empty() || types.len() > 1024 || rows.is_empty() || rows.len() > 131072 {
            return Err("block sound oracle count bound".into());
        }
        let mut materials = Vec::with_capacity(types.len());
        for t in types {
            let mut events = [
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ];
            for (i, key) in ["hit", "break", "place", "step", "fall"].iter().enumerate() {
                events[i] = identifier(t[*key].as_str().ok_or("block material sound missing")?)?;
            }
            let volume = t["volume"]
                .as_f64()
                .ok_or("block material volume missing")? as f32;
            let pitch = t["pitch"].as_f64().ok_or("block material pitch missing")? as f32;
            if !volume.is_finite()
                || !pitch.is_finite()
                || !(0. ..=16.).contains(&volume)
                || !(0.01..=16.).contains(&pitch)
            {
                return Err("block material gain bound".into());
            }
            materials.push(Material {
                events,
                volume,
                pitch,
            });
        }
        let mut states = Vec::with_capacity(rows.len());
        for (id, row) in rows.iter().enumerate() {
            if row["id"].as_u64() != Some(id as u64) {
                return Err("block sound state IDs must be dense and ordered".into());
            }
            let m = row["sound_type"]
                .as_u64()
                .filter(|n| *n < materials.len() as u64)
                .ok_or("block sound type ID out of range")?;
            states.push(m as usize);
        }
        Ok(Self { materials, states })
    }
    /// Call only on a genuine action. The caller owns air/fluid checks, prediction and cadence.
    /// Hit cadence is every fourth destroy tick, not each render frame.
    pub fn request(
        &self,
        state_id: u32,
        action: BlockSoundAction,
        position: [f64; 3],
        seed: u64,
    ) -> Result<SoundRequest> {
        let material = &self.materials[*self
            .states
            .get(state_id as usize)
            .ok_or("unknown block sound state")?];
        // 26.1.2 MultiPlayerGameMode.continueDestroyBlock, BlockItem.place,
        // LevelEventHandler.levelEvent(2001), Entity.playStepSound/playMuffledStepSound.
        let (index, volume, pitch, category) = match action {
            BlockSoundAction::Hit => (
                0,
                (material.volume + 1.) / 8.,
                material.pitch * 0.5,
                SoundCategory::Blocks,
            ),
            BlockSoundAction::Break => (
                1,
                (material.volume + 1.) / 2.,
                material.pitch * 0.8,
                SoundCategory::Blocks,
            ),
            BlockSoundAction::Place => (
                2,
                (material.volume + 1.) / 2.,
                material.pitch * 0.8,
                SoundCategory::Blocks,
            ),
            BlockSoundAction::Step => (
                3,
                material.volume * 0.15,
                material.pitch,
                SoundCategory::Players,
            ),
            BlockSoundAction::MuffledStep => (
                3,
                material.volume * 0.05,
                material.pitch * 0.8,
                SoundCategory::Players,
            ),
            BlockSoundAction::Fall => (
                4,
                material.volume * 0.5,
                material.pitch * 0.75,
                SoundCategory::Players,
            ),
        };
        Ok(SoundRequest {
            event: material.events[index].clone(),
            category,
            source: SoundOrigin::Position(position),
            volume,
            pitch,
            seed,
            looping: false,
        })
    }
    pub fn state_count(&self) -> usize {
        self.states.len()
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn state_lookup_and_action_gains_do_not_guess_names() {
        let hash = "1234567890123456789012345678901234567890";
        let table=BlockSounds::parse(&json!({"version":"26.3","client_sha1":hash,"types":[{"hit":"pack:glass_hit","break":"pack:glass_break","place":"pack:glass_place","step":"pack:glass_step","fall":"pack:glass_fall","volume":0.5,"pitch":1.5}],"states":[{"id":0,"block":"pack:stone_named_glass","sound_type":0}]}),hash).unwrap();
        let hit = table
            .request(0, BlockSoundAction::Hit, [1., 2., 3.], 1)
            .unwrap();
        assert_eq!(hit.event, "pack:glass_hit");
        assert_eq!(hit.volume, 0.1875);
        assert_eq!(hit.pitch, 0.75);
        let place = table
            .request(0, BlockSoundAction::Place, [1., 2., 3.], 1)
            .unwrap();
        assert_eq!(place.volume, 0.75);
        assert_eq!(place.pitch, 1.2);
        assert!(table.request(1, BlockSoundAction::Hit, [0.; 3], 1).is_err());
    }
}
