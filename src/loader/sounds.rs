//! Schema-2 sounds and the wire-3 optional sound command. Admission precedes ACK.
use crate::{
    audio::{PreparedSound, SoundCategory, SoundRequest},
    protocol::Result,
};
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

const MAX_SOUNDS: usize = 64;
const MAX_STARTS_PER_TICK: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Declaration {
    id: String,
}

#[derive(Default)]
pub(crate) struct Registry {
    clips: BTreeMap<String, Arc<PreparedSound>>,
    started_tick: Option<u64>,
    started: BTreeSet<String>,
}

pub(crate) struct Resolved {
    pub id: String,
    pub request: Option<SoundRequest>,
    pub clip: Arc<PreparedSound>,
}

impl Registry {
    /// Only the admission owner may supply the exact paths of hash-verified bytes.
    /// Decode into staging; a rejected bundle cannot leave a partly active registry.
    pub(crate) fn activate(
        &mut self,
        owner: &str,
        entries: &[Value],
        verified_paths: &BTreeMap<String, Arc<[u8]>>,
    ) -> Result<()> {
        if entries.len() > MAX_SOUNDS - self.clips.len() {
            return Err("Loader activated sound registry bound".into());
        }
        if !entries.is_empty()
            && verified_paths.contains_key(&format!("assets/{owner}/sounds.json"))
        {
            return Err("Loader asset collides with generated sound index".into());
        }
        let mut staged = BTreeMap::new();
        for entry in entries {
            let declaration: Declaration = serde_json::from_value(entry.clone())
                .map_err(|e| format!("invalid Loader sound declaration: {e}"))?;
            super::owned(&declaration.id, owner)?;
            if self.clips.contains_key(&declaration.id) || staged.contains_key(&declaration.id) {
                return Err("duplicate activated Loader sound".into());
            }
            let (_, path) = declaration
                .id
                .split_once(':')
                .ok_or("Loader sound namespace missing")?;
            let path = format!("assets/{owner}/sounds/{path}.ogg");
            super::canonical_path(&path, 256)?;
            let bytes = verified_paths
                .get(&path)
                .ok_or("Loader sound is missing its verified Ogg asset")?;
            let clip = Arc::new(PreparedSound::from_ogg(bytes.clone())?);
            staged.insert(declaration.id, clip);
        }
        self.clips.extend(staged);
        Ok(())
    }

    pub(crate) fn ids(&self) -> impl Iterator<Item = &str> {
        self.clips.keys().map(String::as_str)
    }

    /// Optional malformed/unauthorized packets return an error for an explicit rejection
    /// event, never a guessed sound. Starts are bounded like loader-platform-common.
    pub(crate) fn resolve(
        &mut self,
        payload: &[u8],
        connection_active: bool,
        tick: u64,
    ) -> Result<Resolved> {
        if !connection_active {
            return Err("Loader sound requires an active acknowledged connection".into());
        }
        if !(6..=165).contains(&payload.len()) {
            return Err("Loader sound payload byte bound".into());
        }
        if u16::from_be_bytes([payload[0], payload[1]]) != 3 {
            return Err("Loader sound requires wire protocol 3".into());
        }
        let mode = payload[2];
        if mode > 2 {
            return Err("invalid Loader sound mode".into());
        }
        let length = u16::from_be_bytes([payload[3], payload[4]]) as usize;
        let tail = match mode {
            0 => 0,
            1 => 8,
            _ => 32,
        };
        if !(1..=128).contains(&length) || payload.len() != 5 + length + tail {
            return Err("Loader sound payload length mismatch".into());
        }
        let id = std::str::from_utf8(&payload[5..5 + length])
            .map_err(|_| "invalid Loader sound UTF-8")?;
        let clip = self.clips.get(id).ok_or("Loader sound is not activated")?;
        if mode == 0 {
            return Ok(Resolved {
                id: id.to_owned(),
                request: None,
                clip: clip.clone(),
            });
        }
        let offset = 5 + length;
        let volume = f32::from_be_bytes(
            payload[offset..offset + 4]
                .try_into()
                .map_err(|_| "Loader sound volume missing")?,
        );
        let pitch = f32::from_be_bytes(
            payload[offset + 4..offset + 8]
                .try_into()
                .map_err(|_| "Loader sound pitch missing")?,
        );
        if !volume.is_finite()
            || !(0. ..=1.).contains(&volume)
            || !pitch.is_finite()
            || !(0.5..=2.).contains(&pitch)
        {
            return Err("invalid Loader sound volume/pitch".into());
        }
        let mut position = [0.; 3];
        if mode == 2 {
            for (axis, value) in position.iter_mut().enumerate() {
                let start = offset + 8 + axis * 8;
                *value = f64::from_be_bytes(
                    payload[start..start + 8]
                        .try_into()
                        .map_err(|_| "Loader sound position missing")?,
                );
            }
            if position
                .iter()
                .zip([30_000_000., 20_000_000., 30_000_000.])
                .any(|(value, bound)| !value.is_finite() || value.abs() > bound)
            {
                return Err("invalid Loader sound position".into());
            }
        }
        if self.started_tick != Some(tick) {
            self.started_tick = Some(tick);
            self.started.clear();
        }
        if self.started.len() >= MAX_STARTS_PER_TICK || self.started.contains(id) {
            return Err("Loader sound per-tick start bound".into());
        }
        self.started.insert(id.to_owned());
        let request = if mode == 1 {
            SoundRequest::relative(id, SoundCategory::Master, volume, pitch, 0)
        } else {
            SoundRequest::positional(id, SoundCategory::Master, position, volume, pitch, 0)
        };
        Ok(Resolved {
            id: id.to_owned(),
            request: Some(request),
            clip: clip.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const OGG: &[u8] =
        include_bytes!("../../fixtures/content/assets/solaris_fixture/sounds/confirmation.ogg");

    fn assets(names: &[String]) -> BTreeMap<String, Arc<[u8]>> {
        names
            .iter()
            .map(|name| (format!("assets/example/sounds/{name}.ogg"), Arc::from(OGG)))
            .collect()
    }
    fn activate(names: &[String]) -> Registry {
        let entries: Vec<_> = names
            .iter()
            .map(|name| json!({"id":format!("example:{name}")}))
            .collect();
        let mut registry = Registry::default();
        registry
            .activate("example", &entries, &assets(names))
            .unwrap();
        registry
    }
    fn payload(id: &str, mode: u8, volume: f32, pitch: f32, position: [f64; 3]) -> Vec<u8> {
        let mut bytes = vec![0, 3, mode];
        bytes.extend((id.len() as u16).to_be_bytes());
        bytes.extend(id.as_bytes());
        if mode != 0 {
            bytes.extend(volume.to_be_bytes());
            bytes.extend(pitch.to_be_bytes());
        }
        if mode == 2 {
            for value in position {
                bytes.extend(value.to_be_bytes());
            }
        }
        bytes
    }
    fn tone() -> Registry {
        activate(&["tone".to_owned()])
    }

    #[test]
    fn declarations_require_owned_verified_decodable_assets_transactionally() {
        let mut registry = tone();
        let paths = assets(&["second".into()]);
        for entries in [
            vec![json!({"id":"other:second"})],
            vec![json!({"id":"example:second","file":"second.ogg"})],
            vec![json!({"id":"example:missing"})],
            vec![
                json!({"id":"example:second"}),
                json!({"id":"example:second"}),
            ],
            vec![json!({"id":"example:second"}), json!({"id":"example:tone"})],
        ] {
            assert!(registry.activate("example", &entries, &paths).is_err());
            assert_eq!(registry.ids().collect::<Vec<_>>(), ["example:tone"]);
        }
        let mut corrupt = paths.clone();
        corrupt.insert(
            "assets/example/sounds/second.ogg".into(),
            Arc::from(&b"OggSbroken"[..]),
        );
        assert!(
            registry
                .activate("example", &[json!({"id":"example:second"})], &corrupt)
                .is_err()
        );
        assert_eq!(registry.ids().collect::<Vec<_>>(), ["example:tone"]);
        registry
            .activate("example", &[json!({"id":"example:second"})], &paths)
            .unwrap();
        assert_eq!(
            registry.ids().collect::<Vec<_>>(),
            ["example:second", "example:tone"]
        );
    }

    #[test]
    fn registry_bound_and_generated_catalog_collision_are_enforced() {
        let names: Vec<_> = (0..64).map(|i| format!("tone{i}")).collect();
        let mut registry = activate(&names);
        assert!(
            registry
                .activate(
                    "example",
                    &[json!({"id":"example:extra"})],
                    &assets(&["extra".into()])
                )
                .is_err()
        );
        assert_eq!(registry.ids().count(), 64);
        let mut paths = assets(&["tone".into()]);
        paths.insert("assets/example/sounds.json".into(), Arc::from(&b"{}"[..]));
        assert!(
            Registry::default()
                .activate("example", &[json!({"id":"example:tone"})], &paths)
                .is_err()
        );
    }

    #[test]
    fn wire_modes_connection_fence_and_tick_admission_are_real_transitions() {
        let mut registry = tone();
        let play = payload("example:tone", 1, 1., 0.5, [0.; 3]);
        assert!(registry.resolve(&play, false, 7).is_err());
        let command = registry.resolve(&play, true, 7).unwrap();
        assert!(matches!(
            command.request.unwrap().source,
            crate::audio::SoundOrigin::Relative
        ));
        assert!(registry.resolve(&play, true, 7).is_err());
        let stop = payload("example:tone", 0, 1., 1., [0.; 3]);
        assert!(registry.resolve(&stop, true, 7).unwrap().request.is_none());
        // Stop does not erase the start budget for this tick.
        assert!(registry.resolve(&play, true, 7).is_err());
        let position = [-30_000_000., 20_000_000., 30_000_000.];
        let command = registry
            .resolve(&payload("example:tone", 2, 0., 2., position), true, 8)
            .unwrap();
        assert!(
            matches!(command.request.unwrap().source, crate::audio::SoundOrigin::Position(p) if p == position)
        );
        assert!(
            registry
                .resolve(&payload("other:tone", 0, 1., 1., [0.; 3]), true, 9)
                .is_err()
        );
    }

    #[test]
    fn malformed_wire_and_numeric_edges_do_not_consume_tick_budget() {
        let play = payload("example:tone", 2, 1., 1., [0.; 3]);
        let mut malformed = Vec::new();
        for length in 0..play.len() {
            malformed.push(play[..length].to_vec());
        }
        let mut trailing = play.clone();
        trailing.push(0);
        malformed.push(trailing);
        let mut protocol = play.clone();
        protocol[1] = 2;
        malformed.push(protocol);
        let mut mode = play.clone();
        mode[2] = 3;
        malformed.push(mode);
        let mut utf8 = play.clone();
        utf8[5] = 0xff;
        malformed.push(utf8);
        for volume in [-0.01, 1.01, f32::NAN, f32::INFINITY] {
            malformed.push(payload("example:tone", 1, volume, 1., [0.; 3]));
        }
        for pitch in [0.49, 2.01, f32::NAN, f32::INFINITY] {
            malformed.push(payload("example:tone", 1, 1., pitch, [0.; 3]));
        }
        for p in [
            [30_000_001., 0., 0.],
            [0., 20_000_001., 0.],
            [0., 0., -30_000_001.],
            [f64::NAN, 0., 0.],
        ] {
            malformed.push(payload("example:tone", 2, 1., 1., p));
        }
        for bytes in malformed {
            let mut registry = tone();
            assert!(registry.resolve(&bytes, true, 1).is_err());
            assert!(registry.resolve(&play, true, 1).is_ok());
        }
    }

    #[test]
    fn eight_distinct_starts_per_tick_reset_on_next_tick() {
        let names: Vec<_> = (0..9).map(|i| format!("tone{i}")).collect();
        let mut registry = activate(&names);
        for i in 0..8 {
            registry
                .resolve(
                    &payload(&format!("example:tone{i}"), 1, 1., 1., [0.; 3]),
                    true,
                    10,
                )
                .unwrap();
        }
        let ninth = payload("example:tone8", 1, 1., 1., [0.; 3]);
        assert!(registry.resolve(&ninth, true, 10).is_err());
        assert!(registry.resolve(&ninth, true, 11).is_ok());
    }
}
