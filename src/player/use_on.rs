//! Ordinary 26.1.2 client-side interaction results, not server acknowledgements.
//! Declaring-class evidence: ignored `.analysis/player-research/use-on/`.
use super::*;
use crate::inventory::Stack;

mod air;
mod air_special;
mod block_actions;
mod dispatch;
mod entity_use;
mod lifetime;
mod placement;
mod signals;
mod sounds;
mod survival;

pub(crate) use entity_use::EntityUseState;
#[derive(Clone, Debug)]
pub enum UiTrigger {
    ItemUsed { hand: u8 },
    Book { hand: u8 },
    Sign { position: [i32; 3], front: bool },
    Command { position: [i32; 3] },
    MinecartCommand { handle: Handle },
    Structure { position: [i32; 3] },
    Jigsaw { position: [i32; 3] },
    TestBlock { position: [i32; 3] },
    TestInstanceBlock { position: [i32; 3] },
}

#[derive(Clone, Debug)]
pub enum UseParticle {
    Item { stack: Stack },
    BlockDust { state: u32 },
    LargeSmoke,
    Dust { color: i32, scale: f32 },
    SmallSmoke,
    SmallFlame,
    CampfireCosySmoke,
    Smoke,
    CampfireSignalSmoke,
    Effect { color: i32, alpha: f32 },
    HappyVillager,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UseParticleSample {
    pub position: [f64; 3],
    pub velocity: [f64; 3],
}

#[derive(Clone, Debug)]
pub enum UsePrediction {
    Sound(crate::audio::SoundRequest),
    Particles {
        kind: UseParticle,
        samples: [UseParticleSample; 16],
        count: u8,
    },
    WorldBlock {
        position: [i32; 3],
        state: u32,
        flags: i32,
        sequence: i32,
        player_position: [f64; 3],
    },
    ExtraItem {
        stack: Stack,
        drop_remainder: bool,
    },
    PlayerSlot {
        slot: u16,
        stack: Option<Stack>,
        replacement: bool,
    },
    EntityEquipment {
        handle: Handle,
        slot: u8,
        stack: Option<Stack>,
    },
    EntityUseState {
        handle: Handle,
        state: EntityUseState,
    },
    EntityLeash {
        handle: Handle,
        holder: Option<Handle>,
    },
    EntityDiscard {
        handle: Handle,
    },
    LevelEvent {
        event: i32,
        position: [i32; 3],
        data: i32,
    },
    BlockEntityComponents {
        position: [i32; 3],
        components: Arc<BTreeMap<String, Value>>,
        patch: Arc<crate::inventory::ComponentPatch>,
    },
    ComparatorOutput {
        position: [i32; 3],
        signal: i32,
    },
    EntitySwing {
        handle: Handle,
        hand: u8,
    },
    EntityEffect {
        handle: Handle,
        effect: i32,
        duration: i32,
        amplifier: u8,
        flags: u8,
        tick: u64,
    },
    EntityMetadata {
        handle: Handle,
        key: &'static str,
        value: crate::entities::Metadata,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Pass,
    Fail,
    TryEmptyHand,
    Success { swing: bool, item_interaction: bool },
}
impl Outcome {
    pub(super) const CLIENT: Self = Self::Success {
        swing: true,
        item_interaction: true,
    };
    pub(super) const CONSUME: Self = Self::Success {
        swing: false,
        item_interaction: true,
    };
    pub(super) const CLIENT_WITHOUT_ITEM: Self = Self::Success {
        swing: true,
        item_interaction: false,
    };
    pub(super) fn consumes(self) -> bool {
        matches!(self, Self::Success { .. })
    }
    pub(super) fn java_result(self) -> &'static str {
        match self {
            Self::Pass => "Pass[]",
            Self::TryEmptyHand => "TryEmptyHandInteraction[]",
            Self::Fail => "Fail[]",
            Self::Success {
                swing: true,
                item_interaction: true,
            } => {
                "Success[swingSource=CLIENT, itemContext=ItemContext[wasItemInteraction=true, heldItemTransformedTo=null]]"
            }
            Self::Success {
                swing: true,
                item_interaction: false,
            } => {
                "Success[swingSource=CLIENT, itemContext=ItemContext[wasItemInteraction=false, heldItemTransformedTo=null]]"
            }
            Self::Success {
                swing: false,
                item_interaction: true,
            } => {
                "Success[swingSource=NONE, itemContext=ItemContext[wasItemInteraction=true, heldItemTransformedTo=null]]"
            }
            Self::Success {
                swing: false,
                item_interaction: false,
            } => {
                "Success[swingSource=NONE, itemContext=ItemContext[wasItemInteraction=false, heldItemTransformedTo=null]]"
            }
        }
    }
}

pub(super) fn short_class(class: &str) -> &str {
    class.rsplit('.').next().unwrap_or(class)
}

fn item_name<'a>(item: Option<&Stack>, content: &'a Content) -> Result<&'a str> {
    match item.filter(|item| !item.empty()) {
        Some(item) => content
            .item_names
            .get(&item.id)
            .map(String::as_str)
            .ok_or_else(|| "unknown_held_item".into()),
        None => Ok("minecraft:air"),
    }
}

fn item_tag(item: Option<&Stack>, content: &Content, tag: &str) -> Result<bool> {
    let Some(item) = item.filter(|item| !item.empty()) else {
        return Ok(false);
    };
    Ok(content
        .registry_tag("minecraft:item", tag)?
        .contains(&(item.id as i32)))
}

fn property<'a>(content: &'a Content, state: u32, key: &str) -> Result<&'a str> {
    content
        .state_properties(state)
        .and_then(|properties| properties.get(key))
        .map(String::as_str)
        .ok_or_else(|| format!("missing source block property {key} for state {state}"))
}

fn number(content: &Content, state: u32, key: &str) -> Result<i32> {
    property(content, state, key)?
        .parse()
        .map_err(|_| format!("invalid integer block property {key}"))
}

fn boolean(content: &Content, state: u32, key: &str) -> Result<bool> {
    property(content, state, key)?
        .parse()
        .map_err(|_| format!("invalid boolean block property {key}"))
}

fn be_name<'a>(
    position: [i32; 3],
    world: &World,
    entities: &'a EntityStore,
    content: &'a Content,
) -> Option<&'a str> {
    entities.block_entity_name(position).or_else(|| {
        world
            .chunk([position[0].div_euclid(16), position[2].div_euclid(16)])
            .and_then(|chunk| {
                chunk
                    .block_entities
                    .iter()
                    .find(|be| be.position == position)
            })
            .and_then(|be| content.block_entity_names.get(&be.kind).map(String::as_str))
    })
}

fn be_data<'a>(
    position: [i32; 3],
    world: &'a World,
    entities: &'a EntityStore,
) -> Option<&'a Value> {
    entities.block_entity_data(position).or_else(|| {
        world
            .chunk([position[0].div_euclid(16), position[2].div_euclid(16)])
            .and_then(|chunk| {
                chunk
                    .block_entities
                    .iter()
                    .find(|be| be.position == position)
            })
            .map(|be| be.data.as_ref())
    })
}

fn nbt_boolean(value: &Value) -> bool {
    value
        .as_bool()
        .unwrap_or_else(|| value.as_i64().is_some_and(|value| value != 0))
}

const DYE_COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

fn colored_item(name: &str, suffix: &str) -> bool {
    name.strip_prefix("minecraft:")
        .and_then(|name| name.strip_suffix(suffix))
        .is_some_and(|color| DYE_COLORS.contains(&color))
}

fn facing_face(facing: &str) -> Result<i32> {
    match facing {
        "down" => Ok(0),
        "up" => Ok(1),
        "north" => Ok(2),
        "south" => Ok(3),
        "west" => Ok(4),
        "east" => Ok(5),
        _ => Err("invalid block facing".into()),
    }
}

fn face_axis(face: i32) -> usize {
    match face {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 0,
    }
}

fn hit_slot(hit: &BlockHit, facing: &str, rows: i32) -> Result<Option<usize>> {
    if facing_face(facing)? != hit.face || hit.face < 2 {
        return Ok(None);
    }
    let relative = std::array::from_fn::<_, 3, _>(|i| hit.point[i] - f64::from(hit.position[i]));
    let x = match hit.face {
        2 => 1. - relative[0],
        3 => relative[0],
        4 => relative[2],
        _ => 1. - relative[2],
    } as f32;
    let section = |coordinate: f32, count: i32| {
        ((coordinate * 16. / (16. / count as f32)).floor() as i32).clamp(0, count - 1)
    };
    Ok(Some(
        (section(x, 3) + section(1. - relative[1] as f32, rows) * 3) as usize,
    ))
}

impl Player {
    pub(super) fn block_item_result(
        &self,
        owner: &str,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
    ) -> Result<Outcome> {
        let item = super::interaction::held(inventory, hand);
        let name = item_name(item, content)?;
        let state = hit.state;
        let block = content.name(state).ok_or("unknown_block_state")?;
        let has_be = |kind| be_name(hit.position, world, entities, content) == Some(kind);
        let result = match short_class(owner) {
            "BlockBehaviour" => Outcome::TryEmptyHand,
            "NoteBlock" => {
                if hit.face == 1 && item_tag(item, content, "minecraft:note_block_top_instruments")?
                {
                    Outcome::Pass
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "TntBlock" => {
                if matches!(name, "minecraft:flint_and_steel" | "minecraft:fire_charge") {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "ChiseledBookShelfBlock" => {
                if !has_be("minecraft:chiseled_bookshelf") {
                    Outcome::Pass
                } else if !item_tag(item, content, "minecraft:bookshelf_books")? {
                    Outcome::TryEmptyHand
                } else if let Some(slot) = hit_slot(hit, property(content, state, "facing")?, 2)? {
                    if boolean(content, state, &format!("slot_{slot}_occupied"))? {
                        Outcome::TryEmptyHand
                    } else {
                        Outcome::CLIENT
                    }
                } else {
                    Outcome::Pass
                }
            }
            "ShelfBlock" => {
                if hand == 1
                    || !has_be("minecraft:shelf")
                    || hit_slot(hit, property(content, state, "facing")?, 1)?.is_none()
                {
                    Outcome::Pass
                } else if item.is_none_or(Stack::empty) {
                    Outcome::Pass
                } else {
                    Outcome::CLIENT
                }
            }
            "SignBlock" | "CeilingHangingSignBlock" | "WallHangingSignBlock" => {
                if !has_be("minecraft:sign") && !has_be("minecraft:hanging_sign") {
                    Outcome::Pass
                } else {
                    let chains = name.ends_with("_hanging_sign")
                        && match short_class(owner) {
                            "CeilingHangingSignBlock" => hit.face == 0,
                            "WallHangingSignBlock" => {
                                face_axis(hit.face)
                                    != face_axis(facing_face(property(content, state, "facing")?)?)
                            }
                            _ => false,
                        };
                    if chains && !self.sign_click_command(hit, world, entities, content)? {
                        Outcome::Pass
                    } else {
                        let applicator = self.abilities.may_build
                            && (name.ends_with("_dye")
                                || matches!(
                                    name,
                                    "minecraft:glow_ink_sac"
                                        | "minecraft:ink_sac"
                                        | "minecraft:honeycomb"
                                ));
                        let waxed = be_data(hit.position, world, entities)
                            .and_then(|data| data.get("is_waxed"))
                            .is_some_and(nbt_boolean);
                        if !applicator && !waxed {
                            Outcome::CONSUME
                        } else {
                            Outcome::CLIENT
                        }
                    }
                }
            }
            "RedStoneOreBlock" => {
                let block_item = item
                    .map(|item| content.item_block(item.id))
                    .transpose()?
                    .flatten()
                    .is_some();
                if block_item && self.block_place_can_replace(hit, item, hand, world, content)? {
                    Outcome::Pass
                } else {
                    Outcome::CLIENT
                }
            }
            "JukeboxBlock" => {
                if !boolean(content, state, "has_record")?
                    && item
                        .is_some_and(|item| item.component("minecraft:jukebox_playable").is_some())
                {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "CakeBlock" => {
                if number(content, state, "bites")? == 0
                    && item_tag(item, content, "minecraft:candles")?
                {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "PumpkinBlock" => {
                if name == "minecraft:shears" {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "AbstractCauldronBlock" => self.cauldron_result(hit, item, world, content)?,
            "FlowerPotBlock" => {
                let pottable = item
                    .map(|item| content.item_block(item.id))
                    .transpose()?
                    .flatten()
                    .map(|state| content.block_potted_state(state))
                    .transpose()?
                    .flatten()
                    .is_some();
                if !pottable {
                    Outcome::TryEmptyHand
                } else if block == "minecraft:flower_pot" {
                    Outcome::CLIENT
                } else {
                    Outcome::CONSUME
                }
            }
            "LecternBlock" => {
                if boolean(content, state, "has_book")? {
                    Outcome::TryEmptyHand
                } else if item_tag(item, content, "minecraft:lectern_books")? {
                    Outcome::CLIENT
                } else if hand == 0 && item.is_none_or(Stack::empty) {
                    Outcome::Pass
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "CampfireBlock" => {
                if has_be("minecraft:campfire")
                    && item.is_some_and(|item| {
                        inventory.recipe_properties_contains("minecraft:campfire_input", item.id)
                    })
                {
                    Outcome::CONSUME
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "SweetBerryBushBlock" => {
                if number(content, state, "age")? != 3 && name == "minecraft:bone_meal" {
                    Outcome::Pass
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "ComposterBlock" => {
                if number(content, state, "level")? < 8
                    && item.is_some_and(|item| content.item_is_compostable(item.id))
                {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "BeehiveBlock" => {
                if number(content, state, "honey_level")? >= 5 && name == "minecraft:glass_bottle" {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "RespawnAnchorBlock" => {
                if number(content, state, "charges")? < 4 && name == "minecraft:glowstone" {
                    Outcome::CLIENT
                } else if hand == 0
                    && number(content, state, "charges")? < 4
                    && item_name(super::interaction::held(inventory, 1), content)?
                        == "minecraft:glowstone"
                {
                    Outcome::Pass
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "CandleBlock" => {
                if item.is_none_or(Stack::empty)
                    && self.abilities.may_build
                    && boolean(content, state, "lit")?
                {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "CandleCakeBlock" => {
                if matches!(name, "minecraft:flint_and_steel" | "minecraft:fire_charge") {
                    Outcome::Pass
                } else if hit.point[1] - f64::from(hit.position[1]) > 0.5
                    && item.is_none_or(Stack::empty)
                    && boolean(content, state, "lit")?
                {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "CopperGolemStatueBlock" => {
                if item_tag(item, content, "minecraft:axes")? {
                    Outcome::Pass
                } else {
                    Outcome::CLIENT
                }
            }
            "WeatheringCopperGolemStatueBlock" => {
                if !has_be("minecraft:copper_golem_statue") {
                    Outcome::Pass
                } else if item_tag(item, content, "minecraft:axes")? {
                    if block == "minecraft:copper_golem_statue" {
                        Outcome::CLIENT
                    } else {
                        Outcome::Pass
                    }
                } else if name == "minecraft:honeycomb" {
                    Outcome::Pass
                } else {
                    Outcome::CLIENT
                }
            }
            "DecoratedPotBlock" => {
                if has_be("minecraft:decorated_pot") {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "VaultBlock" => {
                if item.is_some_and(|item| !item.empty())
                    && property(content, state, "vault_state")? == "active"
                {
                    Outcome::CONSUME
                } else {
                    Outcome::TryEmptyHand
                }
            }
            owner => {
                return Err(format!(
                    "unknown verified block item interaction owner {owner}"
                ));
            }
        };
        Ok(result)
    }

    pub(super) fn block_empty_result(
        &mut self,
        owner: &str,
        hit: &BlockHit,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
    ) -> Result<Outcome> {
        let state = hit.state;
        let has_be = |kind| be_name(hit.position, world, entities, content) == Some(kind);
        let result = match short_class(owner) {
            "BlockBehaviour" | "MovingPistonBlock" | "FenceBlock" => Outcome::Pass,
            "DispenserBlock"
            | "NoteBlock"
            | "ChestBlock"
            | "CraftingTableBlock"
            | "AbstractFurnaceBlock"
            | "LeverBlock"
            | "FenceGateBlock"
            | "EnchantingTableBlock"
            | "BrewingStandBlock"
            | "DragonEggBlock"
            | "EnderChestBlock"
            | "BeaconBlock"
            | "AnvilBlock"
            | "HopperBlock"
            | "ShulkerBoxBlock"
            | "LoomBlock"
            | "BarrelBlock"
            | "CartographyTableBlock"
            | "GrindstoneBlock"
            | "SmithingTableBlock"
            | "StonecutterBlock"
            | "CrafterBlock" => Outcome::CLIENT,
            "BedBlock" | "LightBlock" => Outcome::CONSUME,
            "SignBlock" => {
                if has_be("minecraft:sign") || has_be("minecraft:hanging_sign") {
                    Outcome::CONSUME
                } else {
                    Outcome::Pass
                }
            }
            "DoorBlock" | "TrapDoorBlock" => {
                if content.block_can_open_by_hand(state)? {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "RepeaterBlock" | "ComparatorBlock" | "DaylightDetectorBlock" => {
                if self.abilities.may_build {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "RedStoneWireBlock" => {
                let dot = ["north", "south", "west", "east"]
                    .into_iter()
                    .all(|key| property(content, state, key) == Ok("none"));
                let cross = ["north", "south", "west", "east"]
                    .into_iter()
                    .all(|key| property(content, state, key).is_ok_and(|value| value != "none"));
                if self.abilities.may_build
                    && (dot || cross)
                    && self.toggled_wire_state(hit.position, state, world, content)? != state
                {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "ButtonBlock" => {
                if boolean(content, state, "powered")? {
                    Outcome::CONSUME
                } else {
                    Outcome::CLIENT
                }
            }
            "JukeboxBlock" => {
                if boolean(content, state, "has_record")? && has_be("minecraft:jukebox") {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "CakeBlock" => {
                if self.food.unwrap_or(20) < 20 || self.abilities.invulnerable {
                    Outcome::CLIENT
                } else if super::interaction::held(inventory, 0).is_none_or(Stack::empty) {
                    Outcome::CONSUME
                } else {
                    Outcome::Pass
                }
            }
            "CandleCakeBlock" => {
                if self.food.unwrap_or(20) < 20 || self.abilities.invulnerable {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "FlowerPotBlock" => {
                if content.name(state) == Some("minecraft:flower_pot") {
                    Outcome::CONSUME
                } else {
                    Outcome::CLIENT
                }
            }
            "LecternBlock" => {
                if boolean(content, state, "has_book")? {
                    Outcome::CLIENT
                } else {
                    Outcome::CONSUME
                }
            }
            "SweetBerryBushBlock" => {
                if number(content, state, "age")? > 1 {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "ComposterBlock" => {
                if number(content, state, "level")? == 8 {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "RespawnAnchorBlock" => {
                if number(content, state, "charges")? > 0 {
                    Outcome::CONSUME
                } else {
                    Outcome::Pass
                }
            }
            "CaveVinesBlock" | "CaveVinesPlantBlock" => {
                if boolean(content, state, "berries")? {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "DecoratedPotBlock" => {
                if has_be("minecraft:decorated_pot") {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "ChiseledBookShelfBlock" => {
                if !has_be("minecraft:chiseled_bookshelf") {
                    Outcome::Pass
                } else if let Some(slot) = hit_slot(hit, property(content, state, "facing")?, 2)? {
                    if boolean(content, state, &format!("slot_{slot}_occupied"))? {
                        Outcome::CLIENT
                    } else {
                        Outcome::CONSUME
                    }
                } else {
                    Outcome::Pass
                }
            }
            "BellBlock" => {
                let axis = face_axis(hit.face);
                let facing = face_axis(facing_face(property(content, state, "facing")?)?);
                let proper = axis != 1
                    && hit.point[1] - f64::from(hit.position[1]) <= f64::from(0.8124_f32)
                    && match property(content, state, "attachment")? {
                        "floor" => axis == facing,
                        "single_wall" | "double_wall" => axis != facing,
                        "ceiling" => true,
                        _ => return Err("invalid bell attachment".into()),
                    };
                if proper {
                    Outcome::CLIENT
                } else {
                    Outcome::Pass
                }
            }
            "CommandBlock" | "StructureBlock" | "JigsawBlock" | "TestBlock"
            | "TestInstanceBlock" => {
                let kind = match short_class(owner) {
                    "CommandBlock" => "minecraft:command_block",
                    "StructureBlock" => "minecraft:structure_block",
                    "JigsawBlock" => "minecraft:jigsaw",
                    "TestBlock" => "minecraft:test_block",
                    _ => "minecraft:test_instance_block",
                };
                if self.game_mode != 1
                    || !self.permission_level.is_some_and(|level| level >= 2)
                    || !has_be(kind)
                {
                    Outcome::Pass
                } else {
                    if self.ordinary_ui.len() >= 16 {
                        return Err("ordinary_ui_trigger_queue_bound".into());
                    }
                    let position = hit.position;
                    self.ordinary_ui.push_back(match short_class(owner) {
                        "CommandBlock" => UiTrigger::Command { position },
                        "StructureBlock" => UiTrigger::Structure { position },
                        "JigsawBlock" => UiTrigger::Jigsaw { position },
                        "TestBlock" => UiTrigger::TestBlock { position },
                        _ => UiTrigger::TestInstanceBlock { position },
                    });
                    Outcome::CLIENT
                }
            }
            owner => {
                return Err(format!(
                    "unknown verified block empty interaction owner {owner}"
                ));
            }
        };
        Ok(result)
    }
}

impl Player {
    fn cauldron_result(
        &self,
        hit: &BlockHit,
        item: Option<&Stack>,
        world: &World,
        content: &Content,
    ) -> Result<Outcome> {
        let name = item_name(item, content)?;
        if name == "minecraft:water_bucket" {
            return Ok(Outcome::CLIENT);
        }
        if matches!(
            name,
            "minecraft:lava_bucket" | "minecraft:powder_snow_bucket"
        ) {
            let above = [hit.position[0], hit.position[1] + 1, hit.position[2]];
            let fluid = content
                .info(world.block(above).ok_or("unloaded_cauldron_neighborhood")?)?
                .fluid;
            return Ok(
                if fluid.is_some_and(|fluid| fluid.kind == FluidKind::Water) {
                    Outcome::CONSUME
                } else {
                    Outcome::CLIENT
                },
            );
        }
        let water_potion = name == "minecraft:potion"
            && item
                .and_then(|item| item.component("minecraft:potion_contents"))
                .and_then(|value| value.get("potion"))
                .and_then(Value::as_str)
                == Some("minecraft:water");
        let result = match content.name(hit.state).ok_or("unknown_cauldron_state")? {
            "minecraft:cauldron" if water_potion => Outcome::CLIENT,
            "minecraft:water_cauldron" => {
                let level = number(content, hit.state, "level")?;
                if name == "minecraft:bucket" && level == 3
                    || name == "minecraft:glass_bottle"
                    || water_potion && level < 3
                    || item_tag(item, content, "minecraft:cauldron_can_remove_dye")?
                        && item.is_some_and(|item| item.component("minecraft:dyed_color").is_some())
                    || colored_item(name, "_shulker_box")
                    || colored_item(name, "_banner")
                        && item
                            .and_then(|item| item.component("minecraft:banner_patterns"))
                            .and_then(Value::as_array)
                            .is_some_and(|patterns| !patterns.is_empty())
                {
                    Outcome::CLIENT
                } else {
                    Outcome::TryEmptyHand
                }
            }
            "minecraft:lava_cauldron" if name == "minecraft:bucket" => Outcome::CLIENT,
            "minecraft:powder_snow_cauldron"
                if name == "minecraft:bucket" && number(content, hit.state, "level")? == 3 =>
            {
                Outcome::CLIENT
            }
            _ => Outcome::TryEmptyHand,
        };
        Ok(result)
    }

    fn sign_click_command(
        &self,
        hit: &BlockHit,
        world: &World,
        entities: &EntityStore,
        content: &Content,
    ) -> Result<bool> {
        let Some(data) = be_data(hit.position, world, entities) else {
            return Ok(false);
        };
        if !data.get("is_waxed").is_some_and(nbt_boolean) {
            return Ok(false);
        }
        let name = content.name(hit.state).ok_or("unknown_sign_state")?;
        let rotation = if let Some(rotation) = content
            .state_properties(hit.state)
            .and_then(|properties| properties.get("rotation"))
        {
            rotation
                .parse::<f32>()
                .map_err(|_| "invalid_sign_rotation")?
                * 22.5
        } else {
            match property(content, hit.state, "facing")? {
                "south" => 0.,
                "west" => 90.,
                "north" => 180.,
                "east" => 270.,
                _ => return Err("invalid_sign_facing".into()),
            }
        };
        let mut center = [0.5; 3];
        if name.ends_with("_wall_sign") {
            let shapes = content
                .selection_boxes(hit.state)
                .ok_or("unknown_sign_shape")?;
            let mut min = [f64::INFINITY; 3];
            let mut max = [f64::NEG_INFINITY; 3];
            for shape in shapes {
                for axis in 0..3 {
                    min[axis] = min[axis].min(shape[axis]);
                    max[axis] = max[axis].max(shape[axis + 3]);
                }
            }
            if shapes.is_empty() {
                return Err("empty_wall_sign_shape".into());
            }
            center = std::array::from_fn(|axis| (min[axis] + max[axis]) * 0.5);
        }
        let dx = self.position[0] - (f64::from(hit.position[0]) + center[0]);
        let dz = self.position[2] - (f64::from(hit.position[2]) + center[2]);
        let player_rotation = (dz.atan2(dx) * 180. / f64::from(std::f32::consts::PI)) as f32 - 90.;
        let difference = (rotation - player_rotation + 180.).rem_euclid(360.) - 180.;
        let side = if difference.abs() <= 90. {
            "front_text"
        } else {
            "back_text"
        };
        // Local Player.isTextFilteringEnabled() is false in the pinned client.
        Ok(data
            .get(side)
            .and_then(|text| text.get("messages"))
            .and_then(Value::as_array)
            .is_some_and(|messages| {
                messages.iter().any(|message| {
                    message
                        .get("click_event")
                        .and_then(|event| event.get("action"))
                        .and_then(Value::as_str)
                        == Some("run_command")
                })
            }))
    }
}

const DIRECTIONS: [[i32; 3]; 6] = [
    [0, -1, 0],
    [0, 1, 0],
    [0, 0, -1],
    [0, 0, 1],
    [-1, 0, 0],
    [1, 0, 0],
];
fn relative(position: [i32; 3], face: i32) -> Result<[i32; 3]> {
    let offset = DIRECTIONS
        .get(usize::try_from(face).map_err(|_| "invalid_hit_face")?)
        .ok_or("invalid_hit_face")?;
    Ok(std::array::from_fn(|axis| position[axis] + offset[axis]))
}

impl Player {
    fn block_can_replace(
        &self,
        state: u32,
        hit: &BlockHit,
        item: Option<&Stack>,
        replacing_clicked: bool,
        content: &Content,
    ) -> Result<bool> {
        let block = content.info(state)?.block;
        let same = item
            .filter(|item| !item.empty())
            .map(|item| content.item_block(item.id))
            .transpose()?
            .flatten()
            .map(|state| content.info(state).map(|info| info.block == block))
            .transpose()?
            .unwrap_or(false);
        let base = content.block_replaceable(state)? && !same;
        let secondary = self.last_sent_input & 32 != 0;
        let result = match short_class(content.block_placement_owners(state)?.1) {
            "BlockBehaviour" => base,
            "PitcherCropBlock" => false,
            "SlabBlock" => {
                let slab_type = property(content, state, "type")?;
                if slab_type == "double" || !same {
                    false
                } else if !replacing_clicked {
                    true
                } else {
                    let above = hit.point[1] - f64::from(hit.position[1]) > 0.5;
                    if slab_type == "bottom" {
                        hit.face == 1 || above && hit.face >= 2
                    } else {
                        hit.face == 0 || !above && hit.face >= 2
                    }
                }
            }
            "SnowLayerBlock" => {
                let layers = number(content, state, "layers")?;
                if !same || layers >= 8 {
                    layers == 1
                } else {
                    !replacing_clicked || hit.face == 1
                }
            }
            "VineBlock" => {
                ["up", "north", "south", "west", "east"]
                    .into_iter()
                    .filter(|key| property(content, state, key) == Ok("true"))
                    .count()
                    < 5
            }
            "MultifaceBlock" => {
                !same
                    || ["down", "up", "north", "south", "west", "east"]
                        .into_iter()
                        .any(|key| property(content, state, key) == Ok("false"))
            }
            "ScaffoldingBlock" => same,
            "TurtleEggBlock" => !secondary && same && number(content, state, "eggs")? < 4 || base,
            "SeaPickleBlock" => {
                !secondary && same && number(content, state, "pickles")? < 4 || base
            }
            "CandleBlock" => !secondary && same && number(content, state, "candles")? < 4 || base,
            "FlowerBedBlock" => {
                !secondary && same && number(content, state, "flower_amount")? < 4 || base
            }
            "LeafLitterBlock" => {
                !secondary && same && number(content, state, "segment_amount")? < 4 || base
            }
            "GrowingPlantBodyBlock" => {
                let head = match content.name(state).ok_or("unknown_growing_plant")? {
                    "minecraft:kelp_plant" => "minecraft:kelp",
                    "minecraft:weeping_vines_plant" => "minecraft:weeping_vines",
                    "minecraft:twisting_vines_plant" => "minecraft:twisting_vines",
                    "minecraft:cave_vines_plant" => "minecraft:cave_vines",
                    _ => return Err("invalid_source_growing_plant_body".into()),
                };
                base && !item
                    .filter(|item| !item.empty())
                    .map(|item| content.item_block(item.id))
                    .transpose()?
                    .flatten()
                    .is_some_and(|block| content.name(block) == Some(head))
            }
            owner => return Err(format!("unknown verified replacement owner {owner}")),
        };
        Ok(result)
    }

    fn block_place_can_replace(
        &self,
        hit: &BlockHit,
        item: Option<&Stack>,
        _hand: u8,
        world: &World,
        content: &Content,
    ) -> Result<bool> {
        if self.block_can_replace(hit.state, hit, item, true, content)? {
            return Ok(true);
        }
        let position = relative(hit.position, hit.face)?;
        let state = world
            .block(position)
            .ok_or("unloaded_placement_neighborhood")?;
        self.block_can_replace(state, hit, item, false, content)
    }
}

impl Player {
    pub(super) fn predict_use(&mut self, prediction: UsePrediction) -> Result<()> {
        if self.use_predictions.len() >= 256 {
            return Err("ordinary_use_prediction_queue_bound".into());
        }
        self.use_predictions.push_back(prediction);
        Ok(())
    }
    fn use_block(&self, world: &World, position: [i32; 3]) -> Result<u32> {
        self.use_predictions
            .iter()
            .rev()
            .find_map(|prediction| match prediction {
                UsePrediction::WorldBlock {
                    position: changed,
                    state,
                    ..
                } if *changed == position => Some(*state),
                _ => None,
            })
            .or_else(|| world.block(position))
            .ok_or_else(|| "unloaded_use_neighborhood".to_owned())
    }
    fn predict_block(
        &mut self,
        world: &World,
        position: [i32; 3],
        state: u32,
        flags: i32,
    ) -> Result<bool> {
        if !world.can_set_block(position)? || self.use_block(world, position)? == state {
            return Ok(false);
        }
        self.predict_use(UsePrediction::WorldBlock {
            position,
            state,
            flags,
            sequence: self.sequence,
            player_position: self.position,
        })?;
        Ok(true)
    }
    fn predict_held(
        &mut self,
        hand: u8,
        stack: Option<Stack>,
        inventory: &InventoryStore,
        replacement: bool,
    ) -> Result<()> {
        self.predict_use(UsePrediction::PlayerSlot {
            slot: if hand == 1 {
                45
            } else {
                36 + u16::from(inventory.selected_slot)
            },
            stack,
            replacement,
        })
    }
    fn consume_held(&mut self, hand: u8, inventory: &InventoryStore, count: i32) -> Result<()> {
        if self.abilities.instabuild {
            return Ok(());
        }
        let Some(stack) = super::interaction::held(inventory, hand) else {
            return Ok(());
        };
        let mut stack = stack.clone();
        stack.count = (stack.count - count).max(0);
        self.predict_held(hand, (!stack.empty()).then_some(stack), inventory, false)
    }
}

fn field<'a>(value: &'a Value, key: &str) -> Result<&'a Value> {
    value
        .get(key)
        .ok_or_else(|| format!("missing numeric effect field {key}"))
}
fn scalar(value: &Value) -> Result<f32> {
    value
        .as_f64()
        .map(|value| value as f32)
        .ok_or_else(|| "invalid numeric effect scalar".into())
}
fn level_value(value: &Value, level: i32, depth: u8) -> Result<f32> {
    if depth >= 64 {
        return Err("enchantment_value_depth".into());
    }
    if value.is_number() {
        return scalar(value);
    }
    let calculate = |key| level_value(field(value, key)?, level, depth + 1);
    Ok(
        match field(value, "type")?
            .as_str()
            .ok_or("invalid level value type")?
        {
            "minecraft:constant" => scalar(field(value, "value")?)?,
            "minecraft:linear" => {
                scalar(field(value, "base")?)?
                    + scalar(field(value, "per_level_above_first")?)? * (level - 1) as f32
            }
            "minecraft:levels_squared" => {
                level.wrapping_mul(level) as f32 + scalar(field(value, "added")?)?
            }
            "minecraft:clamped" => calculate("value")?
                .clamp(scalar(field(value, "min")?)?, scalar(field(value, "max")?)?),
            "minecraft:fraction" => {
                let denominator = calculate("denominator")?;
                if denominator == 0. {
                    0.
                } else {
                    calculate("numerator")? / denominator
                }
            }
            "minecraft:exponent" => {
                f64::from(calculate("base")?).powf(f64::from(calculate("power")?)) as f32
            }
            "minecraft:lookup" => {
                let values = field(value, "values")?
                    .as_array()
                    .ok_or("invalid level lookup")?;
                if level <= values.len() as i32 {
                    scalar(
                        values
                            .get(
                                usize::try_from(level - 1)
                                    .map_err(|_| "invalid level lookup index")?,
                            )
                            .ok_or("invalid level lookup index")?,
                    )?
                } else {
                    calculate("fallback")?
                }
            }
            kind => return Err(format!("unregistered source level value type {kind}")),
        },
    )
}
fn enchantment<'a>(content: &'a Content, name: &str) -> Result<&'a Value> {
    content
        .registry_entries("minecraft:enchantment")?
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
        .ok_or_else(|| format!("unknown enchantment {name}"))
}
fn enchantment_has(item: Option<&Stack>, name: &str, content: &Content) -> Result<bool> {
    let Some(values) = item
        .and_then(|item| item.component("minecraft:enchantments"))
        .and_then(Value::as_object)
    else {
        return Ok(false);
    };
    for (key, _) in values {
        if enchantment(content, key)?
            .get("effects")
            .and_then(|effects| effects.get(name))
            .is_some()
        {
            return Ok(true);
        }
    }
    Ok(false)
}
fn next_damage_breaks(item: &Stack) -> bool {
    !item.components.contains_key("minecraft:unbreakable")
        && item.component("minecraft:damage").is_some()
        && item
            .component("minecraft:max_damage")
            .and_then(Value::as_i64)
            .is_some_and(|maximum| {
                item.component("minecraft:damage")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    >= maximum - 1
            })
}

impl Player {
    fn random_bits(&mut self, bits: u32) -> u32 {
        self.interaction_random = self
            .interaction_random
            .wrapping_mul(0x5DEECE66D)
            .wrapping_add(0xB)
            & ((1u64 << 48) - 1);
        (self.interaction_random >> (48 - bits)) as u32
    }
    fn random_float(&mut self) -> f32 {
        self.random_bits(24) as f32 * (1. / 16777216.)
    }
    fn random_double(&mut self) -> f64 {
        let high = u64::from(self.random_bits(26));
        let low = u64::from(self.random_bits(27));
        ((high << 27) + low) as f64 * (1. / 9007199254740992.)
    }
    fn random_gaussian(&mut self) -> f64 {
        if let Some(value) = self.interaction_gaussian.take() {
            return value;
        }
        loop {
            let x = 2. * self.random_double() - 1.;
            let y = 2. * self.random_double() - 1.;
            let square = x * x + y * y;
            if square >= 1. || square == 0. {
                continue;
            }
            let multiplier = (-2. * square.ln() / square).sqrt();
            self.interaction_gaussian = Some(y * multiplier);
            return x * multiplier;
        }
    }
    fn value_effect(&mut self, effect: &Value, level: i32, input: f32, depth: u8) -> Result<f32> {
        if depth >= 64 {
            return Err("enchantment_effect_depth".into());
        }
        let calculate = |key| level_value(field(effect, key)?, level, 0);
        Ok(
            match field(effect, "type")?
                .as_str()
                .ok_or("invalid enchantment effect type")?
            {
                "minecraft:add" => input + calculate("value")?,
                "minecraft:multiply" => input * calculate("factor")?,
                "minecraft:set" => calculate("value")?,
                "minecraft:exponential" => {
                    (f64::from(input)
                        * f64::from(calculate("base")?).powf(f64::from(calculate("exponent")?)))
                        as f32
                }
                "minecraft:all_of" => {
                    let mut result = input;
                    for effect in field(effect, "effects")?
                        .as_array()
                        .ok_or("invalid all_of effects")?
                    {
                        result = self.value_effect(effect, level, result, depth + 1)?
                    }
                    result
                }
                "minecraft:remove_binomial" => {
                    let probability = calculate("chance")?;
                    let mut removed = 0;
                    if input <= 128.
                        || input * probability < 20.
                        || input * (1. - probability) < 20.
                    {
                        let mut index = 0;
                        while (index as f32) < input {
                            if self.random_float() < probability {
                                removed += 1
                            }
                            index += 1;
                        }
                    } else {
                        let mean = f64::from(input * probability).floor();
                        let sigma = f64::from(input * probability * (1. - probability)).sqrt();
                        removed = ((mean + self.random_gaussian() * sigma + 0.5).floor() as i32)
                            .clamp(0, input as i32);
                    }
                    input - removed as f32
                }
                kind => return Err(format!("unregistered source numeric effect type {kind}")),
            },
        )
    }
    pub(super) fn trident_spin_strength(&mut self, item: &Stack, content: &Content) -> Result<f32> {
        let Some(values) = item
            .component("minecraft:enchantments")
            .and_then(Value::as_object)
        else {
            return Ok(0.);
        };
        let mut strength = 0.;
        for (name, level) in values {
            let level = level
                .as_i64()
                .and_then(|value| i32::try_from(value).ok())
                .ok_or("invalid enchantment level")?;
            if let Some(effect) = enchantment(content, name)?
                .get("effects")
                .and_then(|effects| effects.get("minecraft:trident_spin_attack_strength"))
            {
                strength = self.value_effect(effect, level, strength, 0)?;
            }
        }
        Ok(strength)
    }
    pub(super) fn in_water_or_rain(
        &mut self,
        world: &World,
        content: &Content,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<bool> {
        if self.source_in_water(world, content, entities, tick)? {
            return Ok(true);
        }
        self.rain_at_position
            .ok_or_else(|| "source_rain_column_not_available".into())
    }
}

impl Player {
    pub fn apply_pending_use_release(
        &mut self,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<()> {
        let Some(using) = self.pending_use_release.take() else {
            return Ok(());
        };
        let Some(item) = super::interaction::held(inventory, using.hand)
            .filter(|item| !item.empty() && item.id == using.item_id)
        else {
            return Ok(());
        };
        if short_class(content.item_use_owner(item.id, false)?) == "SpyglassItem" {
            return self.play_use_local_sound(world, "minecraft:item.spyglass.stop_using", 1., 1.);
        }
        if short_class(content.item_use_owner(item.id, false)?) != "TridentItem"
            || using.duration.wrapping_sub(using.remaining) < 10
        {
            return Ok(());
        }
        let strength = self.trident_spin_strength(item, content)?;
        if !(strength > 0.)
            || self.vehicle.is_some()
            || next_damage_breaks(item)
            || !self.in_water_or_rain(world, content, entities, tick)?
        {
            return Ok(());
        }
        let yaw = f64::from(self.yaw * 0.017453292);
        let pitch = f64::from(self.pitch * 0.017453292);
        let mut direction = [
            -super::math::sin(yaw) * super::math::cos(pitch),
            -super::math::sin(pitch),
            super::math::cos(yaw) * super::math::cos(pitch),
        ];
        let length = (direction[0] * direction[0]
            + direction[1] * direction[1]
            + direction[2] * direction[2])
            .sqrt();
        for axis in 0..3 {
            direction[axis] *= strength / length;
            self.velocity[axis] += f64::from(direction[axis]);
        }
        self.auto_spin_ticks = 20;
        self.start_auto_spin(world, content, inventory, entities, tick)
    }
}
