//! Native protocol-777 remote presentation. Session remains the local physics/health owner.
//! Derived model data is explicitly activated; no Java process is used by the client.
use crate::protocol::packet_ids::play::clientbound as cb;
mod analog;
mod block_components;
mod blocks;
mod display;
mod dragon;
mod equipment;
mod local;
#[cfg(test)]
mod animation_tests;
mod metadata;
mod nameplates;
mod physical;
mod prediction;
mod presentation;
mod profiles;
mod render_blocks;
mod render_living;
mod render_nameplates;
mod render_special;
mod resources;
mod source_commands;
mod source_state;
mod sheep;
mod solaris_models;
mod villagers;
use crate::{
    api::DynamicGeometry,
    inventory::{self, Stack},
    protocol::{content::Content, Reader, Result},
};
use glam::Mat4;
pub(crate) use blocks::BlockEntitySnapshot;
pub use metadata::Metadata;
pub(crate) use nameplates::NameplateView;
pub use physical::EntityHit;
pub(crate) use presentation::emit as presentation_emit;
pub use presentation::{special_item, PresentationStats};
pub use profiles::{
    PlayerModel, PlayerProfile, ProfileProperty, ProfileTextureRequest, ProfileTextures,
    RemoteChatKey,
};
pub(crate) use render_blocks::{
    BlockPresentation, BlockRenderStatus, EntityPainter, MovingBlockPainter,
};
pub use resources::Models;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
    sync::Arc,
};

const MAX_ENTITIES: usize = 100_000;
const MAX_BLOCK_ENTITIES: usize = 65_536;
const EVENT_RETENTION: usize = 512;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Handle {
    pub network_id: i32,
    pub generation: u64,
}
#[derive(Clone, Copy, Debug)]
pub struct Transform {
    pub position: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub head_yaw: f32,
}
impl Transform {
    fn interpolate(self, target: Self, factor: f64) -> Self {
        Self {
            position: std::array::from_fn(|i| {
                self.position[i] + (target.position[i] - self.position[i]) * factor
            }),
            yaw: angle(self.yaw, target.yaw, factor as f32),
            pitch: angle(self.pitch, target.pitch, factor as f32),
            head_yaw: angle(self.head_yaw, target.head_yaw, factor as f32),
        }
    }
}
fn angle(from: f32, to: f32, f: f32) -> f32 {
    from + ((to - from + 180.).rem_euclid(360.) - 180.) * f
}
#[derive(Clone, Copy, Debug)]
pub struct MinecartStep {
    pub transform: Transform,
    pub velocity: [f64; 3],
    pub weight: f32,
}
#[derive(Debug, Clone, Serialize)]
pub struct Effect {
    pub amplifier: u32,
    pub duration: i32,
    pub flags: u8,
    pub applied_tick: u64,
}
#[derive(Debug)]
pub struct Entity {
    pub handle: Handle,
    pub uuid: String,
    pub kind: u32,
    pub type_name: String,
    pub position: [f64; 3],
    pub velocity: [f64; 3],
    pub transform: Transform,
    previous: Transform,
    pub on_ground: bool,
    pub spawn_data: i32,
    pub observed_tick: u64,
    pub revision: u64,
    pub(crate) appearance_revision: u64,
    pub(crate) physical: physical::AnimationState,
    pub(crate) binding: Option<std::result::Result<presentation::Binding, String>>,
    pub(crate) source: Option<source_state::SourceState>,
    pub minecart_steps: Vec<MinecartStep>,
    pub acceleration_power: Option<f64>,
    pub metadata: BTreeMap<u8, Metadata>,
    pub equipment: [Option<Stack>; 8],
    pub(crate) predicted_metadata: BTreeMap<u8, Metadata>,
    pub(crate) predicted_equipment: [Option<Option<Stack>>; 8],
    pub(crate) predicted_leash: Option<Option<Handle>>,
    pub effects: BTreeMap<u32, Effect>,
    pub attributes: BTreeMap<u32, f64>,
    pub passenger_ids: Vec<i32>,
    pub passengers: Vec<Handle>,
    pub vehicle: Option<Handle>,
    pub leash: Option<Handle>,
    pub leash_holder_id: Option<i32>,
    pub animation: Option<(u8, u64)>,
    pub damage_tick: Option<u64>,
    pub death_tick: Option<u64>,
    pub hurt_direction: f32,
    pub submitted_revision: Option<u64>,
}
impl Entity {
    pub fn presented_transform(&self, _tick: u64, partial: f32) -> Transform {
        let elapsed = (f64::from(self.physical.lerp_progress)
            + if self.physical.frozen {
                0.
            } else {
                f64::from(partial.clamp(0., 1.) - self.physical.lerp_phase)
            })
        .max(0.);
        let factor = if self.physical.lerp_duration == 0 {
            1.
        } else {
            (elapsed / f64::from(self.physical.lerp_duration)).clamp(0., 1.)
        };
        if !self.minecart_steps.is_empty() {
            let total = self
                .minecart_steps
                .iter()
                .map(|s| f64::from(s.weight))
                .sum::<f64>();
            let mut remaining = factor * total;
            let mut previous = self.previous;
            for step in &self.minecart_steps {
                let weight = f64::from(step.weight);
                if weight > 0. && remaining <= weight {
                    return previous.interpolate(step.transform, remaining / weight);
                }
                remaining -= weight;
                previous = step.transform;
            }
            return previous;
        }
        self.previous.interpolate(self.transform, factor)
    }
    fn move_to(
        &mut self,
        target: Transform,
        velocity: Option<[f64; 3]>,
        ground: bool,
        tick: u64,
        snap: bool,
        phase: f32,
    ) {
        // Continue from the pose on screen at `phase`, not from the tick start.
        self.previous = if snap {
            target
        } else {
            self.presented_transform(tick, phase)
        };
        self.transform = target;
        self.minecart_steps.clear();
        self.position = target.position;
        if let Some(v) = velocity {
            self.velocity = v;
        }
        self.on_ground = ground;
        self.observed_tick = tick;
        self.physical.lerp_progress = if snap { self.physical.lerp_duration } else { 0 };
        self.physical.lerp_phase = if snap { 0. } else { phase.clamp(0., 0.999) };
    }
}
#[derive(Debug)]
pub struct BlockEntity {
    pub position: [i32; 3],
    pub kind: u32,
    pub type_name: String,
    pub data: Arc<Value>,
    pub(crate) predicted_data: Option<Arc<Value>>,
    pub raw_nbt: Arc<[u8]>,
    pub state: Option<u32>,
    pub revision: u64,
    pub event: Option<(u8, u8, u64)>,
    pub(crate) animation: blocks::BlockAnimation,
}
#[derive(Default)]
pub struct Store {
    slots: Vec<Entity>,
    ids: BTreeMap<i32, usize>,
    spatial: BTreeMap<[i32; 2], BTreeSet<i32>>,
    pub(crate) motion_spans: BTreeMap<i64, usize>,
    leash_holders: BTreeMap<Handle, BTreeSet<i32>>,
    predicted_leash_holders: BTreeMap<Handle, BTreeSet<i32>>,
    pub(crate) passenger_vehicles: BTreeMap<i32, Handle>,
    pub(crate) fishing_hooks: BTreeMap<i32, Handle>,
    pub(crate) uuid_entities: BTreeMap<String, BTreeSet<i32>>,
    pub(crate) profiles: BTreeMap<String, PlayerProfile>,
    pub(crate) profile_requests: BTreeMap<String, ProfileTextureRequest>,
    pub(crate) profile_bytes: usize,
    pub(crate) local_entity_id: Option<i32>,
    pub(crate) local_uuid: Option<String>,
    pub(crate) visible_ids: Vec<i32>,
    pub(crate) physical_active: BTreeSet<i32>,
    pub(crate) redirectable_projectiles: BTreeSet<u32>,
    pub(crate) spatial_padding: f64,
    /// Login entity id of the local player and the metadata rows the server sent for it.
    pub(crate) own_id: Option<i32>,
    pub(crate) own_metadata: BTreeMap<u8, Metadata>,
    pub(crate) presentation_tick: u64,
    /// Partial tick of the last rendered frame; lets a mid-tick packet start its
    /// interpolation from the pose already on screen instead of snapping.
    pub(crate) frame_partial: f32,
    pub(crate) frame_tick: u64,
    /// Local player eye for idle head look-at; `None` disables it.
    pub(crate) look_target: Option<[f32; 3]>,
    pub block_entities: BTreeMap<[i32; 3], BlockEntity>,
    predicted_block_data: BTreeMap<[i32; 3], Arc<Value>>,
    pub(crate) block_active: BTreeSet<[i32; 3]>,
    pub(crate) dragon_parts: BTreeMap<i32, (Handle, usize)>,
    pub revision: u64,
    generation: u64,
    pub(crate) models: Option<Arc<Models>>,
    pub(crate) registries: BTreeMap<String, Vec<(String, Value)>>,
    events: VecDeque<Value>,
    pub dropped_events: u64,
    pub(crate) transforms: Vec<Mat4>,
    pub(crate) items: crate::ui::items::ItemModels,
    equipment_cache: equipment::Cache,
    pub(crate) text: Option<Arc<crate::ui::render::TextRenderer>>,
    pub(crate) nameplate_cache: render_nameplates::Cache,
    pub(crate) block_render_cache: render_blocks::Cache,
    pub(crate) special_render_cache: render_special::Cache,
    local_avatar: Option<Entity>,
    local_avatar_tick: Option<u64>,
    render_local_avatar: bool,
    local_render_slot: Option<usize>,
}
impl Store {
    pub fn new() -> Self {
        Self {
            items: crate::ui::items::ItemModels::new(),
            ..Self::default()
        }
    }
    pub fn set_text_renderer(&mut self, text: Arc<crate::ui::render::TextRenderer>) {
        self.nameplate_cache = render_nameplates::Cache::default();
        self.text = Some(text);
    }
    pub fn activate_models(&mut self, models: Arc<Models>) {
        self.items.set_entity_models(models.clone());
        self.equipment_cache = equipment::Cache::default();
        self.nameplate_cache = render_nameplates::Cache::default();
        self.special_render_cache = render_special::Cache::default();
        self.models = Some(models);
        self.local_avatar = None;
        self.local_avatar_tick = None;
        for entity in &mut self.slots {
            entity.binding = None;
        }
        self.revision += 1;
        self.spatial_padding = self
            .models_ref()
            .unwrap()
            .schemas
            .values()
            .map(|schema| f64::from(schema.width) * 16.)
            .fold(32., f64::max);
    }
    pub fn models(&self) -> Option<Arc<Models>> {
        self.models.clone()
    }
    pub fn models_ref(&self) -> Option<&Models> {
        self.models.as_deref()
    }
    pub fn clear_dimension(&mut self) {
        self.slots.clear();
        self.local_avatar = None;
        self.local_avatar_tick = None;
        self.local_render_slot = None;
        self.ids.clear();
        self.spatial.clear();
        self.physical_active.clear();
        self.motion_spans.clear();
        self.dragon_parts.clear();
        self.block_active.clear();
        self.presentation_tick = 0;
        self.leash_holders.clear();
        self.predicted_leash_holders.clear();
        self.visible_ids.clear();
        self.passenger_vehicles.clear();
        self.fishing_hooks.clear();
        self.uuid_entities.clear();
        self.items.clear();
        self.equipment_cache = equipment::Cache::default();
        self.block_entities.clear();
        self.predicted_block_data.clear();
        self.block_render_cache = render_blocks::Cache::default();
        self.special_render_cache = render_special::Cache::default();
        self.nameplate_cache = render_nameplates::Cache::default();
        self.events.clear();
        self.revision += 1;
        // generation deliberately survives dimension/connection clears.
    }
    /// Integer metadata the server sent for the local player (e.g. DATA_AIR_SUPPLY_ID).
    pub(crate) fn own_metadata_int(&self, name: &str) -> Option<i32> {
        let index = self.models_ref()?.accessor("minecraft:player", name)?;
        self.own_metadata.get(&index)?.int()
    }
    pub fn clear_connection(&mut self) {
        self.clear_dimension();
        self.registries.clear();
        self.redirectable_projectiles.clear();
        self.profiles.clear();
        self.profile_requests.clear();
        self.profile_bytes = 0;
        self.local_entity_id = None;
        self.own_id = None;
        self.own_metadata.clear();
        self.local_uuid = None;
    }
    pub fn configure_registry(&mut self, name: &str, entries: &[(String, Value)]) -> Result<()> {
        if entries.len() > 65536 || self.registries.len() > 256 {
            return Err("entity configuration registry bound".into());
        }
        self.registries.insert(name.to_owned(), entries.to_vec());
        for entity in &mut self.slots {
            entity.binding = None;
        }
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.slots.len()
    }
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
    pub fn get(&self, id: i32) -> Option<&Entity> {
        self.ids.get(&id).map(|i| &self.slots[*i])
    }
    pub(crate) fn client_entities(&self) -> impl Iterator<Item = &Entity> {
        self.slots.iter().filter(|entity| {
            !entity
                .source
                .as_ref()
                .is_some_and(|source| source.discarded || source.special.discarded)
        })
    }
    /// Records retained from server packets, including entities hidden by local prediction.
    pub(crate) fn received_entities(&self) -> impl Iterator<Item = &Entity> {
        self.slots.iter()
    }
    pub(crate) fn received_metadata(&self, handle: Handle, key: &str) -> Result<Option<&Metadata>> {
        let entity = self.resolve(handle)?;
        let models = self.models_ref().ok_or("entity models not activated")?;
        Ok(models
            .accessor(&entity.type_name, key)
            .and_then(|index| entity.metadata.get(&index)))
    }
    pub(crate) fn entity_bounds(&self, handle: Handle, tick: u64) -> Result<crate::player::Bounds> {
        physical::bounds(
            self.models_ref().ok_or("entity models not activated")?,
            self.resolve(handle)?,
            tick,
            0.,
        )
    }
    pub fn resolve(&self, handle: Handle) -> Result<&Entity> {
        if let Some((parent, _)) = self.dragon_parts.get(&handle.network_id) {
            if parent.generation == handle.generation {
                return self.resolve(*parent);
            }
        }
        self.get(handle.network_id)
            .filter(|e| e.handle == handle)
            .ok_or_else(|| "stale_entity".into())
    }
    pub fn metadata(&self, handle: Handle, key: &str) -> Result<Option<&Metadata>> {
        let entity = self.resolve(handle)?;
        let models = self.models.as_ref().ok_or("entity models not activated")?;
        Ok(models
            .accessor(&entity.type_name, key)
            .and_then(|index| entity.metadata_value(index)))
    }
    pub fn block_entity_data(&self, position: [i32; 3]) -> Option<&Value> {
        self.predicted_block_data
            .get(&position)
            .map(Arc::as_ref)
            .or_else(|| {
                self.block_entities
                    .get(&position)
                    .map(BlockEntity::data_value)
            })
    }
    pub fn block_entity_nbt(&self, position: [i32; 3]) -> Option<&[u8]> {
        self.block_entities
            .get(&position)
            // Locally constructed defaults have no received wire NBT.
            .filter(|entity| !entity.raw_nbt.is_empty())
            .map(|entity| entity.raw_nbt.as_ref())
    }
    pub fn block_entity_name(&self, position: [i32; 3]) -> Option<&str> {
        self.block_entities
            .get(&position)
            .map(|entity| entity.type_name.as_str())
    }
    pub fn uuid_bytes(&self, handle: Handle) -> Result<[u8; 16]> {
        Ok(*uuid::Uuid::parse_str(&self.resolve(handle)?.uuid)
            .map_err(|_| "invalid observed entity UUID")?
            .as_bytes())
    }
    pub fn vehicle_of(&self, passenger: i32) -> Option<Handle> {
        self.passenger_vehicles
            .get(&passenger)
            .copied()
            .filter(|handle| self.resolve(*handle).is_ok())
    }
    pub fn is_first_passenger(&self, vehicle: Handle, passenger: i32) -> bool {
        self.resolve(vehicle)
            .is_ok_and(|entity| entity.passenger_ids.first() == Some(&passenger))
    }
    pub fn java_class(&self, handle: Handle) -> Result<&str> {
        let entity = self.resolve(handle)?;
        Ok(&self
            .models
            .as_ref()
            .ok_or("entity models not activated")?
            .schemas
            .get(&entity.type_name)
            .ok_or("entity schema missing")?
            .class)
    }
    pub fn attribute(&self, handle: Handle, name: &str) -> Result<f64> {
        let entity = self.resolve(handle)?;
        let schema = self
            .models
            .as_ref()
            .ok_or("entity models not activated")?
            .schemas
            .get(&entity.type_name)
            .ok_or("entity schema missing")?;
        let definition = schema
            .default_attributes
            .get(name)
            .ok_or_else(|| format!("attribute {name} not supplied for {}", entity.type_name))?;
        Ok(entity
            .attributes
            .get(&definition.id)
            .copied()
            .unwrap_or(definition.value))
    }
    pub fn attribute_limits(&self, name: &str) -> Result<(f64, f64)> {
        let bounds = self
            .models_ref()
            .ok_or("entity models not activated")?
            .attributes
            .get(name)
            .ok_or("unknown source attribute")?;
        bounds
            .min
            .zip(bounds.max)
            .ok_or_else(|| format!("source attribute {name} is not ranged"))
    }
    pub fn passenger_position_at(
        &self,
        handle: Handle,
        passenger: i32,
        position: [f64; 3],
        yaw: f32,
        _tick: u64,
        _partial: f32,
    ) -> Result<[f64; 3]> {
        if position.iter().any(|value| !value.is_finite()) || !yaw.is_finite() {
            return Err("invalid predicted vehicle transform".into());
        }
        let entity = self.resolve(handle)?;
        let index = entity
            .passenger_ids
            .iter()
            .position(|id| *id == passenger)
            .ok_or("entity is not a passenger of this vehicle")?;
        let models = self.models_ref().ok_or("entity models not activated")?;
        let schema = &models.schemas[&entity.type_name];
        if schema
            .method_owners
            .get("getPassengerAttachmentPoint")
            .is_some_and(|owner| owner != "net.minecraft.world.entity.Entity")
        {
            return Err(format!(
                "vehicle attachment override is not implemented: {}",
                schema.class
            ));
        }
        let (dimensions, scale) = models.dimensions(entity)?;
        let points = dimensions
            .attachments
            .get("PASSENGER")
            .filter(|points| !points.is_empty())
            .ok_or("source passenger attachment missing")?;
        let point = points[index.min(points.len() - 1)].map(|value| value * f64::from(scale));
        let (sin, cos) = (-yaw).to_radians().sin_cos();
        Ok([
            position[0] + point[0] * f64::from(cos) + point[2] * f64::from(sin),
            position[1] + point[1],
            position[2] + point[2] * f64::from(cos) - point[0] * f64::from(sin),
        ])
    }
    pub fn bounds(&self, handle: Handle, tick: u64, partial: f32) -> Result<crate::player::Bounds> {
        if let Some((parent, index)) = self.dragon_parts.get(&handle.network_id) {
            let e = self.resolve(*parent)?;
            if parent.generation != handle.generation {
                return Err("stale_entity".into());
            }
            return e
                .physical
                .dragon
                .as_ref()
                .ok_or("dragon flight state missing")?
                .bounds(*index);
        }
        physical::bounds(
            self.models_ref().ok_or("entity models not activated")?,
            self.resolve(handle)?,
            tick,
            partial,
        )
    }
    fn get_mut(&mut self, id: i32) -> Option<&mut Entity> {
        self.ids.get(&id).copied().map(|i| &mut self.slots[i])
    }
    fn event(&mut self, value: Value) {
        if self.events.len() == EVENT_RETENTION {
            self.events.pop_front();
            self.dropped_events += 1;
        }
        self.events.push_back(value);
    }
    pub fn pop_event(&mut self) -> Option<Value> {
        self.events.pop_front()
    }
    fn remove(&mut self, id: i32) {
        self.clear_leash_prediction(id);
        if let Some(slot) = self.ids.remove(&id) {
            let e = self.slots.swap_remove(slot);
            if e.type_name == "minecraft:fishing_bobber" {
                self.fishing_hooks.remove(&e.spawn_data);
            }
            self.special_render_cache.remove(e.handle);
            self.remove_motion_span(e.physical.lerp_span);
            if e.type_name == "minecraft:ender_dragon" {
                for index in 0..8 {
                    if let Some(child) = id.checked_add(index + 1) {
                        self.dragon_parts.remove(&child);
                    }
                }
            }
            if slot < self.slots.len() {
                self.ids.insert(self.slots[slot].handle.network_id, slot);
            }
            if let Some(ids) = self.uuid_entities.get_mut(&e.uuid) {
                ids.remove(&id);
                if ids.is_empty() {
                    self.uuid_entities.remove(&e.uuid);
                }
            }
            self.passenger_vehicles.remove(&id);
            for passenger in &e.passenger_ids {
                if self.passenger_vehicles.get(passenger) == Some(&e.handle) {
                    self.passenger_vehicles.remove(passenger);
                }
            }
            let cell = cell(e.position);
            self.physical_active.remove(&id);
            if let Some(bucket) = self.spatial.get_mut(&cell) {
                bucket.remove(&id);
                if bucket.is_empty() {
                    self.spatial.remove(&cell);
                }
            }
            if let Some(vehicle) = e.vehicle {
                if let Some(other) = self.get_mut(vehicle.network_id) {
                    if other.handle == vehicle {
                        other.passengers.retain(|p| *p != e.handle);
                        other.passenger_ids.retain(|p| *p != id);
                    }
                }
            }
            for passenger in e.passengers {
                if let Some(other) = self.get_mut(passenger.network_id) {
                    if other.handle == passenger {
                        other.vehicle = None;
                    }
                }
            }
            if let Some(holder) = e.leash {
                if let Some(links) = self.leash_holders.get_mut(&holder) {
                    links.remove(&id);
                }
            }
            if let Some(links) = self.leash_holders.remove(&e.handle) {
                for id in links {
                    if let Some(other) = self.get_mut(id) {
                        other.leash = None;
                    }
                }
            }
            if let Some(links) = self.predicted_leash_holders.remove(&e.handle) {
                for id in links {
                    if let Some(other) = self.get_mut(id) {
                        other.predicted_leash = Some(None);
                    }
                }
            }
            self.event(json!({"type":"entity_removed","handle":e.handle,"uuid":e.uuid,"origin":"server_observed"}));
        }
    }
    fn move_entity(
        &mut self,
        id: i32,
        target: Transform,
        velocity: Option<[f64; 3]>,
        ground: bool,
        tick: u64,
        snap: bool,
    ) {
        let old = self.get(id).map(|e| cell(e.position));
        let new = cell(target.position);
        let revision = self.revision;
        let phase = if self.frame_tick == tick { self.frame_partial } else { 0. };
        let old_span = self.get(id).map_or(0, |e| e.physical.lerp_span);
        self.remove_motion_span(old_span);
        if let Some(e) = self.get_mut(id) {
            e.move_to(target, velocity, ground, tick, snap, phase);
            e.revision = revision;
            e.physical.lerp_span = if snap {
                0
            } else {
                (e.previous.position[0] - target.position[0])
                    .abs()
                    .max((e.previous.position[2] - target.position[2]).abs())
                    .ceil() as i64
            };
        }
        if self.ids.contains_key(&id) {
            self.physical_active.insert(id);
            let span = self.get(id).unwrap().physical.lerp_span;
            if span != 0 {
                *self.motion_spans.entry(span).or_default() += 1;
            }
        }
        if let Some(old) = old {
            if old != new {
                if let Some(bucket) = self.spatial.get_mut(&old) {
                    bucket.remove(&id);
                    if bucket.is_empty() {
                        self.spatial.remove(&old);
                    }
                }
                self.spatial.entry(new).or_default().insert(id);
            }
        }
    }
    pub fn observation(&self, after: Option<i32>, limit: usize) -> Value {
        let limit = limit.min(512);
        let mut rows = Vec::with_capacity(limit);
        let mut bytes_used = 2048usize;
        let mut next = None;
        for (&id, &slot) in self.ids.range((
            after
                .map(std::ops::Bound::Excluded)
                .unwrap_or(std::ops::Bound::Unbounded),
            std::ops::Bound::Unbounded,
        )) {
            if rows.len() == limit {
                next = rows
                    .last()
                    .and_then(|r: &Value| r["network_id"].as_i64())
                    .map(|v| v as i32);
                break;
            }
            let e = &self.slots[slot];
            let mut row = json!({"network_id":id,"generation":e.handle.generation.to_string(),"uuid":e.uuid,"type_id":e.kind,"type":e.type_name,
                "position":e.position,"velocity":e.velocity,"yaw":e.transform.yaw,"pitch":e.transform.pitch,"head_yaw":e.transform.head_yaw,
                "on_ground":e.on_ground,"observed_tick":e.observed_tick.to_string(),"revision":e.revision.to_string(),"metadata":e.metadata,
                "equipment":e.equipment,"effects":e.effects,"passengers":e.passengers,"vehicle":e.vehicle,"leash":e.leash,
                "damage_tick":e.damage_tick.map(|v|v.to_string()),"death_tick":e.death_tick.map(|v|v.to_string()),
                "submitted_revision":e.submitted_revision.map(|v|v.to_string()),"origin":"server_observed"});
            let mut length = json_length(&row);
            if length > 128 * 1024 {
                let object = row.as_object_mut().unwrap();
                for field in ["metadata", "equipment", "effects", "passengers"] {
                    object.remove(field);
                }
                object.insert(
                    "details_omitted".into(),
                    json!(["metadata", "equipment", "effects", "passengers"]),
                );
                length = json_length(&row);
            }
            if bytes_used.saturating_add(length).saturating_add(1) > 240 * 1024 {
                next = rows
                    .last()
                    .and_then(|r: &Value| r["network_id"].as_i64())
                    .map(|v| v as i32);
                break;
            }
            bytes_used += length + 1;
            rows.push(row);
        }
        json!({"entities":rows,"total_known":self.len(),"truncated":next.is_some(),"next_cursor":next,"revision":self.revision.to_string(),"dropped_events":self.dropped_events.to_string()})
    }
    pub fn apply(
        &mut self,
        id: i32,
        r: &mut Reader<'_>,
        content: &Content,
        tick: u64,
    ) -> Result<bool> {
        if id == cb::PLAYER_INFO_REMOVE || id == cb::PLAYER_INFO_UPDATE {
            self.apply_player_info(id, r)?;
            return Ok(true);
        }
        match id {
            cb::ADD_ENTITY => {
                let entity_id = r.var()?;
                let uuid = metadata::uuid(r)?;
                let kind = r.count(65535)? as u32;
                let position = [r.f64()?, r.f64()?, r.f64()?];
                let velocity = read_velocity(r)?;
                let pitch = degrees(r.u8()?);
                let yaw = degrees(r.u8()?);
                let head_yaw = degrees(r.u8()?);
                let data = r.var()?;
                r.done()?;
                let name = content
                    .entity_names
                    .get(&kind)
                    .ok_or_else(|| format!("unknown entity type {kind}"))?
                    .clone();
                if !self.ids.contains_key(&entity_id) && self.slots.len() == MAX_ENTITIES {
                    return Err("entity state capacity reached".into());
                }
                self.remove(entity_id);
                self.generation = self
                    .generation
                    .checked_add(1)
                    .ok_or("entity generation exhausted")?;
                self.revision += 1;
                let transform = Transform {
                    position,
                    yaw,
                    pitch,
                    head_yaw,
                };
                let handle = Handle {
                    network_id: entity_id,
                    generation: self.generation,
                };
                self.ids.insert(entity_id, self.slots.len());
                self.spatial
                    .entry(cell(position))
                    .or_default()
                    .insert(entity_id);
                self.slots.push(Entity {
                    handle,
                    uuid: uuid.clone(),
                    kind,
                    type_name: name.clone(),
                    position,
                    velocity,
                    transform,
                    previous: transform,
                    on_ground: false,
                    spawn_data: data,
                    observed_tick: tick,
                    revision: self.revision,
                    appearance_revision: self.revision,
                    binding: None,
                    source: None,
                    minecart_steps: Vec::new(),
                    acceleration_power: None,
                    metadata: BTreeMap::new(),
                    equipment: std::array::from_fn(|_| None),
                    effects: BTreeMap::new(),
                    attributes: BTreeMap::new(),
                    passengers: Vec::new(),
                    passenger_ids: Vec::new(),
                    predicted_metadata: BTreeMap::new(),
                    predicted_equipment: std::array::from_fn(|_| None),
                    predicted_leash: None,
                    vehicle: self.passenger_vehicles.get(&entity_id).copied(),
                    leash: None,
                    leash_holder_id: None,
                    animation: None,
                    damage_tick: None,
                    death_tick: None,
                    hurt_direction: 0.,
                    submitted_revision: None,
                    physical: physical::AnimationState::default(),
                });
                if name == "minecraft:fishing_bobber"
                    && (self.local_entity_id == Some(data)
                        || self
                            .get(data)
                            .is_some_and(|owner| owner.type_name == "minecraft:player"))
                {
                    self.fishing_hooks.insert(data, handle);
                }
                if let Some(models) = self.models.as_ref() {
                    let source =
                        source_state::SourceState::new(self.get(entity_id).unwrap(), models)?;
                    self.get_mut(entity_id).unwrap().source = Some(source);
                    self.physical_active.insert(entity_id);
                }
                if display::is_display(self.get(entity_id).unwrap()) {
                    self.get_mut(entity_id).unwrap().physical.lerp_duration = 0;
                    self.physical_active.insert(entity_id);
                }
                if name == "minecraft:ender_dragon" {
                    self.physical_active.insert(entity_id);
                    for index in 0..8 {
                        let child = entity_id
                            .checked_add(index as i32 + 1)
                            .ok_or("dragon part id overflow")?;
                        self.dragon_parts.insert(child, (handle, index));
                    }
                    if let Some(models) = self.models.clone() {
                        dragon::update(&models, self.get_mut(entity_id).unwrap(), tick);
                    }
                }
                self.uuid_entities
                    .entry(uuid.clone())
                    .or_default()
                    .insert(entity_id);
                if let Some(vehicle) = self.passenger_vehicles.get(&entity_id).copied() {
                    if let Some(parent) = self.get_mut(vehicle.network_id) {
                        if parent.handle == vehicle {
                            parent.passengers.push(handle);
                        }
                    }
                }
                self.event(json!({"type":"entity_spawned","handle":handle,"uuid":uuid,"entity_type":name,"origin":"server_observed"}));
            }
            cb::ANIMATE => {
                let target = r.var()?;
                let action = r.u8()?;
                r.done()?;
                self.revision += 1;
                let revision = self.revision;
                if let Some(e) = self.get_mut(target) {
                    e.animation = Some((action, tick));
                    e.revision = revision;
                }
                self.event(json!({"type":"entity_animation","entity":target,"animation":action,"tick":tick.to_string(),"origin":"server_observed"}));
            }
            cb::ENTITY_EVENT => {
                let target = r.i32()?;
                let event = r.u8()?;
                r.done()?;
                self.revision += 1;
                let revision = self.revision;
                if let Some(e) = self.get_mut(target) {
                    if let Some(source) = e.source.as_mut() {
                        if let Some(living) = source.living.as_mut() {
                            living.on_status(event, source.age);
                        }
                        source.special.on_status(event, false);
                    }
                    if event == 3 {
                        e.death_tick = Some(tick);
                    }
                    e.revision = revision;
                }
                self.event(json!({"type":"entity_status","entity":target,"status":event,"origin":"server_observed"}));
            }
            cb::MOVE_ENTITY_POS | cb::MOVE_ENTITY_POS_ROT | cb::MOVE_ENTITY_ROT => {
                let target = r.var()?;
                let (delta, ground) = if id == cb::MOVE_ENTITY_ROT {
                    (None, r.bool()?)
                } else {
                    let props = r.var()?;
                    let steps = props >> 1;
                    let mut delta = [0.; 3];
                    let count = if steps == 0 { 1 } else { steps };
                    for _ in 0..count {
                        if steps != 0 {
                            r.var()?;
                        }
                        for coordinate in &mut delta {
                            *coordinate += f64::from(r.i16()?) / 4096.;
                        }
                    }
                    (Some(delta), props & 1 != 0)
                };
                let rotation = if id != cb::MOVE_ENTITY_POS {
                    Some((degrees(r.u8()?), degrees(r.u8()?)))
                } else {
                    None
                };
                r.done()?;
                if let Some(e) = self.get(target) {
                    let mut transform = e.transform;
                    if let Some(d) = delta {
                        for i in 0..3 {
                            transform.position[i] += d[i];
                        }
                    }
                    if let Some((yaw, pitch)) = rotation {
                        transform.yaw = yaw;
                        transform.pitch = pitch;
                    }
                    self.revision += 1;
                    self.move_entity(target, transform, None, ground, tick, false);
                }
            }
            cb::ENTITY_POSITION_SYNC => {
                let target = r.var()?;
                let position = match r.var()? {
                    0 => [r.f64()?, r.f64()?, r.f64()?],
                    1 => {
                        let count = r.count(256)?;
                        if count == 0 {
                            return Err("empty entity position path".into());
                        }
                        let mut last = [0.; 3];
                        for _ in 0..count {
                            last = [r.f64()?, r.f64()?, r.f64()?];
                            r.var()?;
                        }
                        last
                    }
                    _ => return Err("unknown entity position path type".into()),
                };
                let yaw = r.f32()?;
                let pitch = r.f32()?;
                let ground = r.bool()?;
                let head_yaw = self
                    .get(target)
                    .map(|e| e.transform.head_yaw)
                    .unwrap_or(yaw);
                self.revision += 1;
                self.move_entity(
                    target,
                    Transform {
                        position,
                        yaw,
                        pitch,
                        head_yaw,
                    },
                    None,
                    ground,
                    tick,
                    false,
                );
            }
            cb::REMOVE_ENTITIES => {
                let n = r.count(MAX_ENTITIES)?;
                let ids = (0..n).map(|_| r.var()).collect::<Result<Vec<_>>>()?;
                r.done()?;
                self.revision += 1;
                for id in ids {
                    self.remove(id);
                }
            }
            cb::ROTATE_HEAD => {
                let target = r.var()?;
                let head = degrees(r.u8()?);
                r.done()?;
                if let Some(e) = self.get(target) {
                    let mut t = e.transform;
                    t.head_yaw = head;
                    let ground = e.on_ground;
                    self.revision += 1;
                    self.move_entity(target, t, None, ground, tick, false);
                }
            }
            cb::SET_ENTITY_DATA => {
                let target = r.var()?;
                let models = self
                    .models
                    .clone()
                    .ok_or("entity metadata resource schema not activated")?;
                let data = metadata::decode(r, content, &models)?;
                r.done()?;
                // The local player is never a received entity row, but the server still
                // addresses its metadata (air supply, absorption, flags) by the login id.
                if self.own_id == Some(target) && self.get(target).is_none() {
                    self.own_metadata.extend(data.clone());
                }
                if let Some(e) = self.get(target) {
                    if let Some(schema) = models.schemas.get(&e.type_name) {
                        for (index, value) in &data {
                            let expected = schema
                                .accessors
                                .values()
                                .find(|v| v[0] == u32::from(*index));
                            if expected.map(|v| v[1]) != Some(value.serializer_id()) {
                                return Err(format!(
                                    "undefined/mismatched entity accessor {} index {index}",
                                    e.type_name
                                ));
                            }
                        }
                    }
                }
                self.revision += 1;
                let revision = self.revision;
                if let Some(e) = self.get_mut(target) {
                    let mut changed = [false; 256];
                    for index in data.keys() {
                        e.predicted_metadata.remove(index);
                        changed[usize::from(*index)] = true;
                    }
                    display::on_metadata(&models, e, &data);
                    e.metadata.extend(data);
                    source_state::on_metadata(&models, e, &changed);
                    e.revision = revision;
                    e.appearance_revision = revision;
                    e.binding = None;
                }
                if self.get(target).is_some_and(|e| {
                    matches!(
                        e.type_name.as_str(),
                        "minecraft:shulker" | "minecraft:polar_bear" | "minecraft:ender_dragon"
                    )
                }) {
                    self.physical_active.insert(target);
                }
                if let Some(e) = self.get(target) {
                    let b = self.bounds(e.handle, tick, 0.)?;
                    let extent = (b.max[0] - b.min[0]).max(b.max[2] - b.min[2]);
                    self.spatial_padding = self.spatial_padding.max(extent);
                }
            }
            cb::SET_ENTITY_LINK => {
                let target = r.i32()?;
                let holder = r.i32()?;
                r.done()?;
                let handle = if holder == 0 {
                    None
                } else {
                    self.get(holder).map(|e| e.handle)
                };
                self.revision += 1;
                let revision = self.revision;
                self.clear_leash_prediction(target);
                if let Some(old) = self.get(target).and_then(|e| e.leash) {
                    if let Some(ids) = self.leash_holders.get_mut(&old) {
                        ids.remove(&target);
                    }
                }
                if let Some(handle) = handle {
                    self.leash_holders.entry(handle).or_default().insert(target);
                }
                if let Some(e) = self.get_mut(target) {
                    e.leash = handle;
                    e.revision = revision;
                    e.leash_holder_id = if holder == 0 { None } else { Some(holder) };
                }
            }
            cb::SET_ENTITY_MOTION => {
                let target = r.var()?;
                let velocity = read_velocity(r)?;
                r.done()?;
                self.revision += 1;
                let revision = self.revision;
                if let Some(e) = self.get_mut(target) {
                    e.velocity = velocity;
                    e.revision = revision;
                }
            }
            cb::SET_EQUIPMENT => {
                let target = r.var()?;
                let mut updates = Vec::with_capacity(8);
                let mut seen = 0u16;
                loop {
                    let b = r.u8()?;
                    let slot = b & 127;
                    if slot > 7 || seen & (1 << slot) != 0 {
                        return Err("invalid/duplicate entity equipment slot".into());
                    }
                    seen |= 1 << slot;
                    let stack = inventory::read_stack(r, content)?;
                    updates.push((
                        usize::from(slot),
                        if stack.empty() { None } else { Some(stack) },
                    ));
                    if b & 128 == 0 {
                        break;
                    }
                }
                r.done()?;
                self.revision += 1;
                let revision = self.revision;
                if let Some(e) = self.get_mut(target) {
                    for (slot, stack) in updates {
                        e.equipment[slot] = stack;
                        e.predicted_equipment[slot] = None;
                    }
                    e.revision = revision;
                    e.appearance_revision = e.appearance_revision.wrapping_add(1);
                    e.binding = None;
                }
            }
            cb::SET_PASSENGERS => {
                let vehicle = r.var()?;
                let n = r.count(MAX_ENTITIES)?;
                let ids = (0..n).map(|_| r.var()).collect::<Result<Vec<_>>>()?;
                r.done()?;
                let handle = self.get(vehicle).map(|e| e.handle);
                let passengers = ids
                    .iter()
                    .filter_map(|id| self.get(*id).map(|e| e.handle))
                    .collect::<Vec<_>>();
                let mut unique = BTreeSet::new();
                for id in &ids {
                    if *id == vehicle || !unique.insert(*id) {
                        return Err("cyclic/duplicate entity passenger".into());
                    }
                    let mut ancestor = handle;
                    while let Some(h) = ancestor {
                        if h.network_id == *id {
                            return Err("cyclic entity passenger ancestry".into());
                        }
                        ancestor = self.get(h.network_id).and_then(|e| e.vehicle);
                    }
                }
                self.revision += 1;
                let revision = self.revision;
                let old = self
                    .get_mut(vehicle)
                    .map(|entity| {
                        entity.passengers.clear();
                        std::mem::take(&mut entity.passenger_ids)
                    })
                    .unwrap_or_default();
                for passenger in old {
                    self.passenger_vehicles.remove(&passenger);
                    if let Some(entity) = self.get_mut(passenger) {
                        entity.vehicle = None;
                    }
                }
                if let Some(e) = self.get_mut(vehicle) {
                    e.passengers = passengers;
                    e.passenger_ids = ids.clone();
                    e.revision = revision;
                }
                for id in ids {
                    if let Some(handle) = handle {
                        if let Some(previous) = self.passenger_vehicles.insert(id, handle) {
                            if previous != handle {
                                if let Some(old) = self.get_mut(previous.network_id) {
                                    old.passengers.retain(|p| p.network_id != id);
                                    old.passenger_ids.retain(|p| *p != id);
                                }
                            }
                        }
                    }
                    if let Some(e) = self.get_mut(id) {
                        e.vehicle = handle;
                        e.revision = revision;
                    }
                }
            }
            cb::REMOVE_MOB_EFFECT => {
                let target = r.var()?;
                let effect = r.count(65535)? as u32;
                r.done()?;
                self.revision += 1;
                if let Some(e) = self.get_mut(target) {
                    e.effects.remove(&effect);
                }
            }
            cb::UPDATE_MOB_EFFECT => {
                let target = r.var()?;
                let effect = r.count(65535)? as u32;
                let amplifier = r.count(255)? as u32;
                let duration = r.var()?;
                let flags = r.u8()?;
                if flags & !15 != 0 {
                    return Err("mob effect flag bits".into());
                }
                r.done()?;
                self.revision += 1;
                if let Some(e) = self.get_mut(target) {
                    if e.effects.len() >= 256 && !e.effects.contains_key(&effect) {
                        return Err("entity effect bound".into());
                    }
                    e.effects.insert(
                        effect,
                        Effect {
                            amplifier,
                            duration,
                            flags,
                            applied_tick: tick,
                        },
                    );
                }
            }
            cb::UPDATE_ATTRIBUTES => {
                let target = r.var()?;
                let count = r.count(128)?;
                let mut attrs = BTreeMap::new();
                for _ in 0..count {
                    let attr = r.count(65535)? as u32;
                    let base = r.f64()?;
                    let n = r.count(128)?;
                    let (mut add, mut multiply, mut total) = (0., 0., 1.);
                    let mut modifiers = BTreeSet::new();
                    for _ in 0..n {
                        let name = r.string(32767)?;
                        let amount = r.f64()?;
                        let op = r.count(2)?;
                        if !modifiers.insert(name) {
                            return Err("duplicate entity attribute modifier".into());
                        }
                        match op {
                            0 => add += amount,
                            1 => multiply += amount,
                            2 => total *= 1. + amount,
                            _ => unreachable!(),
                        }
                    }
                    let mut value = (base + add) * (1. + multiply) * total;
                    if let Some(models) = self.models_ref() {
                        let name = models
                            .attribute_ids
                            .get(&attr)
                            .ok_or("unknown entity attribute registry id")?;
                        let bounds = &models.attributes[name];
                        if let Some((min, max)) = bounds.min.zip(bounds.max) {
                            value = value.clamp(min, max);
                        }
                    }
                    if !value.is_finite() || attrs.insert(attr, value).is_some() {
                        return Err("invalid entity attributes".into());
                    }
                }
                r.done()?;
                self.revision += 1;
                if let Some(e) = self.get_mut(target) {
                    e.attributes.extend(attrs);
                }
            }
            cb::DAMAGE_EVENT => {
                let target = r.var()?;
                let damage_type = r.count(65535)?;
                let cause = r.var()?.checked_sub(1).ok_or("damage cause id underflow")?;
                let direct = r
                    .var()?
                    .checked_sub(1)
                    .ok_or("damage direct id underflow")?;
                let source = if r.bool()? {
                    Some([r.f64()?, r.f64()?, r.f64()?])
                } else {
                    None
                };
                r.done()?;
                self.revision += 1;
                if let Some(e) = self.get_mut(target) {
                    e.damage_tick = Some(tick);
                    if let Some(living) = e.source.as_mut().and_then(|s| s.living.as_mut()) {
                        living.on_damage();
                    }
                }
                self.event(json!({"type":"entity_damage","entity":target,"damage_type":damage_type,"cause":cause,"direct":direct,"source":source,"origin":"server_observed","health_confirmation_required":true}));
            }
            cb::HURT_ANIMATION => {
                let target = r.var()?;
                let yaw = r.f32()?;
                r.done()?;
                self.revision += 1;
                if let Some(e) = self.get_mut(target) {
                    e.damage_tick = Some(tick);
                    e.hurt_direction = yaw;
                    if let Some(living) = e.source.as_mut().and_then(|s| s.living.as_mut()) {
                        living.on_damage();
                    }
                }
            }
            cb::TAKE_ITEM_ENTITY => {
                let item = r.var()?;
                let collector = r.var()?;
                let count = r.count(1_000_000)?;
                r.done()?;
                self.revision += 1;
                self.event(json!({"type":"item_take","item_entity_id":item,"collector_entity_id":collector,"count":count,"origin":"server_observed","inventory_confirmation_required":true}));
            }
            cb::BLOCK_ENTITY_DATA => {
                self.receive_block_entity_data(r, content, tick, None)?;
            }
            cb::BLOCK_EVENT => {
                let p = metadata::position(r.i64()?);
                let action = r.u8()?;
                let parameter = r.u8()?;
                let block = r.count(65535)?;
                r.done()?;
                self.revision += 1;
                let revision = self.revision;
                if let Some(e) = self.block_entities.get_mut(&p) {
                    e.event = Some((action, parameter, tick));
                    e.revision = revision;
                    e.trigger_animation(action, parameter);
                    self.block_active.insert(p);
                }
                self.event(json!({"type":"block_entity_event","position":p,"action":action,"parameter":parameter,"block_type_id":block,"origin":"server_observed"}));
            }
            cb::MOVE_MINECART_ALONG_TRACK => {
                let target = r.var()?;
                let count = r.count(4096)?;
                let mut steps = Vec::with_capacity(count);
                for _ in 0..count {
                    let position = [r.f64()?, r.f64()?, r.f64()?];
                    let velocity = [r.f64()?, r.f64()?, r.f64()?];
                    let yaw = degrees(r.u8()?);
                    let pitch = degrees(r.u8()?);
                    let weight = r.f32()?;
                    if weight < 0. {
                        return Err("negative minecart interpolation weight".into());
                    }
                    steps.push(MinecartStep {
                        transform: Transform {
                            position,
                            yaw,
                            pitch,
                            head_yaw: yaw,
                        },
                        velocity,
                        weight,
                    });
                }
                r.done()?;
                if let Some(last) = steps.last().copied() {
                    self.revision += 1;
                    self.move_entity(
                        target,
                        last.transform,
                        Some(last.velocity),
                        false,
                        tick,
                        false,
                    );
                    if let Some(e) = self.get_mut(target) {
                        e.minecart_steps = steps;
                    }
                }
            }
            cb::TELEPORT_ENTITY => {
                let target = r.var()?;
                let mut position = [r.f64()?, r.f64()?, r.f64()?];
                let mut velocity = [r.f64()?, r.f64()?, r.f64()?];
                let mut yaw = r.f32()?;
                let mut pitch = r.f32()?;
                let flags = r.i32()? as u32;
                if flags & !511 != 0 {
                    return Err("unknown entity relative teleport flag".into());
                }
                let ground = r.bool()?;
                r.done()?;
                if let Some(e) = self.get(target) {
                    for i in 0..3 {
                        if flags & (1 << i) != 0 {
                            position[i] += e.position[i];
                        }
                    }
                    if flags & 8 != 0 {
                        yaw += e.transform.yaw;
                    }
                    if flags & 16 != 0 {
                        pitch += e.transform.pitch;
                    }
                    pitch = pitch.clamp(-90., 90.);
                    let base_velocity = if flags & 256 != 0 {
                        rotate_velocity(
                            e.velocity,
                            e.transform.pitch - pitch,
                            e.transform.yaw - yaw,
                        )
                    } else {
                        e.velocity
                    };
                    for i in 0..3 {
                        if flags & (1 << (i + 5)) != 0 {
                            velocity[i] += base_velocity[i];
                        }
                    }
                    let head_yaw = e.transform.head_yaw;
                    self.revision += 1;
                    self.move_entity(
                        target,
                        Transform {
                            position,
                            yaw,
                            pitch,
                            head_yaw,
                        },
                        Some(velocity),
                        ground,
                        tick,
                        true,
                    );
                }
            }
            cb::PROJECTILE_POWER => {
                let target = r.var()?;
                let power = r.f64()?;
                r.done()?;
                self.revision += 1;
                if let Some(e) = self.get_mut(target) {
                    e.acceleration_power = Some(power);
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn apply_block_entity(
        &mut self,
        p: [i32; 3],
        kind: u32,
        data: Arc<Value>,
        raw_nbt: Arc<[u8]>,
        state: Option<u32>,
        content: &Content,
        tick: u64,
    ) -> Result<()> {
        let name = content
            .block_entity_names
            .get(&kind)
            .ok_or_else(|| format!("unknown block entity type {kind}"))?
            .clone();
        if self.block_entities.len() == MAX_BLOCK_ENTITIES && !self.block_entities.contains_key(&p)
        {
            return Err("block entity state capacity".into());
        }
        let animation = blocks::parse(&name, &data, content)?;
        self.predicted_block_data.remove(&p);
        if animation.piston.is_some() {
            self.block_active.insert(p);
        }
        self.revision += 1;
        if let Some(e) = self.block_entities.get_mut(&p) {
            let same_type = e.type_name == name;
            e.kind = kind;
            e.type_name = name;
            e.update_data(data, raw_nbt, animation, self.revision, !same_type);
            e.state = state.or(e.state);
        } else {
            self.block_entities.insert(
                p,
                BlockEntity {
                    position: p,
                    kind,
                    type_name: name,
                    data,
                    predicted_data: None,
                    raw_nbt,
                    state,
                    revision: self.revision,
                    event: None,
                    animation,
                },
            );
        }
        Ok(())
    }
    /// Native constructor projection, not received or fully serialized NBT.
    /// LevelChunk.setBlockState creates these before an update-data packet.
    pub(crate) fn ensure_block_entity(
        &mut self,
        position: [i32; 3],
        state: u32,
        content: &Content,
        _tick: u64,
    ) -> Result<bool> {
        let metadata = content.placement_metadata(state)?;
        let Some(name) = metadata["block_entity_type"].as_str() else {
            if self.block_entities.contains_key(&position) {
                self.remove_block_entity(position);
            }
            return Ok(false);
        };
        if let Some(entity) = self.block_entities.get(&position)
            && entity.type_name == name
            && let Some(previous) = entity.state
            && content.info(previous)?.block == content.info(state)?.block
        {
            return Ok(true);
        }
        if self.block_entity_name(position).is_some() {
            self.remove_block_entity(position);
        }
        let Some(snapshot) = self.new_block_entity_snapshot(position, state, content)? else {
            return Ok(false);
        };
        self.restore_block_entity_snapshot(snapshot)?;
        Ok(true)
    }
    pub(crate) fn remove_block_entity(&mut self, position: [i32; 3]) {
        self.block_entities.remove(&position);
        self.predicted_block_data.remove(&position);
        self.block_active.remove(&position);
        self.block_render_cache.remove(position);
        self.revision += 1;
    }
    pub fn block_changed(&mut self, p: [i32; 3], state: u32) {
        if let Some(e) = self.block_entities.get_mut(&p) {
            e.state = Some(state);
            self.revision += 1;
            e.revision = self.revision;
        }
    }
    pub fn unload_chunk(&mut self, chunk: [i32; 2]) {
        self.block_entities
            .retain(|p, _| [p[0].div_euclid(16), p[2].div_euclid(16)] != chunk);
        self.predicted_block_data
            .retain(|p, _| [p[0].div_euclid(16), p[2].div_euclid(16)] != chunk);
        self.block_active
            .retain(|p| [p[0].div_euclid(16), p[2].div_euclid(16)] != chunk);
        self.revision += 1;
    }
    pub fn append_geometry(
        &mut self,
        content: &Content,
        origin: [f64; 3],
        camera: [f64; 3],
        camera_rotation: [f32; 2],
        radius: f32,
        tick: u64,
        partial: f32,
        light: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
        bed_direction: &dyn Fn([i32; 3]) -> Option<[i32; 2]>,
        out: &mut DynamicGeometry,
    ) -> Result<PresentationStats> {
        // A temporary render slot reuses the ordinary rig/equipment path without
        // registering the predicted local avatar as a received remote entity.
        if self.render_local_avatar && let Some(avatar) = self.local_avatar.take() {
            self.local_render_slot = Some(self.slots.len());
            self.slots.push(avatar);
        }
        let result = presentation::append(
            self,
            content,
            origin,
            camera,
            camera_rotation,
            radius,
            tick,
            partial,
            light,
            bed_direction,
            out,
        );
        if self.local_render_slot.take().is_some() {
            self.local_avatar = self.slots.pop();
        }
        result
    }
}
fn cell(p: [f64; 3]) -> [i32; 2] {
    [(p[0] / 16.).floor() as i32, (p[2] / 16.).floor() as i32]
}
fn degrees(v: u8) -> f32 {
    f32::from(v) * 360. / 256.
}
pub fn read_velocity(r: &mut Reader<'_>) -> Result<[f64; 3]> {
    let first = r.u8()?;
    if first == 0 {
        return Ok([0.; 3]);
    }
    let second = r.u8()?;
    let hi = r.i32()? as u32;
    let packed = (u64::from(hi) << 16) | (u64::from(second) << 8) | u64::from(first);
    let mut scale = u64::from(first & 3);
    if first & 4 != 0 {
        let extension = r.var()?;
        if extension < 0 {
            return Err("negative LP velocity scale".into());
        }
        scale |= u64::from(extension as u32) << 2;
    }
    Ok(std::array::from_fn(|i| {
        (((((packed >> (3 + 15 * i)) & 32767).min(32766)) as f64 * 2. / 32766.) - 1.) * scale as f64
    }))
}
fn rotate_velocity(v: [f64; 3], pitch: f32, yaw: f32) -> [f64; 3] {
    let (sin_x, cos_x) = pitch.to_radians().sin_cos();
    let x = v[0];
    let y = v[1] * f64::from(cos_x) + v[2] * f64::from(sin_x);
    let z = v[2] * f64::from(cos_x) - v[1] * f64::from(sin_x);
    let (sin_y, cos_y) = yaw.to_radians().sin_cos();
    [
        x * f64::from(cos_y) + z * f64::from(sin_y),
        y,
        z * f64::from(cos_y) - x * f64::from(sin_y),
    ]
}
struct JsonLength(usize);
impl std::io::Write for JsonLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn json_length(value: &Value) -> usize {
    let mut count = JsonLength(0);
    if serde_json::to_writer(&mut count, value).is_err() {
        usize::MAX
    } else {
        count.0
    }
}
