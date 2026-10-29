//! Received block-entity state with source-rate client animation, shared by rendering
//! and collision. No second visual timer and no extrapolated server block authority.
use super::{BlockEntity, MAX_BLOCK_ENTITIES, Store, physical, render_blocks};
use crate::{
    player::Bounds,
    protocol::{Reader, Result, content::Content, world::World},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};

/// An owned received entity kept outside presentation while a block prediction hides it.
pub(crate) struct BlockEntitySnapshot {
    pub(super) entity: BlockEntity,
    active: bool,
}

impl BlockEntitySnapshot {
    pub(crate) fn matches_state(&self, state: u32, content: &Content) -> Result<bool> {
        Ok(match self.entity.state {
            Some(previous) => content.info(previous)?.block == content.info(state)?.block,
            None => false,
        })
    }
}

impl BlockEntity {
    pub(super) fn update_data(
        &mut self,
        data: Arc<Value>,
        raw_nbt: Arc<[u8]>,
        animation: BlockAnimation,
        revision: u64,
        reset_animation: bool,
    ) {
        self.data = data;
        self.predicted_data = None;
        self.raw_nbt = raw_nbt;
        self.revision = revision;
        if reset_animation || animation.piston.is_some() {
            self.animation = animation;
        } else {
            // NBT refresh loads source fields without rewinding client visual clocks.
            self.animation.source.gateway_age = animation.source.gateway_age;
            self.animation.source.spawn_delay = animation.source.spawn_delay;
            self.animation.signals = animation.signals;
        }
    }
}

impl Store {
    pub(crate) fn take_block_entity_snapshot(
        &mut self,
        position: [i32; 3],
    ) -> Option<BlockEntitySnapshot> {
        let entity = self.block_entities.remove(&position)?;
        self.predicted_block_data.remove(&position);
        let active = self.block_active.remove(&position);
        self.block_render_cache.remove(position);
        self.revision += 1;
        Some(BlockEntitySnapshot { entity, active })
    }

    pub(crate) fn restore_block_entity_snapshot(
        &mut self,
        mut snapshot: BlockEntitySnapshot,
    ) -> Result<()> {
        let position = snapshot.entity.position;
        if self.block_entities.len() == MAX_BLOCK_ENTITIES
            && !self.block_entities.contains_key(&position)
        {
            return Err("block entity state capacity".into());
        }
        self.predicted_block_data.remove(&position);
        if let Some(data) = &snapshot.entity.predicted_data {
            self.predicted_block_data.insert(position, data.clone());
        }
        if snapshot.active {
            self.block_active.insert(position);
        } else {
            self.block_active.remove(&position);
        }
        self.block_render_cache.remove(position);
        self.revision += 1;
        snapshot.entity.revision = self.revision;
        self.block_entities.insert(position, snapshot.entity);
        Ok(())
    }

    /// Shared constructor for visible placements and hidden server corrections.
    pub(crate) fn new_block_entity_snapshot(
        &self,
        position: [i32; 3],
        state: u32,
        content: &Content,
    ) -> Result<Option<BlockEntitySnapshot>> {
        let metadata = content.placement_metadata(state)?;
        let Some(name) = metadata["block_entity_type"].as_str() else {
            return Ok(None);
        };
        // MovingPistonBlock.newBlockEntity returns null: only received motion creates it.
        if name == "minecraft:piston" {
            return Ok(None);
        }
        if !render_blocks::has_factory(name) && !render_blocks::source_no_factory(name) {
            return Err(format!("unknown block entity constructor type: {name}"));
        }
        let kind = content.block_entity_names.iter()
            .find_map(|(kind, value)| (value == name).then_some(*kind))
            .ok_or_else(|| format!("block entity type unavailable: {name}"))?;
        // Parsers project absent item lists/profiles/targets to constructor defaults.
        let data = if matches!(name, "minecraft:sign" | "minecraft:hanging_sign") {
            let text = json!({
                "messages": ["", "", "", ""], "color": "black", "has_glowing_text": false
            });
            json!({"front_text":text,"back_text":text,"is_waxed":false})
        } else if name == "minecraft:structure_block" {
            let mode = content.state_properties(state)
                .and_then(|properties| properties.get("mode"))
                .map(|mode| mode.to_ascii_uppercase())
                .unwrap_or_else(|| "DATA".into());
            json!({"mode":mode,"showboundingbox":true})
        } else {
            json!({})
        };
        let animation = parse(name, &data, content)?;
        Ok(Some(BlockEntitySnapshot {
            entity: BlockEntity {
                position, kind, type_name: name.into(), data: Arc::new(data),
                predicted_data: None, raw_nbt: Arc::from([]), state: Some(state),
                revision: self.revision, event: None, animation,
            },
            active: false,
        }))
    }

    pub(crate) fn receive_block_entity_data(
        &mut self,
        r: &mut Reader<'_>,
        content: &Content,
        tick: u64,
        snapshot: Option<&mut BlockEntitySnapshot>,
    ) -> Result<()> {
        let position = super::metadata::position(r.i64()?);
        let kind = r.count(65535)? as u32;
        let start = r.offset;
        let data = Arc::new(r.nbt()?);
        let raw_nbt: Arc<[u8]> = Arc::from(&r.bytes[start..r.offset]);
        r.done()?;
        let name = content.block_entity_names.get(&kind)
            .ok_or_else(|| format!("unknown block entity type {kind}"))?;
        let visible = self.block_entity_name(position) == Some(name.as_str());
        if let Some(snapshot) = snapshot.filter(|snapshot| snapshot.entity.type_name == *name) {
            let animation = parse(name, &data, content)?;
            snapshot.active |= animation.piston.is_some();
            self.revision += 1;
            if visible {
                snapshot.entity.update_data(data.clone(), raw_nbt.clone(), animation, self.revision, false);
            } else {
                snapshot.entity.update_data(data, raw_nbt, animation, self.revision, false);
                return Ok(());
            }
        }
        // An entity hidden by prediction remains a received-data target, not a visible one.
        if visible {
            self.apply_block_entity(position, kind, data, raw_nbt, None, content, tick)?;
        }
        Ok(())
    }
}

#[derive(Default, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShulkerStatus {
    #[default]
    Closed,
    Opening,
    Opened,
    Closing,
}
#[derive(Default, Debug)]
pub(crate) struct BlockAnimation {
    pub progress: f32,
    pub previous: f32,
    pub opening: Option<bool>,
    pub shulker_status: ShulkerStatus,
    pub source: SourceAnimation,
    pub piston: Option<Piston>,
    pub signals: SignalState,
}
#[derive(Default, Debug)]
pub(crate) struct SignalState {
    pub jukebox_elapsed: Option<i64>,
    pub jukebox_duration: Option<i64>,
    pub comparator_output: i32,
    pub test_powered: bool,
}
#[derive(Default, Debug)]
pub(crate) struct BookAnimation {
    pub open: f32,
    pub previous_open: f32,
    pub rotation: f32,
    pub previous_rotation: f32,
    pub target_rotation: f32,
    pub flip: f32,
    pub previous_flip: f32,
    pub target_flip: f32,
    pub flip_speed: f32,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct BeamSection {
    pub color: u32,
    pub height: i32,
}
#[derive(Default, Debug)]
pub(crate) struct SourceAnimation {
    pub ticks: u64,
    pub open_count: i32,
    pub skull_ticks: u64,
    pub skull_active: bool,
    pub book: BookAnimation,
    pub spin: f32,
    pub previous_spin: f32,
    pub spawn_spin: f64,
    pub previous_spawn_spin: f64,
    pub spawn_delay: i32,
    pub conduit_active: bool,
    pub conduit_hunting: bool,
    pub conduit_rotation: f32,
    pub gateway_age: u64,
    pub gateway_cooldown: u32,
    pub bell_ticks: u32,
    pub bell_direction: Option<u8>,
    pub wobble: Option<(u8, u64)>,
    pub beacon_levels: u8,
    pub beacon_levels_pending: bool,
    pub beam_sections: Vec<BeamSection>,
    pub checking_beam: Vec<BeamSection>,
    pub last_check_y: Option<i32>,
    pub pending_world: bool,
    random: u64,
}
impl SourceAnimation {
    pub(crate) fn random_step(&mut self) {
        self.random = self.random.wrapping_mul(0x5deece66d).wrapping_add(11) & ((1u64 << 48) - 1);
    }
    pub(crate) fn random_bits(&self) -> u64 {
        self.random
    }
}
#[derive(Debug)]
pub(crate) struct Piston {
    pub moved_state: u32,
    pub collision_state: u32,
    pub base_collision_state: Option<u32>,
    pub direction: [f64; 3],
    pub extending: bool,
    pub source: bool,
    pub head_long: Option<u32>,
    pub head_short: Option<u32>,
}
fn bool_value(value: &Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| value.as_i64().is_some_and(|v| v != 0))
}
pub(crate) fn parse(name: &str, data: &Value, content: &Content) -> Result<BlockAnimation> {
    if name != "minecraft:piston" {
        return Ok(BlockAnimation {
            source: SourceAnimation {
                gateway_age: data["Age"].as_u64().unwrap_or(0),
                spawn_delay: data["Delay"].as_i64().unwrap_or(20) as i32,
                random: 0x5deece66d,
                ..SourceAnimation::default()
            },
            signals: super::render_blocks::signals::parse(name, data, content)?,
            ..BlockAnimation::default()
        });
    }
    let state = &data["blockState"];
    let block = state["Name"].as_str().unwrap_or("minecraft:air");
    let mut properties = BTreeMap::new();
    if let Some(values) = state["Properties"].as_object() {
        for (key, value) in values {
            properties.insert(
                key.clone(),
                value
                    .as_str()
                    .ok_or("piston blockState property is not a string")?
                    .to_owned(),
            );
        }
    }
    let moved_state = content.resolve_block_state(block, &properties)?;
    let face = data["facing"].as_i64().unwrap_or(0) as i32;
    let direction = physical::direction(face)?;
    let extending = bool_value(&data["extending"]);
    let source = bool_value(&data["source"]);
    let progress = data["progress"].as_f64().unwrap_or(0.) as f32;
    if !progress.is_finite() {
        return Err("invalid piston progress".into());
    }
    let mut head_long = None;
    let mut head_short = None;
    if source {
        let facing = direction_name(face)?;
        let mut properties = BTreeMap::from([
            ("facing".into(), facing.into()),
            ("short".into(), "false".into()),
            (
                "type".into(),
                if block == "minecraft:sticky_piston"
                    || state["Properties"]["type"].as_str() == Some("sticky")
                {
                    "sticky"
                } else {
                    "normal"
                }
                .into(),
            ),
        ]);
        head_long = Some(content.resolve_block_state("minecraft:piston_head", &properties)?);
        properties.insert("short".into(), "true".into());
        head_short = Some(content.resolve_block_state("minecraft:piston_head", &properties)?);
    }
    let base_collision_state = if !extending
        && source
        && matches!(block, "minecraft:piston" | "minecraft:sticky_piston")
    {
        Some(content.with_state_property(moved_state, "extended", "true")?)
    } else {
        None
    };
    let collision_state = if source {
        if extending != (1. - progress < 0.25) {
            head_short.unwrap()
        } else {
            head_long.unwrap()
        }
    } else {
        moved_state
    };
    Ok(BlockAnimation {
        progress,
        previous: progress,
        opening: None,
        shulker_status: ShulkerStatus::Closed,
        piston: Some(Piston {
            moved_state,
            collision_state,
            base_collision_state,
            direction,
            extending,
            source,
            head_long,
            head_short,
        }),
        source: SourceAnimation::default(),
        signals: SignalState::default(),
    })
}
pub(crate) fn direction_name(id: i32) -> Result<&'static str> {
    Ok(match id {
        0 => "down",
        1 => "up",
        2 => "north",
        3 => "south",
        4 => "west",
        5 => "east",
        _ => return Err("invalid source direction".into()),
    })
}
pub(crate) fn facing(block: &BlockEntity, content: &Content) -> Result<[f64; 3]> {
    let name = block
        .state
        .and_then(|s| content.state_properties(s))
        .and_then(|p| p.get("facing"))
        .map(String::as_str)
        .unwrap_or("up");
    physical::direction(match name {
        "down" => 0,
        "up" => 1,
        "north" => 2,
        "south" => 3,
        "west" => 4,
        "east" => 5,
        _ => return Err("invalid shulker box facing".into()),
    })
}
impl BlockEntity {
    pub(crate) fn progress(&self, partial: f32) -> f32 {
        self.animation.previous
            + (self.animation.progress - self.animation.previous) * partial.clamp(0., 1.)
    }
    pub(crate) fn trigger_animation(&mut self, action: u8, parameter: u8) {
        if action != 1 {
            return;
        }
        if self.type_name == "minecraft:end_gateway" {
            self.animation.source.gateway_cooldown = 40;
            return;
        }
        if self.type_name == "minecraft:bell" {
            self.animation.source.bell_ticks = 0;
            self.animation.source.bell_direction = Some(parameter);
            return;
        }
        if self.type_name == "minecraft:decorated_pot" {
            self.animation.source.wobble = Some((parameter, self.animation.source.ticks));
            return;
        }
        if self.type_name == "minecraft:shulker_box" {
            self.animation.source.open_count = i32::from(parameter);
            // Additional viewers (2+) do not restart a closing/opening animation.
            if parameter == 0 {
                self.animation.opening = Some(false);
                self.animation.shulker_status = ShulkerStatus::Closing;
            } else if parameter == 1 {
                self.animation.opening = Some(true);
                self.animation.shulker_status = ShulkerStatus::Opening;
            }
        } else if matches!(
            self.type_name.as_str(),
            "minecraft:chest" | "minecraft:trapped_chest" | "minecraft:ender_chest"
        ) {
            self.animation.source.open_count = i32::from(parameter);
            self.animation.opening = Some(parameter > 0);
        }
    }
    pub(crate) fn piston_offset(&self, partial: f32) -> Option<[f64; 3]> {
        let piston = self.animation.piston.as_ref()?;
        let progress = self.progress(partial);
        let extended = if piston.extending {
            progress - 1.
        } else {
            1. - progress
        };
        Some(piston.direction.map(|v| v * f64::from(extended)))
    }
    pub(crate) fn open_count(&self) -> Option<i32> {
        matches!(
            self.type_name.as_str(),
            "minecraft:chest" | "minecraft:trapped_chest"
        )
        .then_some(self.animation.source.open_count)
    }
    pub(crate) fn jukebox_playing(&self) -> Option<bool> {
        (self.type_name == "minecraft:jukebox")
            .then_some(self.animation.signals.jukebox_elapsed.is_some())
    }
    pub(crate) fn comparator_output(&self) -> Option<i32> {
        (self.type_name == "minecraft:comparator")
            .then_some(self.animation.signals.comparator_output)
    }
    pub(crate) fn test_block_powered(&self) -> Option<bool> {
        (self.type_name == "minecraft:test_block").then_some(self.animation.signals.test_powered)
    }
}
impl Store {
    pub(crate) fn comparator_output(&self, position: [i32; 3]) -> Option<i32> {
        self.block_entities
            .get(&position)
            .and_then(BlockEntity::comparator_output)
    }
    pub(crate) fn jukebox_playing(&self, position: [i32; 3]) -> Option<bool> {
        self.block_entities
            .get(&position)
            .and_then(BlockEntity::jukebox_playing)
    }
    pub(crate) fn block_open_count(&self, position: [i32; 3]) -> Option<i32> {
        self.block_entities
            .get(&position)
            .and_then(BlockEntity::open_count)
    }
    pub(crate) fn test_block_powered(&self, position: [i32; 3]) -> Option<bool> {
        self.block_entities
            .get(&position)
            .and_then(BlockEntity::test_block_powered)
    }
    pub(crate) fn tick_blocks(&mut self, frozen: bool) -> Result<()> {
        if frozen {
            return Ok(());
        }
        let active = std::mem::take(&mut self.block_active);
        for p in &active {
            let Some(block) = self.block_entities.get_mut(p) else {
                continue;
            };
            let animation = &mut block.animation;
            animation.previous = animation.progress;
            if let Some(opening) = animation.opening {
                animation.progress = if opening {
                    (animation.progress + 0.1).min(1.)
                } else {
                    (animation.progress - 0.1).max(0.)
                };
            }
            if block.type_name == "minecraft:shulker_box" {
                match animation.shulker_status {
                    ShulkerStatus::Opening if animation.progress >= 1. => {
                        animation.shulker_status = ShulkerStatus::Opened;
                    }
                    ShulkerStatus::Closing if animation.progress <= 0. => {
                        animation.shulker_status = ShulkerStatus::Closed;
                    }
                    _ => {}
                }
            }
            if let Some(piston) = &mut animation.piston {
                animation.progress = (animation.progress + 0.5).min(1.);
                if piston.source {
                    let short = piston.extending != (1. - animation.progress < 0.25);
                    piston.collision_state = if short {
                        piston.head_short.unwrap()
                    } else {
                        piston.head_long.unwrap()
                    };
                }
            }
        }
        self.block_active = active;
        Ok(())
    }
    pub(crate) fn shulker_is_closed(&self, position: [i32; 3]) -> Option<bool> {
        self.block_entities
            .get(&position)
            .filter(|block| block.type_name == "minecraft:shulker_box")
            .map(|block| block.animation.shulker_status == ShulkerStatus::Closed)
    }
    pub fn append_block_collision(
        &self,
        position: [i32; 3],
        world: &World,
        content: &Content,
        _tick: u64,
        out: &mut Vec<Bounds>,
    ) -> Result<bool> {
        let Some(block) = self.block_entities.get(&position) else {
            return Ok(false);
        };
        if block.type_name == "minecraft:shulker_box" {
            let mut shape = Bounds::new([0., 0., 0., 1., 1., 1.]);
            shape = shape.expanded(
                facing(block, content)?.map(|v| v * f64::from(block.animation.progress * 0.5)),
            );
            out.push(shape.moved(position.map(f64::from)));
            return Ok(true);
        }
        if block.type_name != "minecraft:piston" {
            return Ok(false);
        }
        if content.name(world.block(position).ok_or("unloaded piston collision")?)
            != Some("minecraft:moving_piston")
        {
            return Ok(false);
        }
        let piston = block
            .animation
            .piston
            .as_ref()
            .ok_or("missing authoritative moving piston state")?;
        if let Some(base) = piston.base_collision_state {
            for raw in content
                .collision_boxes(base)
                .ok_or("unknown piston base collision")?
            {
                out.push(Bounds::new(*raw).moved(position.map(f64::from)));
            }
        }
        let offset = block.piston_offset(1.).unwrap();
        let moved = std::array::from_fn(|i| f64::from(position[i]) + offset[i]);
        for raw in content
            .collision_boxes(piston.collision_state)
            .ok_or("unknown moved block collision")?
        {
            out.push(Bounds::new(*raw).moved(moved));
        }
        Ok(true)
    }
}
