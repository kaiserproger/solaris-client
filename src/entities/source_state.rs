//! Source-only client fields share the received entity's generation and random stream.
use super::{
    Entity, Handle, Metadata, Models, Store, presentation as p, render_living, render_special,
};
use crate::{
    player::{Bounds, EntityUseState, Player},
    protocol::{
        Result,
        content::{Content, FluidKind},
        models::JavaRandom,
        world::World,
    },
};
use std::{cell::RefCell, rc::Rc};

#[derive(Debug)]
pub(crate) struct SourceState {
    pub age: i32,
    pub random: Rc<RefCell<JavaRandom>>,
    pub living: Option<render_living::State>,
    pub special: render_special::State,
    pub use_state: EntityUseState,
    pub discarded: bool,
    pub boat_out_of_control: f32,
    pub pending_world: bool,
    pub display: Option<Box<render_special::DisplaySnapshot>>,
    pub display_revision: u64,
    previous_yaw: f32,
}
impl SourceState {
    pub fn new(entity: &Entity, models: &Models) -> Result<Self> {
        let mut seed = [0u8; 8];
        getrandom::fill(&mut seed).map_err(|error| format!("entity random source: {error}"))?;
        let random = Rc::new(RefCell::new(JavaRandom::new(i64::from_ne_bytes(seed))));
        let schema = models
            .schemas
            .get(&entity.type_name)
            .ok_or("entity Source schema missing")?;
        let living = if schema
            .hierarchy
            .iter()
            .any(|name| name == "net.minecraft.world.entity.LivingEntity")
        {
            Some(render_living::State::new(
                &entity.type_name,
                entity.position,
                entity.transform.yaw,
                entity.transform.head_yaw,
                random.clone(),
                models
                    .motion
                    .get(&entity.type_name)
                    .ok_or("entity Source motion missing")?,
            )?)
        } else {
            None
        };
        let special = render_special::State::new(entity, random.clone());
        Ok(Self {
            age: 0,
            random,
            living,
            special,
            use_state: EntityUseState::default(),
            discarded: false,
            boat_out_of_control: 0.,
            pending_world: false,
            display: None,
            display_revision: u64::MAX,
            previous_yaw: entity.transform.yaw,
        })
    }
}
impl Store {
    pub(crate) fn source_use_state(&self, handle: Handle) -> Result<&EntityUseState> {
        let entity = self.resolve(handle)?;
        if entity.handle != handle {
            return Err("dragon child use-state query".into());
        }
        Ok(&entity
            .source
            .as_ref()
            .ok_or("entity Source state not activated")?
            .use_state)
    }
    pub(crate) fn apply_use_state(&mut self, handle: Handle, state: EntityUseState) -> Result<()> {
        self.source_use_state(handle)?;
        self.get_mut(handle.network_id)
            .unwrap()
            .source
            .as_mut()
            .unwrap()
            .use_state = state;
        Ok(())
    }
    pub(crate) fn entity_next_float(&self, handle: Handle) -> Result<f32> {
        let entity = self.resolve(handle)?;
        if entity.handle != handle {
            return Err("dragon child random query".into());
        }
        Ok(entity
            .source
            .as_ref()
            .ok_or("entity Source state not activated")?
            .random
            .borrow_mut()
            .next_float())
    }
    pub(crate) fn entity_next_double(&self, handle: Handle) -> Result<f64> {
        let entity = self.resolve(handle)?;
        if entity.handle != handle {
            return Err("dragon child random query".into());
        }
        Ok(entity
            .source
            .as_ref()
            .ok_or("entity Source state not activated")?
            .random
            .borrow_mut()
            .next_double())
    }
    pub(crate) fn boat_out_of_control_ticks(&self, handle: Handle) -> Result<f32> {
        Ok(self
            .resolve(handle)?
            .source
            .as_ref()
            .ok_or("entity Source state not activated")?
            .boat_out_of_control)
    }
    pub(crate) fn predict_discard(&mut self, handle: Handle) -> Result<()> {
        self.source_use_state(handle)?;
        self.get_mut(handle.network_id)
            .unwrap()
            .source
            .as_mut()
            .unwrap()
            .discarded = true;
        self.revision += 1;
        Ok(())
    }
    pub(crate) fn eating_sound(&self, handle: Handle) -> Result<&str> {
        let entity = self.resolve(handle)?;
        let models = self.models_ref().ok_or("entity models not activated")?;
        let registry = match entity.type_name.as_str() {
            "minecraft:cat" => "minecraft:cat_sound_variant",
            "minecraft:pig" => "minecraft:pig_sound_variant",
            "minecraft:nautilus" => {
                return Ok(if p::boolean(models, entity, "DATA_BABY_ID") {
                    "minecraft:entity.baby_nautilus.eat"
                } else {
                    "minecraft:entity.nautilus.eat"
                });
            }
            "minecraft:zombie_nautilus" => return Ok("minecraft:entity.zombie_nautilus.eat"),
            _ => return Err("entity Source eating sound owner is not variant-driven".into()),
        };
        let entries = self
            .registries
            .get(registry)
            .ok_or("entity sound variant registry unavailable")?;
        let selected = match p::value(models, entity, "DATA_SOUND_VARIANT_ID") {
            Some(Metadata::Registry { id, .. }) => entries.get(*id as usize),
            None => entries
                .iter()
                .find(|(name, _)| name == "minecraft:classic")
                .or_else(|| entries.first()),
            _ => return Err("invalid entity sound holder accessor".into()),
        }
        .ok_or("entity sound holder unavailable")?;
        let data = if selected.1.is_null() {
            models
                .builtin_variants
                .get(registry)
                .and_then(|entries| entries.get(&selected.0))
                .ok_or("builtin entity sound holder unavailable")?
        } else {
            &selected.1
        };
        data[if p::boolean(models, entity, "DATA_BABY_ID") {
            "baby_sounds"
        } else {
            "adult_sounds"
        }]["eat_sound"]
            .as_str()
            .ok_or("entity Source eating sound absent".into())
    }
    pub(crate) fn predict_block_wobble(
        &mut self,
        position: [i32; 3],
        style: u8,
        _tick: u64,
    ) -> Result<()> {
        if style > 1 {
            return Err("decorated-pot wobble style bound".into());
        }
        let block = self
            .block_entities
            .get_mut(&position)
            .ok_or("decorated pot not observed")?;
        if block.type_name != "minecraft:decorated_pot" {
            return Err("wobble target is not decorated pot".into());
        }
        block.animation.source.wobble = Some((style, block.animation.source.ticks));
        self.revision += 1;
        Ok(())
    }
}
pub(crate) fn on_metadata(models: &Models, entity: &mut Entity, changed: &[bool; 256]) {
    let Some(mut source) = entity.source.take() else {
        return;
    };
    if let Some(living) = source.living.as_mut() {
        living.on_metadata(models, entity, changed, source.age);
    }
    source
        .special
        .on_metadata(models, entity, changed, source.age);
    entity.source = Some(source);
}
fn water_contact(
    world: &World,
    content: &Content,
    bounds: Bounds,
    top_only: bool,
) -> Result<Option<bool>> {
    let top = bounds.max[1] + 0.001;
    let min_y = if top_only {
        bounds.max[1].floor() as i32
    } else {
        bounds.min[1].floor() as i32
    };
    let max_y = if top_only {
        top.ceil() as i32
    } else {
        bounds.max[1].ceil() as i32
    };
    let mut contact = false;
    for x in bounds.min[0].floor() as i32..bounds.max[0].ceil() as i32 {
        for y in min_y..max_y {
            for z in bounds.min[2].floor() as i32..bounds.max[2].ceil() as i32 {
                let position = [x, y, z];
                let Some(state) = world.block(position) else {
                    return Ok(None);
                };
                let Some(fluid) = content
                    .info(state)?
                    .fluid
                    .filter(|f| f.kind == FluidKind::Water)
                else {
                    continue;
                };
                if world.block([x, y + 1, z]).is_none() {
                    return Ok(None);
                }
                let height = f64::from(y) + Player::fluid_height(world, content, position, fluid)?;
                contact |= if top_only {
                    top < height
                } else {
                    height >= bounds.min[1]
                };
            }
        }
    }
    Ok(Some(contact))
}
pub(crate) fn tick(
    entity: &mut Entity,
    models: &Models,
    world: &World,
    content: &Content,
    game_time: i64,
    tick: u64,
    first_passenger_is_mob: bool,
    controlling_passenger: bool,
    before_bounds: Bounds,
) -> Result<render_special::TickEffects> {
    let Some(mut source) = entity.source.take() else {
        return Ok(render_special::TickEffects::default());
    };
    source.age = source.age.wrapping_add(1);
    let result = (|| {
        if source.discarded || source.special.discarded {
            return Ok(render_special::TickEffects::default());
        }
        if super::display::is_display(entity)
            && source.display_revision != entity.appearance_revision
        {
            source.display = Some(render_special::DisplaySnapshot::commit(
                source.display.take(),
                models,
                entity,
            ));
            source.display_revision = entity.appearance_revision;
        }
        let bounds = super::physical::bounds(models, entity, 0, 0.)?.deflated(0.001);
        let in_water = if source.living.is_some() {
            water_contact(world, content, bounds, false)?
        } else {
            Some(false)
        };
        let boat = models.schemas[&entity.type_name]
            .hierarchy
            .iter()
            .any(|class| class == "net.minecraft.world.entity.vehicle.boat.AbstractBoat");
        let under_water = if boat {
            water_contact(world, content, before_bounds, true)?
        } else {
            Some(false)
        };
        source.pending_world = in_water.is_none() || under_water.is_none();
        if source.pending_world {
            return Ok(render_special::TickEffects::default());
        }
        if boat {
            source.boat_out_of_control = if under_water == Some(true) {
                source.boat_out_of_control + 1.
            } else {
                0.
            };
        }
        let state = &mut source.use_state;
        for timer in [
            &mut state.age_lock_particle_timer,
            &mut state.forced_age_timer,
            &mut state.in_love,
        ] {
            if *timer > 0 {
                *timer -= 1;
            }
        }
        if state.stand_counter > 0 {
            state.stand_counter -= 1;
            if state.stand_counter == 0 {
                if let Some(index) = models.accessor(&entity.type_name, "DATA_ID_FLAGS") {
                    let flags = p::number(models, entity, "DATA_ID_FLAGS").unwrap_or(0) & !32;
                    entity
                        .predicted_metadata
                        .insert(index, Metadata::Byte(flags as i8));
                }
            }
        }
        // Furnace fuel, tadpole age and allay duplication cooldown only advance on the server.
        if let Some(living) = source.living.as_mut() {
            let facts = models
                .motion
                .get(&entity.type_name)
                .ok_or("entity Source motion missing")?;
            let transform = entity.presented_transform(0, 0.);
            let last_pose = match p::value(models, entity, "LAST_POSE_CHANGE_TICK") {
                Some(Metadata::Long(value)) => value
                    .parse::<i64>()
                    .map_err(|_| "camel pose clock invalid")?,
                _ => 0,
            };
            let sitting = last_pose < 0;
            let pose_time = game_time.wrapping_sub(last_pose.wrapping_abs());
            let flags = p::number(models, entity, "DATA_LIVING_ENTITY_FLAGS").unwrap_or(0);
            let using = flags & 1 != 0;
            let blocking = using
                && entity
                    .equipment_value(if flags & 2 != 0 { 1 } else { 0 })
                    .is_some_and(|stack| stack.component("minecraft:blocks_attacks").is_some());
            living.tick(
                models,
                entity,
                &render_living::TickInput {
                    age: source.age,
                    position: transform.position,
                    yaw: transform.yaw,
                    alive: entity.death_tick.is_none()
                        && match p::value(models, entity, "DATA_HEALTH_ID") {
                            Some(Metadata::Float(health)) => *health > 0.,
                            _ => true,
                        },
                    passenger: entity.vehicle.is_some(),
                    baby: p::boolean(models, entity, "DATA_BABY_ID"),
                    visually_swimming: p::number(models, entity, "DATA_POSE") == Some(3),
                    in_water: in_water == Some(true),
                    uses_y_for_walk: facts.walk_uses_y(entity),
                    rotation_changed_before_base_tick: transform.yaw != source.previous_yaw,
                    is_mob: facts.is_mob,
                    first_passenger_is_mob,
                    max_head_yaw: if facts.is_mob {
                        facts
                            .max_head_yaw(models, entity)
                            .ok_or("Source maximum head rotation missing")?
                    } else {
                        0.
                    },
                    max_body_head_yaw: facts
                        .max_body_head_yaw(blocking)
                        .ok_or("Source maximum body/head rotation missing")?,
                    attack_active: entity.animation.is_some_and(|(action, start)| {
                        matches!(action, 0 | 3) && tick.saturating_sub(start) < 6
                    }),
                    camel_sitting: sitting,
                    camel_visually_sitting: (pose_time < 0) != sitting,
                    camel_sitting_down: sitting && (0..40).contains(&pose_time),
                    camel_pose_transition: pose_time < if sitting { 40 } else { 52 },
                    camel_pose_time: pose_time,
                },
            );
            living.avatar.tick(transform.position, entity.velocity);
            source.previous_yaw = transform.yaw;
        }
        Ok(source
            .special
            .tick(models, entity, source.age, controlling_passenger))
    })();
    entity.source = Some(source);
    result
}
