use super::*;

#[derive(Default)]
pub(super) struct PlacementFacts {
    states: Vec<StateFacts>,
    blocks: BTreeMap<u32, Value>,
    items: BTreeMap<u32, Value>,
    actions: Vec<ActionFacts>,
    sounds: Vec<SourceBlockSound>,
    use_sounds: BTreeMap<String, BTreeMap<String, String>>,
    fluid_legacy: BTreeMap<String, u32>,
    potted: BTreeMap<u32, u32>,
    potted_contents: BTreeMap<u32, u32>,
    candle_cakes: BTreeMap<u32, u32>,
    entity_use: Value,
    item_on: BTreeMap<u32, Value>,
    entity_dimensions: BTreeMap<String, [f32; 2]>,
    item_ids: BTreeMap<String, u32>,
}
#[derive(serde::Deserialize)]
struct StateFacts {
    is_solid: bool,
    blocks_motion: bool,
    liquid: bool,
    can_burn: bool,
    instrument: String,
    instrument_above: bool,
    static_support_cache: bool,
    supports: Option<[[bool; 3]; 6]>,
    collision_face_full: Option<[bool; 6]>,
    support_face_full: Option<[bool; 6]>,
    collision_boxes: Option<Vec<[f64; 6]>>,
    support_boxes: Option<Vec<[f64; 6]>>,
    #[serde(default)]
    empty_supports: Option<[[bool; 3]; 6]>,
    #[serde(default)]
    empty_collision_face_full: Option<[bool; 6]>,
    #[serde(default)]
    empty_support_face_full: Option<[bool; 6]>,
    #[serde(default)]
    empty_collision_boxes: Option<Vec<[f64; 6]>>,
    #[serde(default)]
    empty_support_boxes: Option<Vec<[f64; 6]>>,
    #[serde(default)]
    candle_offsets: Vec<[f64; 3]>,
}
#[derive(serde::Deserialize)]
struct ActionFacts {
    fluid_holder: u32,
    can_replace_water: bool,
    can_replace_lava: bool,
    stripped_state: Option<u32>,
    waxed_state: Option<u32>,
    unwaxed_state: Option<u32>,
    copper_previous_state: Option<u32>,
    sound_type: usize,
    spawn_terrain_particles: bool,
    render_invisible: bool,
    has_analog_output: bool,
}
#[derive(serde::Deserialize)]
pub struct SourceBlockSound {
    pub volume: f32,
    pub pitch: f32,
    #[serde(flatten)]
    events: BTreeMap<String, String>,
}
#[derive(serde::Deserialize)]
struct PlacementReport {
    game_version: String,
    states: BTreeMap<u32, StateFacts>,
    blocks: BTreeMap<u32, Value>,
    item_on_static: BTreeMap<u32, Value>,
    entity_dimensions: BTreeMap<String, [f32; 2]>,
}
#[derive(serde::Deserialize)]
struct ItemPlacementReport {
    game_version: String,
    items: BTreeMap<u32, Value>,
}
#[derive(serde::Deserialize)]
struct ActionReport {
    game_version: String,
    states: BTreeMap<u32, ActionFacts>,
    sound_types: Vec<SourceBlockSound>,
    block_use_sounds: BTreeMap<String, BTreeMap<String, String>>,
    fluid_legacy_states: BTreeMap<String, u32>,
    potted_blocks: BTreeMap<u32, u32>,
    candle_cakes: BTreeMap<u32, u32>,
}
fn report<T: serde::de::DeserializeOwned>(
    data: &Path,
    variable: &str,
    name: &str,
    bound: usize,
) -> Result<T> {
    let path = std::env::var(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|_| data.join("reports").join(name));
    serde_json::from_slice(&bounded_bytes(&path, bound as u64)?)
        .map_err(|e| format!("{}: {e}", path.display()))
}
fn rows<T>(mut values: BTreeMap<u32, T>, field: &str, count: usize) -> Result<Vec<T>> {
    if values.len() != count {
        return Err(format!("Source {field} coverage mismatch"));
    }
    (0..count)
        .map(|id| {
            values
                .remove(&(id as u32))
                .ok_or_else(|| format!("missing Source {field} row {id}"))
        })
        .collect()
}
impl PlacementFacts {
    pub(super) fn load(
        data: &Path,
        states: &[BlockInfo],
        item_names: &BTreeMap<u32, String>,
    ) -> Result<Self> {
        let placement: PlacementReport = report(
            data,
            "SOLARIS_CLIENT_PLACEMENT_FACTS",
            "placement_state_facts.json",
            32 << 20,
        )?;
        let items: ItemPlacementReport = report(
            data,
            "SOLARIS_CLIENT_ITEM_PLACEMENT_FACTS",
            "item_placement_facts.json",
            2 << 20,
        )?;
        let actions: ActionReport = report(
            data,
            "SOLARIS_CLIENT_ACTION_STATE_FACTS",
            "action_state_facts.json",
            16 << 20,
        )?;
        let entity_use = report(
            data,
            "SOLARIS_CLIENT_ENTITY_USE_STATIC",
            "entity_use_static.json",
            2 << 20,
        )?;
        if [
            &placement.game_version,
            &items.game_version,
            &actions.game_version,
        ]
        .into_iter()
        .any(|version| version != "26.1.2")
        {
            return Err("Source placement/action facts version mismatch".into());
        }
        // Decode the dense per-state tables directly into their semantic types.
        // Keeping a generic JSON tree and cloning it here multiplied startup heap.
        let state_facts = rows(placement.states, "states", states.len())?;
        let action_facts = rows(actions.states, "states", states.len())?;
        let blocks = placement.blocks;
        let item_facts = items.items;
        let sounds = actions.sound_types;
        for (id, state) in states.iter().enumerate() {
            if !blocks.contains_key(&state.registry_id) {
                return Err(format!("Source placement block coverage {id}"));
            }
            let fact = &state_facts[id];
            if fact.static_support_cache != fact.supports.is_some()
                || fact.static_support_cache != fact.collision_face_full.is_some()
                || fact.static_support_cache != fact.support_face_full.is_some()
                || fact.static_support_cache != fact.collision_boxes.is_some()
                || fact.static_support_cache != fact.support_boxes.is_some()
            {
                return Err(format!("Source placement cache schema {id}"));
            }
            for boxes in [&fact.collision_boxes, &fact.support_boxes]
                .into_iter()
                .flatten()
            {
                if boxes.len() > 256
                    || boxes.iter().any(|b| {
                        b.iter().any(|v| !v.is_finite()) || (0..3).any(|axis| b[axis] > b[axis + 3])
                    })
                {
                    return Err(format!("Source placement shape bounds {id}"));
                }
            }
            let action = &action_facts[id];
            if action.sound_type >= sounds.len()
                || [
                    action.stripped_state,
                    action.waxed_state,
                    action.unwaxed_state,
                    action.copper_previous_state,
                ]
                .into_iter()
                .flatten()
                .any(|state| state as usize >= states.len())
            {
                return Err(format!("Source action reference bound {id}"));
            }
        }
        for (id, fact) in &item_facts {
            if item_names.get(id).map(String::as_str) != fact["name"].as_str() {
                return Err(format!("Source item placement registry mismatch {id}"));
            }
        }
        let potted = actions.potted_blocks;
        let potted_contents = potted.iter().map(|(plant, pot)| (*pot, *plant)).collect();
        Ok(Self {
            states: state_facts,
            blocks,
            items: item_facts,
            actions: action_facts,
            sounds,
            use_sounds: actions.block_use_sounds,
            fluid_legacy: actions.fluid_legacy_states,
            potted,
            potted_contents,
            candle_cakes: actions.candle_cakes,
            item_on: placement.item_on_static,
            entity_dimensions: placement.entity_dimensions,
            item_ids: item_names
                .iter()
                .map(|(id, name)| (name.clone(), *id))
                .collect(),
            entity_use,
        })
    }
}
impl Content {
    pub fn item_id(&self, name: &str) -> Result<u32> {
        self.placement
            .item_ids
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown Source item {name}"))
    }
    pub fn item_on_metadata(&self, id: u32) -> Result<Option<&Value>> {
        Ok(self.placement.item_on.get(&id))
    }
    pub fn item_bucket_fluid(&self, id: u32) -> Result<Option<&str>> {
        Ok(self
            .placement
            .item_on
            .get(&id)
            .and_then(|fact| fact["bucket_fluid"].as_str()))
    }
    pub fn item_bucket_empty_sound(&self, id: u32) -> Result<Option<&str>> {
        Ok(self
            .placement
            .item_on
            .get(&id)
            .and_then(|fact| fact["bucket_empty_sound"].as_str()))
    }
    pub fn source_entity_dimensions(&self, name: &str) -> Result<[f32; 2]> {
        self.placement
            .entity_dimensions
            .get(name)
            .copied()
            .ok_or_else(|| format!("unknown Source entity dimensions {name}"))
    }
    pub fn candle_particle_offsets(&self, state: u32) -> Result<&[[f64; 3]]> {
        Ok(&self.placement_state(state)?.candle_offsets)
    }
    pub(crate) fn terrain_facts(&self) -> &[super::super::models::shapes::TerrainFact] {
        &self.terrain
    }
    pub fn cycle_state_property(&self, state: u32, key: &str) -> Result<u32> {
        let info = self.info(state)?;
        let domain = self
            .property_domains
            .get(&self.names[info.block])
            .and_then(|properties| properties.get(key))
            .ok_or("missing Source property domain")?;
        let value = self
            .state_properties(state)
            .and_then(|properties| properties.get(key))
            .ok_or("missing Source state property")?;
        let index = domain
            .iter()
            .position(|candidate| candidate == value)
            .ok_or("Source property outside domain")?;
        self.with_state_property(state, key, &domain[(index + 1) % domain.len()])
    }
    pub fn state_property_index(&self, state: u32, key: &str) -> Result<usize> {
        let info = self.info(state)?;
        let domain = self
            .property_domains
            .get(&self.names[info.block])
            .and_then(|properties| properties.get(key))
            .ok_or("missing Source property domain")?;
        let value = self
            .state_properties(state)
            .and_then(|properties| properties.get(key))
            .ok_or("missing Source state property")?;
        domain
            .iter()
            .position(|candidate| candidate == value)
            .ok_or_else(|| "Source property outside domain".into())
    }
    pub fn block_is_solid_render(&self, state: u32) -> Result<bool> {
        Ok(self.info(state)?.solid_render)
    }
    pub fn block_should_spawn_terrain_particles(&self, state: u32) -> Result<bool> {
        Ok(self.action_state(state)?.spawn_terrain_particles)
    }
    pub fn block_render_invisible(&self, state: u32) -> Result<bool> {
        Ok(self.action_state(state)?.render_invisible)
    }
    pub fn block_analog_output_owner(&self, state: u32) -> Result<Option<&str>> {
        if !self.action_state(state)?.has_analog_output {
            return Ok(None);
        }
        self.placement_metadata(state)?["analog_output_owner"]
            .as_str()
            .map(Some)
            .ok_or_else(|| "missing Source analog output owner".into())
    }
    pub fn block_brush_sound(&self, state: u32) -> Result<Option<&str>> {
        Ok(self
            .placement_metadata(state)?
            .get("brush_sound")
            .and_then(Value::as_str))
    }
    fn placement_state(&self, state: u32) -> Result<&StateFacts> {
        self.placement
            .states
            .get(state as usize)
            .ok_or_else(|| format!("missing Source placement state {state}"))
    }
    fn action_state(&self, state: u32) -> Result<&ActionFacts> {
        self.placement
            .actions
            .get(state as usize)
            .ok_or_else(|| format!("missing Source action state {state}"))
    }
    pub fn placement_metadata(&self, state: u32) -> Result<&Value> {
        self.placement
            .blocks
            .get(&self.info(state)?.registry_id)
            .ok_or_else(|| format!("missing Source placement metadata {state}"))
    }
    #[cfg(test)]
    pub(crate) fn insert_test_placement_metadata(&mut self, state: u32, metadata: Value) {
        let id = self.info(state).unwrap().registry_id;
        self.placement.blocks.insert(id, metadata);
    }
    #[cfg(test)]
    pub(crate) fn insert_test_entity_dimensions(&mut self, name: &str, dimensions: [f32; 2]) {
        self.placement
            .entity_dimensions
            .insert(name.into(), dimensions);
    }
    pub fn block_fluid_overlay(&self, state: u32) -> Result<bool> {
        self.placement_metadata(state)?["fluid_overlay"]
            .as_bool()
            .ok_or_else(|| "missing Source fluid overlay subtype fact".into())
    }
    pub fn item_placement_metadata(&self, id: u32) -> Result<&Value> {
        self.placement
            .items
            .get(&id)
            .ok_or_else(|| format!("missing Source item placement metadata {id}"))
    }
    pub fn block_is_solid(&self, state: u32) -> Result<bool> {
        Ok(self.placement_state(state)?.is_solid)
    }
    pub fn block_blocks_motion(&self, state: u32) -> Result<bool> {
        Ok(self.placement_state(state)?.blocks_motion)
    }
    pub fn block_liquid(&self, state: u32) -> Result<bool> {
        Ok(self.placement_state(state)?.liquid)
    }
    pub fn block_can_burn(&self, state: u32) -> Result<bool> {
        Ok(self.placement_state(state)?.can_burn)
    }
    pub fn block_note_instrument(&self, state: u32) -> Result<(&str, bool)> {
        let fact = self.placement_state(state)?;
        Ok((&fact.instrument, fact.instrument_above))
    }
    pub fn block_supports(&self, state: u32, face: u8, support: u8) -> Result<bool> {
        if face >= 6 || support >= 3 {
            return Err("Source support face/type bound".into());
        }
        if let Some(faces) = self.placement_state(state)?.supports {
            return Ok(faces[face as usize][support as usize]);
        }
        // Dynamic cache owners require the world-specific support shape, never a cubic substitute.
        Err(format!(
            "Source support requires block context for state {state}"
        ))
    }
    pub fn block_supports_at(
        &self,
        state: u32,
        position: [i32; 3],
        face: u8,
        support: u8,
    ) -> Result<bool> {
        if face >= 6 || support >= 3 {
            return Err("Source support face/type bound".into());
        }
        let fact = self.placement_state(state)?;
        if let Some(faces) = fact.supports {
            return Ok(faces[face as usize][support as usize]);
        }
        let owner = self.placement_metadata(state)?["class"].as_str();
        if matches!(
            owner,
            Some(
                "net.minecraft.world.level.block.ShulkerBoxBlock"
                    | "net.minecraft.world.level.block.piston.MovingPistonBlock"
            )
        ) {
            return Err(format!(
                "Source live block entity support required for state {state}"
            ));
        }
        let offset = self.block_offset(state, position);
        if offset == [0.; 3] {
            return fact
                .empty_supports
                .map(|faces| faces[face as usize][support as usize])
                .ok_or_else(|| "missing Source contextual support capture".into());
        }
        let boxes = fact
            .empty_support_boxes
            .as_deref()
            .ok_or("missing Source offset support shape")?;
        if boxes.len() > 256 {
            return Err("Source support shape bound".into());
        }
        let normal = match face {
            0 | 1 => 1,
            2 | 3 => 2,
            _ => 0,
        };
        let axes = match normal {
            1 => [0, 2],
            2 => [0, 1],
            _ => [2, 1],
        };
        let plane = if face & 1 == 0 { 1e-7 } else { 0.9999999 };
        let mut rectangles = [[0.; 4]; 256];
        let mut count = 0;
        for b in boxes {
            if b[normal] + offset[normal] <= plane && b[normal + 3] + offset[normal] > plane {
                rectangles[count] = [
                    b[axes[0]] + offset[axes[0]],
                    b[axes[1]] + offset[axes[1]],
                    b[axes[0] + 3] + offset[axes[0]],
                    b[axes[1] + 3] + offset[axes[1]],
                ];
                count += 1;
            }
        }
        let center = if normal == 1 {
            [0.4375, 0.4375, 0.5625, 0.5625]
        } else {
            [0.4375, 0., 0.5625, 0.625]
        };
        let ring = [
            [0., 0., 0.125, 1.],
            [0.875, 0., 1., 1.],
            [0.125, 0., 0.875, 0.125],
            [0.125, 0.875, 0.875, 1.],
        ];
        let required: &[[f64; 4]] = match support {
            1 => std::slice::from_ref(&center),
            2 if normal == 1 => &ring,
            _ => &[[0., 0., 1., 1.]],
        };
        Ok(rectangles_cover(&rectangles[..count], required))
    }
    pub fn block_empty_support_boxes(&self, state: u32) -> Result<&[[f64; 6]]> {
        self.placement_state(state)?
            .empty_support_boxes
            .as_deref()
            .ok_or_else(|| "missing Source empty-context support capture".into())
    }
    pub fn block_support_face_full(&self, state: u32, face: u8) -> Result<bool> {
        if face >= 6 {
            return Err("Source support face bound".into());
        }
        self.placement_state(state)?
            .support_face_full
            .map(|faces| faces[face as usize])
            .ok_or_else(|| format!("Source support face requires block context for state {state}"))
    }
    pub fn block_collision_face_full(&self, state: u32, face: u8) -> Result<bool> {
        if face >= 6 {
            return Err("Source collision face bound".into());
        }
        self.placement_state(state)?
            .collision_face_full
            .map(|faces| faces[face as usize])
            .ok_or_else(|| {
                format!("Source collision face requires block context for state {state}")
            })
    }
    pub fn block_collision_face_covers(
        &self,
        state: u32,
        face: u8,
        required: [f64; 4],
    ) -> Result<bool> {
        if face >= 6 {
            return Err("Source collision face bound".into());
        }
        let boxes = self
            .placement_state(state)?
            .collision_boxes
            .as_deref()
            .ok_or_else(|| {
                format!("Source collision face requires block context for state {state}")
            })?;
        if boxes.len() > 256 {
            return Err("Source collision shape bound".into());
        }
        let normal = match face {
            0 | 1 => 1,
            2 | 3 => 2,
            _ => 0,
        };
        let axes = match normal {
            1 => [0, 2],
            2 => [0, 1],
            _ => [2, 1],
        };
        let plane = if face & 1 == 0 { 1e-7 } else { 0.9999999 };
        let mut rectangles = [[0.; 4]; 256];
        let mut count = 0;
        for bounds in boxes {
            if bounds[normal] <= plane && bounds[normal + 3] > plane {
                rectangles[count] = [
                    bounds[axes[0]],
                    bounds[axes[1]],
                    bounds[axes[0] + 3],
                    bounds[axes[1] + 3],
                ];
                count += 1;
            }
        }
        Ok(rectangles_cover(
            &rectangles[..count],
            std::slice::from_ref(&required),
        ))
    }
    pub fn block_support_boxes(&self, state: u32) -> Result<&[[f64; 6]]> {
        self.placement_state(state)?
            .support_boxes
            .as_deref()
            .ok_or_else(|| format!("Source support shape requires block context for state {state}"))
    }
    pub fn block_placement_collision_boxes(&self, state: u32) -> Result<&[[f64; 6]]> {
        if self.placement_metadata(state)?["collision_owner"].as_str()
            == Some("net.minecraft.world.level.block.ScaffoldingBlock")
        {
            return Ok(&[]); // CollisionContext.of(player, true) overrides scaffolding collision.
        }
        let fact = self.placement_state(state)?;
        if let Some(boxes) = fact.collision_boxes.as_deref() {
            return Ok(boxes);
        }
        if matches!(
            self.placement_metadata(state)?["class"].as_str(),
            Some(
                "net.minecraft.world.level.block.BambooStalkBlock"
                    | "net.minecraft.world.level.block.PointedDripstoneBlock"
                    | "net.minecraft.world.level.block.ShulkerBoxBlock"
            )
        ) {
            return fact
                .empty_collision_boxes
                .as_deref()
                .ok_or_else(|| "missing Source fresh-block placement collision capture".into());
        }
        Err(format!(
            "Source placement collision requires player/block context for state {state}"
        ))
    }
    pub fn block_fluid_holder(&self, state: u32) -> Result<u32> {
        Ok(self.action_state(state)?.fluid_holder)
    }
    pub fn block_can_replace_fluid(&self, state: u32, kind: FluidKind) -> Result<bool> {
        let fact = self.action_state(state)?;
        Ok(match kind {
            FluidKind::Water => fact.can_replace_water,
            FluidKind::Lava => fact.can_replace_lava,
        })
    }
    pub fn fluid_legacy_state(&self, name: &str) -> Result<u32> {
        self.placement
            .fluid_legacy
            .get(name)
            .copied()
            .ok_or_else(|| format!("missing Source legacy fluid state {name}"))
    }
    pub fn block_stripped_state(&self, state: u32) -> Result<Option<u32>> {
        Ok(self.action_state(state)?.stripped_state)
    }
    pub fn block_waxed_state(&self, state: u32) -> Result<Option<u32>> {
        Ok(self.action_state(state)?.waxed_state)
    }
    pub fn block_unwaxed_state(&self, state: u32) -> Result<Option<u32>> {
        Ok(self.action_state(state)?.unwaxed_state)
    }
    pub fn block_copper_previous_state(&self, state: u32) -> Result<Option<u32>> {
        Ok(self.action_state(state)?.copper_previous_state)
    }
    pub fn block_sound(&self, state: u32) -> Result<&SourceBlockSound> {
        self.placement
            .sounds
            .get(self.action_state(state)?.sound_type)
            .ok_or_else(|| "missing Source sound type".into())
    }
    #[cfg(test)]
    pub(crate) fn insert_test_block_sound(
        &mut self,
        state: u32,
        volume: f32,
        pitch: f32,
        events: BTreeMap<String, String>,
    ) {
        let states = self.states.len();
        let index = self.placement.sounds.len();
        self.placement
            .actions
            .resize_with(states.max(state as usize + 1), || ActionFacts {
                fluid_holder: 0,
                can_replace_water: false,
                can_replace_lava: false,
                stripped_state: None,
                waxed_state: None,
                unwaxed_state: None,
                copper_previous_state: None,
                sound_type: 0,
                spawn_terrain_particles: false,
                render_invisible: false,
                has_analog_output: false,
            });
        self.placement.actions[state as usize].sound_type = index;
        self.placement.sounds.push(SourceBlockSound {
            volume,
            pitch,
            events,
        });
    }
    pub fn block_use_sound(&self, state: u32, event: &str) -> Result<&str> {
        if let Some(sound) = self.block_sound(state)?.events.get(event) {
            return Ok(sound);
        }
        let name = self.name(state).ok_or("unknown Source use sound block")?;
        self.placement
            .use_sounds
            .get(name)
            .and_then(|sounds| sounds.get(event))
            .map(String::as_str)
            .ok_or_else(|| format!("missing Source block use sound {name}/{event}"))
    }
    pub fn block_potted_state(&self, state: u32) -> Result<Option<u32>> {
        Ok(self
            .placement
            .potted
            .get(&self.block_default_state(state)?)
            .copied())
    }
    pub fn block_potted_content(&self, state: u32) -> Result<Option<u32>> {
        Ok(self
            .placement
            .potted_contents
            .get(&self.block_default_state(state)?)
            .copied())
    }
    pub fn block_as_item(&self, state: u32) -> Result<u32> {
        self.placement_metadata(state)?["as_item"]
            .as_u64()
            .and_then(|id| u32::try_from(id).ok())
            .ok_or_else(|| "missing Source block asItem registry fact".into())
    }
    pub fn block_candle_cake_state(&self, state: u32) -> Result<Option<u32>> {
        Ok(self
            .placement
            .candle_cakes
            .get(&self.block_default_state(state)?)
            .copied())
    }
    pub fn bee_food_effect(&self, id: u32) -> Result<Option<Value>> {
        self.item_entity_effect(id, "bee_effect")
    }
    pub fn suspicious_stew_effect(&self, id: u32) -> Result<Option<Value>> {
        self.item_entity_effect(id, "suspicious_stew_effects")
    }
    fn item_entity_effect(&self, id: u32, field: &str) -> Result<Option<Value>> {
        let name = self
            .item_names
            .get(&id)
            .ok_or("unknown Source entity-use item")?;
        let row = self.placement.entity_use["items"]
            .get(name)
            .ok_or("missing Source entity-use item")?;
        Ok(row.get(field).filter(|value| !value.is_null()).cloned())
    }
    pub fn entity_can_serialize(&self, name: &str) -> Result<bool> {
        self.placement.entity_use["entities"][name]["can_serialize"]
            .as_bool()
            .ok_or_else(|| format!("missing Source entity serialization predicate {name}"))
    }
}
