//! Version-pinned entity shape and query rules. Predicates follow declaring Java owners,
//! not an approximate species list; server tags remain authoritative.
use super::{Entity, Handle, Metadata, Models, Store};
use crate::{
    player::{collision, math, Bounds},
    protocol::{content::Content, world::World, Result},
};

#[derive(Clone, Copy, Debug)]
pub struct EntityHit {
    pub handle: Handle,
    pub point: [f64; 3],
    pub distance: f64,
}
#[derive(Debug)]
pub(crate) struct AnimationState {
    pub lerp_progress: u8,
    pub lerp_duration: u8,
    pub lerp_span: i64,
    pub frozen: bool,
    pub peek: f32,
    pub peek_old: f32,
    pub bear_stand: f32,
    pub bear_stand_old: f32,
    pub dragon: Option<Box<super::dragon::Flight>>,
    pub display: Option<Box<super::display::State>>,
    /// Partial tick at which the current interpolation began, so a packet that
    /// lands mid-tick continues from the pose already on screen.
    pub lerp_phase: f32,
    pub motion: crate::motion::EntityMotion,
}
impl Default for AnimationState {
    fn default() -> Self {
        Self {
            lerp_progress: 3,
            lerp_duration: 3,
            lerp_span: 0,
            frozen: false,
            peek: 0.,
            peek_old: 0.,
            bear_stand: 0.,
            bear_stand_old: 0.,
            dragon: None,
            display: None,
            lerp_phase: 0.,
            motion: crate::motion::EntityMotion::default(),
        }
    }
}
pub(crate) fn value<'a>(models: &Models, e: &'a Entity, name: &str) -> Option<&'a Metadata> {
    models
        .accessor(&e.type_name, name)
        .and_then(|id| e.metadata_value(id))
}
pub(crate) fn number(models: &Models, e: &Entity, name: &str, default: i32) -> i32 {
    value(models, e, name)
        .and_then(Metadata::int)
        .unwrap_or(default)
}
/// Whether the client's idle head glance at the local player may drive this
/// entity. Vanilla clients never turn a NoAI mob's head (Mob flags bit 0): its
/// look comes only from received rotation.
pub(crate) fn may_glance(models: &Models, e: &Entity) -> bool {
    models.motion.get(&e.type_name).is_some_and(|f| f.is_mob)
        && e.source
            .as_ref()
            .is_some_and(|source| source.living.is_some())
        && e.vehicle.is_none()
        && e.death_tick.is_none()
        && number(models, e, "DATA_POSE", 0) == 0
        && number(models, e, "DATA_MOB_FLAGS_ID", 0) & 1 == 0
        && e.type_name != "minecraft:armor_stand"
}
pub(crate) fn boolean(models: &Models, e: &Entity, name: &str, default: bool) -> bool {
    value(models, e, name)
        .and_then(Metadata::boolean)
        .unwrap_or(default)
}
fn float(models: &Models, e: &Entity, name: &str, default: f32) -> f32 {
    match value(models, e, name) {
        Some(Metadata::Float(v)) => *v,
        _ => default,
    }
}
fn hierarchy(models: &Models, e: &Entity, class: &str) -> bool {
    models.schemas[&e.type_name]
        .hierarchy
        .iter()
        .any(|v| v == class)
}
fn owner<'a>(models: &'a Models, e: &Entity, method: &str) -> &'a str {
    models.schemas[&e.type_name]
        .method_owners
        .get(method)
        .map(String::as_str)
        .unwrap_or("")
}
fn alive(models: &Models, e: &Entity) -> bool {
    e.death_tick.is_none() && float(models, e, "DATA_HEALTH_ID", 1.) > 0.
}
fn spectator(store: &Store, e: &Entity) -> bool {
    store
        .profiles
        .get(&e.uuid)
        .is_some_and(|p| p.game_mode == 3)
}
pub(crate) fn approach(current: f32, target: f32, step: f32) -> f32 {
    if current > target {
        (current - step).clamp(target, 1.)
    } else {
        (current + step).clamp(0., target)
    }
}
pub(crate) fn direction(id: i32) -> Result<[f64; 3]> {
    Ok(match id {
        0 => [0., -1., 0.],
        1 => [0., 1., 0.],
        2 => [0., 0., -1.],
        3 => [0., 0., 1.],
        4 => [-1., 0., 0.],
        5 => [1., 0., 0.],
        _ => return Err("invalid source direction".into()),
    })
}
pub(crate) fn physical_peek(amount: f32) -> f32 {
    0.5 - math::sin(f64::from((0.5 + amount) * std::f32::consts::PI)) * 0.5
}
pub(crate) fn bounds(models: &Models, e: &Entity, tick: u64, partial: f32) -> Result<Bounds> {
    let (dimensions, scale) = models.dimensions(e)?;
    let position = e.presented_transform(tick, partial).position;
    let mut width = dimensions.width * scale;
    let mut height = dimensions.height * scale;
    if e.type_name == "minecraft:interaction" {
        width = float(models, e, "DATA_WIDTH_ID", 1.);
        height = float(models, e, "DATA_HEIGHT_ID", 1.);
    }
    if e.type_name == "minecraft:polar_bear" {
        // refreshDimensions precedes this tick's stand update in the source.
        height *= 1. + e.physical.bear_stand_old / 6.;
    }
    if e.type_name == "minecraft:warden" && matches!(number(models, e, "DATA_POSE", 0), 13 | 14) {
        width = dimensions.width;
        height = 1.;
    }
    if width < 0. || height < 0. || !width.is_finite() || !height.is_finite() {
        return Err("invalid entity physical dimensions".into());
    }
    let half = f64::from(width / 2.);
    let mut b = Bounds {
        min: [position[0] - half, position[1], position[2] - half],
        max: [
            position[0] + half,
            position[1] + f64::from(height),
            position[2] + half,
        ],
    };
    if e.type_name == "minecraft:shulker" {
        let face = direction(number(models, e, "DATA_ATTACH_FACE_ID", 0))?;
        b = b.expanded(
            face.map(|v| -v * f64::from(physical_peek(e.physical.peek)) * f64::from(scale)),
        );
    }
    if hierarchy(
        models,
        e,
        "net.minecraft.world.entity.projectile.hurtingprojectile.windcharge.AbstractWindCharge",
    ) {
        b = b.moved([0., -f64::from(0.15f32), 0.]);
    }
    Ok(b)
}
impl Store {
    pub(crate) fn query_padding(&self) -> f64 {
        self.spatial_padding
            + self
                .motion_spans
                .last_key_value()
                .map_or(0., |(distance, _)| *distance as f64)
    }
    pub(crate) fn remove_motion_span(&mut self, span: i64) {
        if span == 0 {
            return;
        }
        if let Some(count) = self.motion_spans.get_mut(&span) {
            *count -= 1;
            if *count == 0 {
                self.motion_spans.remove(&span);
            }
        }
    }
    pub fn configure_interaction_tags(&mut self, content: &Content) -> Result<()> {
        self.redirectable_projectiles.clear();
        for id in
            content.registry_tag("minecraft:entity_type", "minecraft:redirectable_projectile")?
        {
            let id = u32::try_from(*id).map_err(|_| "negative entity interaction tag id")?;
            if !content.entity_names.contains_key(&id) {
                return Err("unknown entity interaction tag id".into());
            }
            self.redirectable_projectiles.insert(id);
        }
        Ok(())
    }
    pub(crate) fn candidates(&self, area: Bounds) -> impl Iterator<Item = &Entity> {
        let padding = self.query_padding();
        let low = super::cell([area.min[0] - padding, 0., area.min[2] - padding]);
        let high = super::cell([area.max[0] + padding, 0., area.max[2] + padding]);
        self.spatial
            .range([low[0], i32::MIN]..=[high[0], i32::MAX])
            .filter(move |(cell, _)| cell[1] >= low[1] && cell[1] <= high[1])
            .flat_map(|(_, ids)| ids.iter())
            .filter_map(|id| self.get(*id))
            .filter(|entity| {
                !entity
                    .source
                    .as_ref()
                    .is_some_and(|s| s.discarded || s.special.discarded)
            })
    }
    fn root_vehicle(&self, mut id: i32) -> Result<i32> {
        for _ in 0..self.ids.len().min(1024) + 1 {
            let Some(parent) = self.vehicle_of(id) else {
                return Ok(id);
            };
            id = parent.network_id;
        }
        Err("entity passenger cycle".into())
    }
    fn pickable(&self, models: &Models, e: &Entity) -> Result<bool> {
        Ok(match owner(models, e, "isPickable") {
            "net.minecraft.world.entity.Entity"
            | "net.minecraft.world.entity.boss.enderdragon.EnderDragon" => false,
            "net.minecraft.world.entity.decoration.ArmorStand" => {
                number(models, e, "DATA_CLIENT_FLAGS", 0) & 16 == 0
            }
            "net.minecraft.world.entity.player.Player" => !spectator(self, e),
            "net.minecraft.world.entity.projectile.Projectile" => {
                self.redirectable_projectiles.contains(&e.kind)
            }
            "net.minecraft.world.entity.projectile.arrow.AbstractArrow" => {
                self.redirectable_projectiles.contains(&e.kind)
                    && !boolean(models, e, "IN_GROUND", false)
            }
            "net.minecraft.world.entity.LivingEntity"
            | "net.minecraft.world.entity.Interaction"
            | "net.minecraft.world.entity.boss.enderdragon.EndCrystal"
            | "net.minecraft.world.entity.decoration.BlockAttachedEntity"
            | "net.minecraft.world.entity.item.FallingBlockEntity"
            | "net.minecraft.world.entity.item.PrimedTnt"
            | "net.minecraft.world.entity.projectile.ShulkerBullet"
            | "net.minecraft.world.entity.vehicle.boat.AbstractBoat"
            | "net.minecraft.world.entity.vehicle.minecart.AbstractMinecart" => true,
            name => return Err(format!("unmapped source pickability owner {name}")),
        })
    }
    pub fn raycast_excluding(
        &self,
        origin: [f64; 3],
        direction: [f64; 3],
        limit: f64,
        exclude_id: i32,
        vehicle: Option<Handle>,
        tick: u64,
        partial: f32,
    ) -> Result<Option<EntityHit>> {
        if !limit.is_finite()
            || limit < 0.
            || origin
                .iter()
                .chain(direction.iter())
                .any(|v| !v.is_finite())
        {
            return Err("invalid entity ray".into());
        }
        let models = self.models_ref().ok_or("entity models not activated")?;
        if let Some(vehicle) = vehicle {
            self.resolve(vehicle)?;
        }
        let end: [f64; 3] = std::array::from_fn(|i| origin[i] + direction[i] * limit);
        let area = Bounds {
            min: std::array::from_fn(|i| origin[i].min(end[i])),
            max: std::array::from_fn(|i| origin[i].max(end[i])),
        };
        let mut hit: Option<EntityHit> = None;
        for e in self.candidates(area) {
            if e.type_name == "minecraft:ender_dragon" {
                if let Some(flight) = &e.physical.dragon {
                    for index in 0..8 {
                        if let Some(distance) =
                            clip(flight.bounds(index)?, origin, direction, limit)
                        {
                            if hit.is_none_or(|old| distance < old.distance) {
                                hit = Some(EntityHit {
                                    handle: Handle {
                                        network_id: e
                                            .handle
                                            .network_id
                                            .checked_add(index as i32 + 1)
                                            .ok_or("dragon part id overflow")?,
                                        generation: e.handle.generation,
                                    },
                                    point: std::array::from_fn(|i| {
                                        origin[i] + direction[i] * distance
                                    }),
                                    distance,
                                });
                            }
                        }
                    }
                }
                continue;
            }
            if e.handle.network_id == exclude_id
                || spectator(self, e)
                || !self.pickable(models, e)?
            {
                continue;
            }
            let mut bounds = bounds(models, e, tick, partial)?;
            if owner(models, e, "getPickRadius")
                == "net.minecraft.world.entity.projectile.Projectile"
            {
                bounds = bounds.deflated(-1.);
            }
            let Some(distance) = clip(bounds, origin, direction, limit) else {
                continue;
            };
            // ProjectileUtil allows targets sharing the rider's root only when the eye is inside.
            if distance != 0.
                && self.root_vehicle(e.handle.network_id)? == self.root_vehicle(exclude_id)?
            {
                continue;
            }
            if hit.is_none_or(|old| distance < old.distance) {
                hit = Some(EntityHit {
                    handle: e.handle,
                    point: std::array::from_fn(|i| origin[i] + direction[i] * distance),
                    distance,
                });
            }
        }
        Ok(hit)
    }
    fn hard_collision(
        &self,
        models: &Models,
        e: &Entity,
        mover: Option<&Entity>,
        mover_position: [f64; 3],
        tick: u64,
    ) -> Result<bool> {
        Ok(match owner(models, e, "canBeCollidedWith") {
            "net.minecraft.world.entity.Entity" => false,
            "net.minecraft.world.entity.vehicle.boat.AbstractBoat" => true,
            "net.minecraft.world.entity.monster.Shulker" => alive(models, e),
            "net.minecraft.world.entity.animal.happyghast.HappyGhast" => {
                !boolean(models, e, "DATA_BABY_ID", false)
                    && alive(models, e)
                    && ((mover.is_none_or(|m| {
                        hierarchy(models, m, "net.minecraft.world.entity.player.Player")
                    }) && mover_position[1] >= bounds(models, e, tick, 0.)?.max[1])
                        || (!e.passenger_ids.is_empty()
                            && mover.is_some_and(|m| {
                                hierarchy(
                                    models,
                                    m,
                                    "net.minecraft.world.entity.animal.happyghast.HappyGhast",
                                )
                            }))
                        || boolean(models, e, "STAYS_STILL", false))
            }
            name => return Err(format!("unmapped source hard-collision owner {name}")),
        })
    }
    fn pushable(
        &self,
        models: &Models,
        e: &Entity,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        let living = || -> Result<bool> {
            Ok(alive(models, e)
                && !spectator(self, e)
                && !collision::climbable(world, content, e.position)?)
        };
        Ok(match owner(models, e, "isPushable") {
            "net.minecraft.world.entity.Entity"
            | "net.minecraft.world.entity.ambient.Bat"
            | "net.minecraft.world.entity.decoration.ArmorStand" => false,
            "net.minecraft.world.entity.animal.parrot.Parrot"
            | "net.minecraft.world.entity.vehicle.boat.AbstractBoat"
            | "net.minecraft.world.entity.vehicle.minecart.AbstractMinecart" => true,
            "net.minecraft.world.entity.animal.equine.AbstractHorse" => e.passenger_ids.is_empty(),
            "net.minecraft.world.entity.LivingEntity" => living()?,
            "net.minecraft.world.entity.monster.creaking.Creaking" => {
                living()? && boolean(models, e, "CAN_MOVE", true)
            }
            "net.minecraft.world.entity.monster.warden.Warden" => {
                !matches!(number(models, e, "DATA_POSE", 0), 13 | 14) && living()?
            }
            name => return Err(format!("unmapped source pushability owner {name}")),
        })
    }
    pub fn collision_bounds(
        &self,
        area: Bounds,
        exclude_id: i32,
        mover: Option<Handle>,
        tick: u64,
        mover_position: [f64; 3],
        world: &World,
        content: &Content,
        out: &mut Vec<Bounds>,
    ) -> Result<()> {
        let models = self.models_ref().ok_or("entity models not activated")?;
        let moving = mover.map(|handle| self.resolve(handle)).transpose()?;
        let root = self.root_vehicle(mover.map_or(exclude_id, |h| h.network_id))?;
        let vehicle = moving.is_some_and(|e| {
            matches!(
                owner(models, e, "canCollideWith"),
                "net.minecraft.world.entity.vehicle.boat.AbstractBoat"
                    | "net.minecraft.world.entity.vehicle.minecart.AbstractMinecart"
            )
        });
        for e in self.candidates(area) {
            if e.handle.network_id == exclude_id
                || Some(e.handle) == mover
                || spectator(self, e)
                || self.root_vehicle(e.handle.network_id)? == root
            {
                continue;
            }
            if moving.is_some_and(|m|hierarchy(models,m,"net.minecraft.world.entity.projectile.hurtingprojectile.windcharge.AbstractWindCharge") && hierarchy(models,e,"net.minecraft.world.entity.projectile.hurtingprojectile.windcharge.AbstractWindCharge")) {continue;}
            if !self.hard_collision(models, e, moving, mover_position, tick)?
                && !(vehicle && self.pushable(models, e, world, content)?)
            {
                continue;
            }
            let b = bounds(models, e, tick, 0.)?;
            if b.intersects(area) {
                out.push(b);
            }
        }
        Ok(())
    }
    pub(crate) fn placement_unobstructed(
        &self,
        area: Bounds,
        exclude_id: i32,
        tick: u64,
    ) -> Result<bool> {
        let models = self.models_ref().ok_or("entity models not activated")?;
        for entity in self.candidates(area) {
            if entity.handle.network_id == exclude_id || !alive(models, entity) {
                continue;
            }
            let blocks_building =
                hierarchy(models, entity, "net.minecraft.world.entity.LivingEntity")
                    || hierarchy(
                        models,
                        entity,
                        "net.minecraft.world.entity.vehicle.boat.AbstractBoat",
                    )
                    || hierarchy(
                        models,
                        entity,
                        "net.minecraft.world.entity.vehicle.minecart.AbstractMinecart",
                    )
                    || matches!(
                        entity.type_name.as_str(),
                        "minecraft:falling_block" | "minecraft:tnt"
                    );
            let marker = entity.type_name == "minecraft:armor_stand"
                && number(models, entity, "DATA_CLIENT_FLAGS", 0) & 16 != 0;
            if blocks_building && !marker && bounds(models, entity, tick, 0.)?.intersects(area) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    pub fn has_living_contact(&self, area: Bounds, exclude_id: i32, tick: u64) -> Result<bool> {
        let models = self.models_ref().ok_or("entity models not activated")?;
        for e in self.candidates(area) {
            if e.handle.network_id != exclude_id
                && hierarchy(models, e, "net.minecraft.world.entity.LivingEntity")
                && bounds(models, e, tick, 0.)?.intersects(area)
            {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(crate) fn armor_stand_item_blocked(
        &self,
        area: Bounds,
        local_bounds: Bounds,
        local_id: i32,
        local_spectator: bool,
        tick: u64,
    ) -> Result<bool> {
        // EntityGetter.getEntities(null, area) uses NO_SPECTATORS, not blocksBuilding.
        if !local_spectator && local_bounds.intersects(area) {
            return Ok(true);
        }
        let expanded = area.deflated(-1e-7);
        let models = self.models_ref().ok_or("entity models not activated")?;
        for entity in self.candidates(expanded) {
            if entity.handle.network_id == local_id || spectator(self, entity) {
                continue;
            }
            let bounds = bounds(models, entity, tick, 0.)?;
            if bounds.intersects(area) {
                return Ok(true);
            }
            if !bounds.intersects(expanded) {
                continue;
            }
            // noCollision(null, area) additionally checks CAN_BE_COLLIDED_WITH in
            // area.inflate(1e-7). Null has no player/vehicle collision privileges.
            let hard = if owner(models, entity, "canBeCollidedWith")
                == "net.minecraft.world.entity.animal.happyghast.HappyGhast"
            {
                !boolean(models, entity, "DATA_BABY_ID", false)
                    && alive(models, entity)
                    && boolean(models, entity, "STAYS_STILL", false)
            } else {
                self.hard_collision(models, entity, None, area.min, tick)?
            };
            if hard {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(crate) fn boat_item_blocked(
        &self,
        area: Bounds,
        local_bounds: Bounds,
        local_id: i32,
        local_spectator: bool,
        tick: u64,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        if !local_spectator && local_bounds.intersects(area) {
            return Ok(true);
        }
        let models = self.models_ref().ok_or("entity models not activated")?;
        for entity in self.candidates(area) {
            if entity.handle.network_id == local_id || spectator(self, entity) {
                continue;
            }
            if !bounds(models, entity, tick, 0.)?.intersects(area) {
                continue;
            }
            // The hypothetical mover is a boat, not the ordinary local-player query.
            let hard = if owner(models, entity, "canBeCollidedWith")
                == "net.minecraft.world.entity.animal.happyghast.HappyGhast"
            {
                !boolean(models, entity, "DATA_BABY_ID", false)
                    && alive(models, entity)
                    && boolean(models, entity, "STAYS_STILL", false)
            } else {
                self.hard_collision(models, entity, None, area.min, tick)?
            };
            if hard || self.pushable(models, entity, world, content)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub(crate) fn boat_item_eye_obstructed(
        &self,
        player_bounds: Bounds,
        look: [f64; 3],
        eye: [f64; 3],
        local_id: i32,
        tick: u64,
    ) -> Result<bool> {
        let area = Bounds {
            min: std::array::from_fn(|i| player_bounds.min[i] + (look[i] * 5.).min(0.) - 1.),
            max: std::array::from_fn(|i| player_bounds.max[i] + (look[i] * 5.).max(0.) + 1.),
        };
        let models = self.models_ref().ok_or("entity models not activated")?;
        for entity in self.candidates(area) {
            if entity.handle.network_id == local_id
                || spectator(self, entity)
                || !self.pickable(models, entity)?
            {
                continue;
            }
            let bounds = bounds(models, entity, tick, 0.)?;
            if !bounds.intersects(area) {
                continue;
            }
            let radius = match owner(models, entity, "getPickRadius") {
                "net.minecraft.world.entity.Entity" => 0.,
                "net.minecraft.world.entity.projectile.Projectile" => 1.,
                name => return Err(format!("unmapped Source pick-radius owner {name}")),
            };
            let bounds = bounds.deflated(-radius);
            if (0..3).all(|i| eye[i] >= bounds.min[i] && eye[i] < bounds.max[i]) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    /// Evidence only: how many entities are currently glancing at the player or
    /// squashing after a landing, and the strongest glance.
    pub fn motion_observation(&self) -> serde_json::Value {
        let (tick, partial) = (self.frame_tick, self.frame_partial);
        let (mut glancing, mut squashing) = (0u32, 0u32);
        let mut best: Option<(f32, &Entity, f32)> = None;
        for e in &self.slots {
            let (yaw, _, weight) = e.physical.motion.look(partial);
            if weight > 0.05 {
                glancing += 1;
                if best.is_none_or(|(w, _, _)| weight > w) {
                    best = Some((weight, e, yaw));
                }
            }
            if e.physical.motion.squash(tick, partial).abs() > 0.01 {
                squashing += 1;
            }
        }
        serde_json::json!({
            "glancing": glancing, "squashing": squashing,
            "strongest_glance": best.map(|(weight, e, yaw)| serde_json::json!({
                "entity_id": e.handle.network_id, "entity_type": e.type_name,
                "weight": weight, "look_yaw": yaw,
            })),
        })
    }
    /// Local player eye for idle head look-at (`None` disables it).
    pub fn set_look_target(&mut self, eye: Option<[f64; 3]>) {
        self.look_target = eye.map(|p| p.map(|v| v as f32));
    }
    pub fn tick_presentation(
        &mut self,
        tick: u64,
        frozen: bool,
        world: &World,
        content: &Content,
        game_time: i64,
    ) -> Result<()> {
        if tick <= self.presentation_tick {
            return Ok(());
        }
        self.presentation_tick = tick;
        let Some(models) = self.models.clone() else {
            return Ok(());
        };
        let mut active = std::mem::take(&mut self.physical_active);
        let look_target = self.look_target;
        for id in &active {
            let Some(e) = self.get(*id) else { continue };
            let paused = frozen
                && !hierarchy(&models, e, "net.minecraft.world.entity.player.Player")
                && !self.root_has_player(self.root_vehicle(*id)?, &models, 0)?;
            let peek = number(&models, e, "DATA_PEEK_ID", 0) as f32 * 0.01;
            let standing = boolean(&models, e, "DATA_STANDING_ID", false);
            let before_bounds = bounds(&models, e, tick, 0.)?;
            let first = e.passenger_ids.first().and_then(|id| self.get(*id));
            let first_passenger_is_mob = first.is_some_and(|p| models.motion[&p.type_name].is_mob);
            let controlling_passenger = e.passenger_ids.first().is_some_and(|passenger| {
                self.local_entity_id == Some(*passenger)
                    || first.is_some_and(|p| {
                        hierarchy(&models, p, "net.minecraft.world.entity.player.Player")
                    })
            });
            self.get_mut(*id).unwrap().physical.frozen = paused;
            if paused {
                continue;
            }
            let e = self.get_mut(*id).unwrap();
            e.physical.lerp_progress = e
                .physical
                .lerp_progress
                .saturating_add(1)
                .min(e.physical.lerp_duration.saturating_add(1));
            e.physical.peek_old = e.physical.peek;
            e.physical.peek = approach(e.physical.peek, peek, 0.05);
            e.physical.bear_stand_old = e.physical.bear_stand;
            e.physical.bear_stand =
                (e.physical.bear_stand + if standing { 1. } else { -1. }).clamp(0., 6.);
            if e.type_name == "minecraft:ender_dragon" {
                super::dragon::update(&models, e, tick);
            }
            if super::display::is_display(e) {
                super::display::tick(&models, e);
            }
            let effects = super::source_state::tick(
                e,
                &models,
                world,
                content,
                game_time,
                tick,
                first_passenger_is_mob,
                controlling_passenger,
                before_bounds,
            )?;
            let (walk_speed, body_yaw) = e
                .source
                .as_ref()
                .and_then(|source| source.living.as_ref())
                .map_or((0., e.transform.yaw), |living| {
                    (living.walk.speed(1.), living.body(1.))
                });
            let can_look = may_glance(&models, e);
            let at = e.presented_transform(tick, 0.);
            e.physical.motion.tick(&crate::motion::EntityTick {
                tick,
                on_ground: e.on_ground,
                y: at.position[1] as f32,
                walk_speed,
                body_yaw,
                position: at.position.map(|v| v as f32),
                can_look,
                target: look_target,
            });
            let span = if e.physical.lerp_progress == e.physical.lerp_duration {
                std::mem::take(&mut e.physical.lerp_span)
            } else {
                0
            };
            self.remove_motion_span(span);
            if effects.sky_flash || effects.lightning_sound_pitches.is_some() {
                self.event(serde_json::json!({
                    "type": "source_lightning", "entity": id,
                    "sky_flash": effects.sky_flash, "sound_pitches": effects.lightning_sound_pitches,
                    "origin": "source_client_tick",
                }));
            }
        }
        active.retain(|id| {
            self.get(*id).is_some_and(|e| {
                e.physical.lerp_progress < e.physical.lerp_duration
                    || e.source.is_some()
                    || super::display::is_display(e)
                    || matches!(
                        e.type_name.as_str(),
                        "minecraft:shulker" | "minecraft:polar_bear" | "minecraft:ender_dragon"
                    )
            })
        });
        self.physical_active = active;
        self.tick_blocks(frozen)?;
        Ok(())
    }
    fn root_has_player(&self, id: i32, models: &Models, depth: usize) -> Result<bool> {
        if depth > 1024 {
            return Err("entity passenger cycle".into());
        }
        let Some(e) = self.get(id) else {
            return Ok(self.local_entity_id == Some(id));
        };
        if hierarchy(models, e, "net.minecraft.world.entity.player.Player") {
            return Ok(true);
        }
        for child in &e.passenger_ids {
            if self.root_has_player(*child, models, depth + 1)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
fn clip(b: Bounds, origin: [f64; 3], direction: [f64; 3], limit: f64) -> Option<f64> {
    let mut enter: f64 = 0.;
    let mut exit = limit;
    for i in 0..3 {
        if direction[i].abs() < 1e-12 {
            if origin[i] < b.min[i] || origin[i] > b.max[i] {
                return None;
            }
        } else {
            let a = (b.min[i] - origin[i]) / direction[i];
            let z = (b.max[i] - origin[i]) / direction[i];
            enter = enter.max(a.min(z));
            exit = exit.min(a.max(z));
            if enter > exit {
                return None;
            }
        }
    }
    (exit >= 0. && enter <= limit).then_some(enter)
}
