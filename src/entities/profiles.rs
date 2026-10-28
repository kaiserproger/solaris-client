//! Protocol-777 PlayerInfo and presentation profile lifetimes.
//! Signature validation/download/atlas publication is parent-owned; raw properties never
//! become trusted merely because a server supplied them.
use super::{metadata, Store};
use crate::protocol::{content::TextureRegion, Reader, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::Arc};

const MAX_PROFILE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PROFILES: usize = 100_000;
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "UPPERCASE")]
pub enum PlayerModel {
    Slim,
    Wide,
}
#[derive(Debug, Serialize)]
pub struct ProfileProperty {
    pub name: String,
    pub value: String,
    pub signature: Option<String>,
}
#[derive(Clone, Debug)]
pub struct ProfileTextureRequest {
    pub uuid: String,
    pub generation: u64,
    pub property: Arc<ProfileProperty>,
    pub require_secure: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct ProfileTextures {
    pub skin: Option<TextureRegion>,
    pub cape: Option<TextureRegion>,
    pub elytra: Option<TextureRegion>,
    pub model: Option<PlayerModel>,
    pub secure: bool,
}
#[derive(Debug, Serialize)]
pub struct RemoteChatKey {
    pub session_id: String,
    pub expires_at_millis: String,
    pub public_key_der: Arc<[u8]>,
    pub signature: Arc<[u8]>,
}
#[derive(Debug)]
pub struct PlayerProfile {
    pub uuid: String,
    pub name: String,
    pub properties: Vec<Arc<ProfileProperty>>,
    pub generation: u64,
    pub listed: bool,
    pub latency: i32,
    pub game_mode: i32,
    pub display_name: Option<Value>,
    pub list_order: i32,
    pub show_hat: bool,
    pub chat_key: Option<RemoteChatKey>,
    pub textures: Option<ProfileTextures>,
}
struct Update {
    uuid: String,
    profile: Option<(String, Vec<Arc<ProfileProperty>>)>,
    chat: Option<Option<RemoteChatKey>>,
    game_mode: Option<i32>,
    listed: Option<bool>,
    latency: Option<i32>,
    display: Option<Option<Value>>,
    order: Option<i32>,
    hat: Option<bool>,
}
fn properties_bytes(properties: &[Arc<ProfileProperty>]) -> usize {
    properties
        .iter()
        .map(|p| p.name.len() + p.value.len() + p.signature.as_ref().map_or(0, String::len))
        .sum()
}
fn bytes(r: &mut Reader<'_>, limit: usize) -> Result<Arc<[u8]>> {
    let size = r.count(limit)?;
    Ok(Arc::from(r.take(size)?))
}
fn decode(r: &mut Reader<'_>) -> Result<Vec<Update>> {
    // Eight actions: FriendlyByteBuf.readEnumSet reads a fixed one-byte bitset.
    let actions = r.u8()?;
    let count = r.count(MAX_PROFILES)?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let uuid = metadata::uuid(r)?;
        if !seen.insert(uuid.clone()) {
            return Err("duplicate player-info UUID".into());
        }
        let profile = if actions & 1 != 0 {
            let name = r.string(16)?;
            let count = r.count(16)?;
            let mut properties = Vec::with_capacity(count);
            for _ in 0..count {
                properties.push(Arc::new(ProfileProperty {
                    name: r.string(64)?,
                    value: r.string(32767)?,
                    signature: if r.bool()? {
                        Some(r.string(1024)?)
                    } else {
                        None
                    },
                }));
            }
            Some((name, properties))
        } else {
            None
        };
        let chat = if actions & 2 != 0 {
            Some(if r.bool()? {
                Some(RemoteChatKey {
                    session_id: metadata::uuid(r)?,
                    expires_at_millis: r.i64()?.to_string(),
                    public_key_der: bytes(r, 512)?,
                    signature: bytes(r, 4096)?,
                })
            } else {
                None
            })
        } else {
            None
        };
        let game_mode = if actions & 4 != 0 {
            Some(r.var()?)
        } else {
            None
        };
        let listed = if actions & 8 != 0 {
            Some(r.bool()?)
        } else {
            None
        };
        let latency = if actions & 16 != 0 {
            Some(r.var()?)
        } else {
            None
        };
        let display = if actions & 32 != 0 {
            Some(if r.bool()? { Some(r.nbt()?) } else { None })
        } else {
            None
        };
        let order = if actions & 64 != 0 {
            Some(r.var()?)
        } else {
            None
        };
        let hat = if actions & 128 != 0 {
            Some(r.bool()?)
        } else {
            None
        };
        out.push(Update {
            uuid,
            profile,
            chat,
            game_mode,
            listed,
            latency,
            display,
            order,
            hat,
        });
    }
    r.done()?;
    Ok(out)
}
impl Store {
    pub fn set_local_player(&mut self, network_id: i32, uuid: &str) -> Result<()> {
        uuid::Uuid::parse_str(uuid).map_err(|_| "invalid local player UUID")?;
        self.local_entity_id = Some(network_id);
        self.local_uuid = Some(uuid.to_owned());
        Ok(())
    }
    pub fn player_profile(&self, uuid: &str) -> Option<&PlayerProfile> {
        self.profiles.get(uuid)
    }
    pub fn player_profiles(&self) -> impl Iterator<Item = &PlayerProfile> {
        self.profiles.values()
    }
    pub fn pop_profile_texture_request(&mut self) -> Option<ProfileTextureRequest> {
        self.profile_requests
            .pop_first()
            .map(|(_, request)| request)
    }
    pub fn set_profile_texture(
        &mut self,
        uuid: &str,
        skin: Option<TextureRegion>,
        cape: Option<TextureRegion>,
        elytra: Option<TextureRegion>,
        model: Option<PlayerModel>,
        secure: bool,
        generation: u64,
    ) -> Result<bool> {
        let Some(profile) = self.profiles.get_mut(uuid) else {
            return Ok(false);
        };
        if profile.generation != generation {
            return Ok(false);
        }
        if self.local_uuid.as_deref() != Some(uuid) && !secure {
            return Err("remote_profile_texture_untrusted".into());
        }
        for region in [skin, cape, elytra].into_iter().flatten() {
            if region.width == 0
                || region.height == 0
                || region
                    .uv
                    .iter()
                    .any(|f| !f.is_finite() || !(0. ..=1.).contains(f))
            {
                return Err("invalid published profile texture region".into());
            }
        }
        profile.textures = Some(ProfileTextures {
            skin,
            cape,
            elytra,
            model,
            secure,
        });
        self.invalidate_profile(uuid);
        self.revision += 1;
        Ok(true)
    }
    fn invalidate_profile(&mut self, uuid: &str) {
        if let Some(avatar) = self.local_avatar.as_mut().filter(|e| e.uuid == uuid) {
            avatar.binding = None;
            avatar.appearance_revision = avatar.appearance_revision.wrapping_add(1);
        }
        if let Some(ids) = self.uuid_entities.get(uuid) {
            for id in ids {
                if let Some(slot) = self.ids.get(id) {
                    let e = &mut self.slots[*slot];
                    e.binding = None;
                    e.appearance_revision = e.appearance_revision.wrapping_add(1);
                }
            }
        }
    }
    pub(super) fn apply_player_info(&mut self, id: i32, r: &mut Reader<'_>) -> Result<()> {
        if id == crate::protocol::packet_ids::play::clientbound::PLAYER_INFO_REMOVE {
            let count = r.count(MAX_PROFILES)?;
            let mut ids = BTreeSet::new();
            for _ in 0..count {
                ids.insert(metadata::uuid(r)?);
            }
            r.done()?;
            for uuid in ids {
                if let Some(profile) = self.profiles.remove(&uuid) {
                    self.profile_bytes -= properties_bytes(&profile.properties);
                }
                self.profile_requests.remove(&uuid);
                self.invalidate_profile(&uuid);
            }
        } else {
            let updates = decode(r)?;
            let mut bytes = self.profile_bytes;
            let mut new_count = self.profiles.len();
            for update in &updates {
                if let Some((_, properties)) = &update.profile {
                    if let Some(old) = self.profiles.get(&update.uuid) {
                        bytes -= properties_bytes(&old.properties);
                    } else {
                        new_count += 1;
                    }
                    bytes = bytes
                        .checked_add(properties_bytes(properties))
                        .ok_or("player profile byte overflow")?;
                }
            }
            if bytes > MAX_PROFILE_BYTES || new_count > MAX_PROFILES {
                return Err("player_profile_resource_budget".into());
            }
            self.profile_bytes = bytes;
            for update in updates {
                if let Some((name, properties)) = update.profile {
                    self.generation = self
                        .generation
                        .checked_add(1)
                        .ok_or("profile generation exhausted")?;
                    let generation = self.generation;
                    let texture = properties.iter().find(|p| p.name == "textures").cloned();
                    self.profiles.insert(
                        update.uuid.clone(),
                        PlayerProfile {
                            uuid: update.uuid.clone(),
                            name,
                            properties,
                            generation,
                            listed: false,
                            latency: 0,
                            game_mode: 0,
                            display_name: None,
                            list_order: 0,
                            show_hat: true,
                            chat_key: None,
                            textures: None,
                        },
                    );
                    self.profile_requests.remove(&update.uuid);
                    if let Some(property) = texture {
                        self.profile_requests.insert(
                            update.uuid.clone(),
                            ProfileTextureRequest {
                                uuid: update.uuid.clone(),
                                generation,
                                property,
                                require_secure: self.local_uuid.as_deref() != Some(&update.uuid),
                            },
                        );
                    }
                    self.invalidate_profile(&update.uuid);
                }
                if let Some(profile) = self.profiles.get_mut(&update.uuid) {
                    if let Some(v) = update.chat {
                        profile.chat_key = v;
                    }
                    if let Some(v) = update.game_mode {
                        profile.game_mode = if (0..=3).contains(&v) { v } else { 0 };
                    }
                    if let Some(v) = update.listed {
                        profile.listed = v;
                    }
                    if let Some(v) = update.latency {
                        profile.latency = v;
                    }
                    if let Some(v) = update.display {
                        profile.display_name = v;
                    }
                    if let Some(v) = update.order {
                        profile.list_order = v;
                    }
                    if let Some(v) = update.hat {
                        profile.show_hat = v;
                    }
                }
            }
        }
        self.revision += 1;
        Ok(())
    }
}
