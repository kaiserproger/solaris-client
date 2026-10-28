//! Server-observed inventory lifetimes and source client-side menu prediction.
//! Wire contracts: target Clientbound*Container*, MenuType, ContainerInput,
//! ItemStack and ServerboundContainerClickPacket classes (26.1.2).
use crate::protocol::{Reader, Result, Writer, content::Content};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, LazyLock};
mod actions;
mod components;
mod menu_data;
mod persistent;
mod receive;
pub use actions::MenuButton;
pub use menu_data::{DisplayStack, ItemCost, Recipe, StonecutterRecipe, Trade, Trades};
pub mod nbt;
pub mod predicates;
pub mod snbt;
pub use crate::ui::hash::Value as Tag;
pub use predicates::AdventureBlock;

/// The persistent codec value and its wire payload are immutable and shared.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentValue {
    pub name: String,
    pub value: Tag,
    pub wire_range: std::ops::Range<usize>,
    pub adventure: Option<Arc<predicates::AdventurePredicate>>,
    pub hash: Option<u32>,
    pub nested_stacks: Arc<[Stack]>,
}
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ComponentPatch {
    pub added: BTreeMap<i32, ComponentValue>,
    pub removed: Vec<i32>,
}
fn empty_components() -> Arc<BTreeMap<String, Value>> {
    static EMPTY: LazyLock<Arc<BTreeMap<String, Value>>> =
        LazyLock::new(|| Arc::new(BTreeMap::new()));
    EMPTY.clone()
}
fn empty_patch() -> Arc<ComponentPatch> {
    static EMPTY: LazyLock<Arc<ComponentPatch>> =
        LazyLock::new(|| Arc::new(ComponentPatch::default()));
    EMPTY.clone()
}
fn empty_stacks() -> Arc<[Stack]> {
    static EMPTY: LazyLock<Arc<[Stack]>> = LazyLock::new(|| Arc::from([]));
    EMPTY.clone()
}
fn empty_wire() -> Arc<[u8]> {
    static EMPTY: LazyLock<Arc<[u8]>> = LazyLock::new(|| Arc::from([]));
    EMPTY.clone()
}

#[derive(Clone, Debug, Serialize)]
pub struct Stack {
    pub id: u32,
    pub count: i32,
    pub components: Arc<BTreeMap<String, Value>>,
    #[serde(skip)]
    pub wire: Arc<[u8]>,
    #[serde(skip)]
    pub patch: Arc<ComponentPatch>,
    // BundleContents selection is client-local, excluded from equality/codecs.
    #[serde(skip)]
    bundle_selected: i32,
}
impl PartialEq for Stack {
    fn eq(&self, other: &Self) -> bool {
        self.count == other.count && self.same_item(other)
    }
}
impl Default for Stack {
    fn default() -> Self {
        Self {
            id: 0,
            count: 0,
            components: empty_components(),
            wire: empty_wire(),
            patch: empty_patch(),
            bundle_selected: -1,
        }
    }
}
impl Stack {
    pub fn empty(&self) -> bool {
        self.count <= 0
    }
    pub fn same_item(&self, other: &Self) -> bool {
        self.id == other.id
            && self.components == other.components
            && self.patch.added.values().all(|value| {
                other
                    .typed_component(&value.name)
                    .is_none_or(|other| other == &value.value)
            })
    }
    pub fn component(&self, name: &str) -> Option<&Value> {
        self.components.get(name)
    }
    pub fn typed_component(&self, name: &str) -> Option<&Tag> {
        self.patch
            .added
            .values()
            .find(|v| v.name == name)
            .map(|v| &v.value)
    }
    pub fn bundle_selected_index(&self) -> i32 {
        self.bundle_selected
    }
    pub fn bundle_visible_items(&self) -> usize {
        let size = self
            .patch
            .added
            .values()
            .find(|value| value.name == "minecraft:bundle_contents")
            .map_or(0, |value| value.nested_stacks.len());
        let limit = if size > 12 { 11 } else { 12 };
        let padding = (4 - size % 4) % 4;
        size.min(limit - padding)
    }
    pub fn component_hashes(&self) -> Result<impl Iterator<Item = (i32, u32)> + '_> {
        if self.patch.added.values().any(|value| value.hash.is_none()) {
            return Err(
                "target HashOps rejects network-only component without persistent codec".into(),
            );
        }
        Ok(self
            .patch
            .added
            .iter()
            .map(|(&id, value)| (id, value.hash.unwrap())))
    }
    pub fn matches_adventure(
        &self,
        name: &str,
        block: &AdventureBlock<'_>,
        content: &Content,
    ) -> Result<bool> {
        if !matches!(name, "minecraft:can_break" | "minecraft:can_place_on") {
            return Err("not an adventure component".into());
        }
        if self.component(name).is_none() {
            return Ok(false);
        }
        let predicate = self
            .patch
            .added
            .values()
            .find(|value| value.name == name)
            .and_then(|value| value.adventure.as_deref())
            .ok_or("adventure component lacks retained typed predicate")?;
        predicate.matches(block, content)
    }
    pub fn max_count(&self) -> Result<i32> {
        let count = self
            .components
            .get("minecraft:max_stack_size")
            .and_then(Value::as_i64)
            .ok_or("verified stack lacks max_stack_size")?;
        if !(1..=99).contains(&count) {
            return Err("verified max_stack_size exceeds target bounds".into());
        }
        Ok(count as i32)
    }
    pub fn write(&self, w: &mut Writer) -> Result<()> {
        if self.empty() {
            w.var(0);
            return Ok(());
        }
        w.var(self.count);
        w.var(self.id as i32);
        self.write_patch(w)
    }
    fn write_template(&self, w: &mut Writer) -> Result<()> {
        if self.empty() {
            return Err("item stack template must be non-empty".into());
        }
        w.var(self.id as i32);
        w.var(self.count);
        self.write_patch(w)
    }
    fn write_patch(&self, w: &mut Writer) -> Result<()> {
        if self.wire.is_empty() {
            return Err("stack has no verified wire representation".into());
        }
        let mut source = Reader::new(&self.wire);
        source.var()?;
        source.var()?;
        let patch_start = source.offset;
        w.var(self.patch.added.len() as i32);
        w.var(self.patch.removed.len() as i32);
        for (id, value) in &self.patch.added {
            w.var(*id);
            w.0.extend_from_slice(
                &self.wire
                    [patch_start + value.wire_range.start..patch_start + value.wire_range.end],
            );
        }
        for id in &self.patch.removed {
            w.var(*id);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuKind {
    Inventory,
    Chest(u8),
    Dispenser,
    Crafter,
    Anvil,
    Beacon,
    BlastFurnace,
    BrewingStand,
    Crafting,
    Enchantment,
    Furnace,
    Grindstone,
    Hopper,
    Lectern,
    Loom,
    Merchant,
    ShulkerBox,
    Smithing,
    Smoker,
    Cartography,
    Stonecutter,
    Horse,
}
impl MenuKind {
    pub fn from_wire(id: i32) -> Result<Self> {
        Ok(match id {
            0..=5 => Self::Chest(id as u8 + 1),
            6 => Self::Dispenser,
            7 => Self::Crafter,
            8 => Self::Anvil,
            9 => Self::Beacon,
            10 => Self::BlastFurnace,
            11 => Self::BrewingStand,
            12 => Self::Crafting,
            13 => Self::Enchantment,
            14 => Self::Furnace,
            15 => Self::Grindstone,
            16 => Self::Hopper,
            17 => Self::Lectern,
            18 => Self::Loom,
            19 => Self::Merchant,
            20 => Self::ShulkerBox,
            21 => Self::Smithing,
            22 => Self::Smoker,
            23 => Self::Cartography,
            24 => Self::Stonecutter,
            _ => return Err(format!("unknown target menu registry ID {id}")),
        })
    }
    pub fn slot_count(self) -> usize {
        match self {
            Self::Inventory => 46,
            Self::Chest(n) => n as usize * 9 + 36,
            Self::Dispenser => 45,
            Self::Crafter => 46,
            Self::Anvil
            | Self::Furnace
            | Self::BlastFurnace
            | Self::Smoker
            | Self::Grindstone
            | Self::Merchant
            | Self::Cartography => 39,
            Self::Beacon => 37,
            Self::BrewingStand | Self::Hopper => 41,
            Self::Crafting => 46,
            Self::Enchantment | Self::Stonecutter => 38,
            Self::Lectern => 1,
            Self::Loom | Self::Smithing => 40,
            Self::ShulkerBox => 63,
            Self::Horse => 0,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Inventory => "inventory",
            Self::Chest(_) => "generic_54",
            Self::Dispenser => "dispenser",
            Self::Crafter => "crafter",
            Self::Anvil => "anvil",
            Self::Beacon => "beacon",
            Self::BlastFurnace => "blast_furnace",
            Self::BrewingStand => "brewing_stand",
            Self::Crafting => "crafting_table",
            Self::Enchantment => "enchanting_table",
            Self::Furnace => "furnace",
            Self::Grindstone => "grindstone",
            Self::Hopper => "hopper",
            Self::Lectern => "lectern",
            Self::Loom => "loom",
            Self::Merchant => "villager",
            Self::ShulkerBox => "shulker_box",
            Self::Smithing => "smithing",
            Self::Smoker => "smoker",
            Self::Cartography => "cartography_table",
            Self::Stonecutter => "stonecutter",
            Self::Horse => "horse",
        }
    }
    pub fn recipe_book(self) -> Option<i32> {
        match self {
            Self::Inventory | Self::Crafting => Some(0),
            Self::Furnace => Some(1),
            Self::BlastFurnace => Some(2),
            Self::Smoker => Some(3),
            _ => None,
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Menu {
    pub id: i32,
    pub kind: MenuKind,
    pub title: Value,
    pub state_id: i32,
    pub slots: Vec<Option<Stack>>,
    /// Received shorts are sign-extended; local DataSlot predictions retain i32.
    pub data: BTreeMap<i16, i32>,
    pub entity_id: Option<i32>,
    pub horse_columns: Option<u8>,
    pub mount_kind: Option<String>,
    pub trades: Option<Arc<Trades>>,
    #[serde(skip)]
    anvil_name: Option<String>,
    #[serde(skip)]
    anvil_hover_name: Option<String>,
    #[serde(skip)]
    repair_material_count: i32,
    #[serde(skip)]
    only_renaming: bool,
    #[serde(skip)]
    active_offer: Option<usize>,
}
impl Menu {
    fn new(id: i32, kind: MenuKind, title: Value) -> Self {
        Self {
            id,
            kind,
            title,
            state_id: 0,
            slots: vec![None; kind.slot_count()],
            data: match kind {
                MenuKind::Anvil | MenuKind::Loom | MenuKind::Stonecutter | MenuKind::Lectern => {
                    BTreeMap::from([(0, 0)])
                }
                MenuKind::Beacon => (0..3).map(|key| (key, 0)).collect(),
                MenuKind::Crafter => (0..10).map(|key| (key, 0)).collect(),
                MenuKind::Enchantment => (0..3)
                    .map(|key| (key, 0))
                    .chain((4..10).map(|key| (key, -1)))
                    .collect(),
                _ => BTreeMap::new(),
            },
            entity_id: None,
            horse_columns: None,
            mount_kind: None,
            trades: None,
            anvil_name: None,
            anvil_hover_name: None,
            repair_material_count: 0,
            only_renaming: false,
            active_offer: None,
        }
    }
    pub fn known(&self) -> bool {
        self.slots.iter().all(Option::is_some)
    }
    fn player_slot_index(&self, index: usize) -> Option<usize> {
        if self.kind == MenuKind::Inventory {
            return (index < self.slots.len()).then_some(index);
        }
        if self.kind == MenuKind::Lectern {
            return None;
        }
        let first = if self.kind == MenuKind::Crafter {
            9
        } else {
            self.slots.len().saturating_sub(36)
        };
        (first + 36 <= self.slots.len() && (first..first + 36).contains(&index))
            .then(|| 9 + index - first)
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UiContext {
    #[serde(with = "decimal_u64")]
    pub screen_instance: u64,
    #[serde(with = "decimal_u64")]
    pub revision: u64,
    #[serde(with = "decimal_u64")]
    pub ui_revision: u64,
    pub container_id: i32,
    pub state_id: i32,
}
pub struct PacketCommand {
    pub id: i32,
    pub payload: Writer,
}
impl PacketCommand {
    pub fn new(id: i32, payload: Writer) -> Self {
        Self { id, payload }
    }
}
#[derive(Clone, Debug)]
pub struct InventoryStore {
    pub player: Menu,
    pub active: Option<Menu>,
    pub cursor: Option<Stack>,
    pub selected_slot: u8,
    pub game_mode: u8,
    pub revision: u64,
    pub screen_instance: u64,
    pub screen_open: bool,
    pub sign_editor: Option<([i32; 3], bool)>,
    pub recipes: BTreeMap<i32, Recipe>,
    pub recipe_book_settings: [(bool, bool); 4],
    pub recipe_property_sets: BTreeMap<String, Vec<u32>>,
    pub stonecutter_recipes: Vec<StonecutterRecipe>,
    pub ghost_recipe: Option<(i32, Value)>,
    pub ghost_recipe_stacks: Vec<DisplayStack>,
    pub experience_level: i32,
    received_player: Menu,
    received_active: Option<Menu>,
    received_cursor: Option<Stack>,
    drag: actions::QuickCraft,
    enabled_features: Arc<BTreeSet<String>>,
    creative_screen: bool,
    creative_inventory_tab: bool,
    player_slot_identities: [u64; 48],
    player_auxiliary: [Option<Stack>; 2],
    received_auxiliary: [Option<Stack>; 2],
    enchantment_seed: Option<i32>,
}
impl Default for InventoryStore {
    fn default() -> Self {
        Self {
            player: Menu::new(
                0,
                MenuKind::Inventory,
                json!({"translate":"container.inventory"}),
            ),
            active: None,
            cursor: None,
            selected_slot: 0,
            game_mode: 0,
            revision: 1,
            screen_instance: 1,
            screen_open: false,
            sign_editor: None,
            recipes: BTreeMap::new(),
            recipe_book_settings: [(false, false); 4],
            recipe_property_sets: BTreeMap::new(),
            stonecutter_recipes: Vec::new(),
            ghost_recipe: None,
            ghost_recipe_stacks: Vec::new(),
            experience_level: 0,
            received_player: Menu::new(
                0,
                MenuKind::Inventory,
                json!({"translate":"container.inventory"}),
            ),
            received_active: None,
            received_cursor: None,
            drag: actions::QuickCraft::default(),
            enabled_features: Arc::new(BTreeSet::new()),
            creative_screen: false,
            creative_inventory_tab: false,
            player_slot_identities: [0; 48],
            player_auxiliary: [Some(Stack::default()), Some(Stack::default())],
            received_auxiliary: [None, None],
            enchantment_seed: None,
        }
    }
}
impl InventoryStore {
    pub fn set_source_enchantment_seed(&mut self, seed: i32) {
        self.enchantment_seed = Some(seed);
        if let Some(menu) = self
            .active
            .as_mut()
            .filter(|menu| menu.kind == MenuKind::Enchantment)
        {
            menu.data.entry(3).or_insert(seed);
        }
    }
    pub fn set_enabled_features(&mut self, features: Arc<BTreeSet<String>>) {
        self.enabled_features = features;
    }
    pub fn set_creative_screen(&mut self, open: bool) {
        self.creative_screen = open;
    }
    pub fn set_creative_inventory_tab(&mut self, inventory_tab: bool) {
        self.creative_inventory_tab = inventory_tab;
    }
    pub fn menu(&self) -> &Menu {
        self.active.as_ref().unwrap_or(&self.player)
    }
    pub fn received_menu(&self) -> &Menu {
        self.received_active
            .as_ref()
            .unwrap_or(&self.received_player)
    }
    pub fn received_cursor(&self) -> Option<&Stack> {
        self.received_cursor.as_ref()
    }
    pub fn received_player_slot(&self, slot: u16) -> Result<Option<&Stack>> {
        match slot {
            0..=45 => Ok(self.received_player.slots[usize::from(slot)].as_ref()),
            46..=47 => Ok(self.received_auxiliary[usize::from(slot - 46)].as_ref()),
            _ => Err("player_slot_outside_0_to_47".into()),
        }
    }
    pub fn recipe_properties_contains(&self, name: &str, item_id: u32) -> bool {
        self.recipe_property_sets
            .get(name)
            .is_some_and(|items| items.contains(&item_id))
    }
    pub fn context(&self) -> UiContext {
        let m = self.menu();
        UiContext {
            screen_instance: self.screen_instance,
            revision: self.revision,
            ui_revision: 0,
            container_id: m.id,
            state_id: m.state_id,
        }
    }
    pub fn validate(&self, c: &UiContext) -> Result<()> {
        if c.screen_instance != self.screen_instance {
            return Err("stale_screen".into());
        }
        if c.revision != self.revision {
            return Err("stale_ui_revision".into());
        }
        if c.container_id != self.menu().id {
            return Err("stale_container".into());
        }
        if c.state_id != self.menu().state_id {
            return Err("stale_container_state".into());
        }
        Ok(())
    }
    pub fn open_inventory(&mut self) -> Result<()> {
        if self.active.is_some() {
            return Err("container_already_open".into());
        }
        self.screen_open = true;
        self.screen_instance += 1;
        self.revision += 1;
        Ok(())
    }
    pub fn close(&mut self) {
        self.active = None;
        self.received_active = None;
        self.drag = actions::QuickCraft::default();
        self.ghost_recipe = None;
        self.ghost_recipe_stacks.clear();
        self.sign_editor = None;
        self.screen_open = false;
        self.screen_instance += 1;
        self.revision += 1;
    }
    pub fn reset(&mut self) {
        let instance = self.screen_instance + 1;
        *self = Self::default();
        self.screen_instance = instance;
    }
    pub fn observation(&self) -> Value {
        json!({"origin":"server_observed","revision":self.revision.to_string(),"screen_instance":self.screen_instance.to_string(),"screen_open":self.screen_open,"container":self.received_active.as_ref().unwrap_or(&self.received_player),"player":self.received_player,"equipment":{"body":self.received_auxiliary[0],"saddle":self.received_auxiliary[1]},"cursor":self.received_cursor,"selected_slot":self.selected_slot,"game_mode":self.game_mode})
    }
    pub fn apply(&mut self, id: i32, r: &mut Reader<'_>, content: &Content) -> Result<bool> {
        if self.apply_menu_data(id, r, content)? {
            self.revision += 1;
            return Ok(true);
        }
        match id {
            17 => {
                let container = r.var()?;
                r.done()?;
                if self.menu().id == container {
                    self.close();
                }
                return Ok(true);
            }
            18 => {
                let container = r.var()?;
                let state = r.var()?;
                let n = r.count(256)?;
                let mut slots = Vec::with_capacity(n);
                for _ in 0..n {
                    slots.push(Some(read_stack(r, content)?));
                }
                let cursor = read_stack(r, content)?;
                r.done()?;
                let target = if container == 0 {
                    Some(&mut self.player)
                } else {
                    self.active.as_mut().filter(|m| m.id == container)
                };
                if let Some(m) = target {
                    if m.slots.len() != n {
                        return Err(format!(
                            "menu slot count {n} differs from verified layout {}",
                            m.slots.len()
                        ));
                    }
                    m.slots = slots;
                    let observed = if container == 0 {
                        Some(&mut self.received_player)
                    } else {
                        self.received_active.as_mut().filter(|m| m.id == container)
                    };
                    if let Some(observed) = observed {
                        observed.slots = m.slots.clone();
                        observed.state_id = state;
                        observed.refresh_active_offer()?;
                    }
                    self.received_cursor = Some(cursor.clone());
                    m.state_id = state;
                    m.refresh_active_offer()?;
                    self.cursor = Some(cursor);
                    if container == 0 {
                        for slot in 0..self.player.slots.len() {
                            self.player_slot_identities[slot] =
                                self.player_slot_identities[slot].wrapping_add(1);
                            self.sync_active_player_slot(slot);
                        }
                    } else {
                        for index in 0..self.menu().slots.len() {
                            if let Some(slot) = self.menu().player_slot_index(index) {
                                self.player_slot_identities[slot] =
                                    self.player_slot_identities[slot].wrapping_add(1);
                            }
                        }
                        self.sync_player();
                    }
                }
            }
            19 => {
                let container = r.var()?;
                let key = r.i16()?;
                let value = i32::from(r.i16()?);
                r.done()?;
                if self.menu().id == container {
                    let m = self.active.as_mut().unwrap_or(&mut self.player);
                    if !(0..128).contains(&key) {
                        return Err("container data bound".into());
                    }
                    m.data.insert(key, value);
                    self.received_active
                        .as_mut()
                        .unwrap_or(&mut self.received_player)
                        .data
                        .insert(key, value);
                }
            }
            20 => {
                let container = r.var()?;
                let state = r.var()?;
                let slot = r.i16()?;
                let stack = read_stack(r, content)?;
                r.done()?;
                let target = if container == 0 {
                    Some(&mut self.player)
                } else {
                    self.active.as_mut().filter(|m| m.id == container)
                };
                if let Some(m) = target {
                    let s = m
                        .slots
                        .get_mut(usize::try_from(slot).map_err(|_| "negative slot")?)
                        .ok_or("container slot bound")?;
                    *s = Some(stack.clone());
                    let observed = if container == 0 {
                        Some(&mut self.received_player)
                    } else {
                        self.received_active.as_mut().filter(|m| m.id == container)
                    };
                    if let Some(observed) = observed {
                        observed.slots[slot as usize] = Some(stack);
                        observed.state_id = state;
                        observed.refresh_active_offer()?;
                    }
                    m.state_id = state;
                    m.refresh_active_offer()?;
                    let player_slot = m.player_slot_index(slot as usize);
                    if let Some(player_slot) = player_slot {
                        self.player_slot_identities[player_slot] =
                            self.player_slot_identities[player_slot].wrapping_add(1);
                        if container == 0 {
                            self.sync_active_player_slot(player_slot);
                        }
                    }
                    if container != 0 {
                        self.sync_player();
                    }
                }
            }
            60 => {
                let container = r.var()?;
                if container <= 0 {
                    return Err("open screen container ID must be positive".into());
                }
                let kind = MenuKind::from_wire(r.var()?)?;
                let title = r.nbt()?;
                r.done()?;
                let mut menu = Menu::new(container, kind, title);
                if kind == MenuKind::Enchantment {
                    if let Some(seed) = self.enchantment_seed {
                        menu.data.insert(3, seed);
                    }
                }
                let mut received = menu.clone();
                received.data.clear();
                self.active = Some(menu);
                self.received_active = Some(received);
                self.drag = actions::QuickCraft::default();
                self.ghost_recipe = None;
                self.ghost_recipe_stacks.clear();
                self.screen_open = true;
                self.screen_instance += 1;
            }
            61 => {
                let pos = crate::protocol::unpack_pos(r.i64()?);
                let front = r.var()? == 1;
                r.done()?;
                self.sign_editor = Some((pos, front));
                self.screen_open = true;
                self.screen_instance += 1;
            }
            96 => {
                let cursor = read_stack(r, content)?;
                r.done()?;
                self.received_cursor = Some(cursor.clone());
                self.cursor = Some(cursor);
            }
            107 => {
                self.selected_slot = r.count(8)? as u8;
                r.done()?;
            }
            108 => {
                let index = r.count(42)?;
                let stack = read_stack(r, content)?;
                r.done()?;
                let slot = match index {
                    0..=8 => 36 + index,
                    9..=35 => index,
                    36..=39 => 44 - index,
                    40 => 45,
                    41..=42 => index + 5,
                    _ => unreachable!(),
                };
                self.receive_player_slot(slot, stack);
            }
            _ => return Ok(false),
        }
        self.revision += 1;
        Ok(true)
    }
    fn receive_player_slot(&mut self, slot: usize, stack: Stack) {
        if slot < 46 {
            self.player.slots[slot] = Some(stack.clone());
            self.received_player.slots[slot] = Some(stack);
        } else {
            self.player_auxiliary[slot - 46] = Some(stack.clone());
            self.received_auxiliary[slot - 46] = Some(stack);
        }
        self.player_slot_identities[slot] = self.player_slot_identities[slot].wrapping_add(1);
        self.sync_active_player_slot(slot);
    }
    fn sync_player(&mut self) {
        fn sync(menu: Option<&Menu>, player: &mut Menu) {
            let Some(menu) = menu.filter(|m| m.kind != MenuKind::Lectern) else {
                return;
            };
            let first = if menu.kind == MenuKind::Crafter {
                9
            } else {
                menu.slots.len().saturating_sub(36)
            };
            if first + 36 <= menu.slots.len() {
                for i in 0..36 {
                    player.slots[9 + i] = menu.slots[first + i].clone();
                }
            }
        }
        sync(self.active.as_ref(), &mut self.player);
        sync(self.received_active.as_ref(), &mut self.received_player);
    }
    fn sync_active_player_slot(&mut self, slot: usize) {
        if !(9..45).contains(&slot) {
            return;
        }
        for (menu, player) in [
            (&mut self.active, &self.player),
            (&mut self.received_active, &self.received_player),
        ] {
            if let Some(menu) = menu.as_mut().filter(|m| m.kind != MenuKind::Lectern) {
                let first = if menu.kind == MenuKind::Crafter {
                    9
                } else {
                    menu.slots.len().saturating_sub(36)
                };
                if first + 36 <= menu.slots.len() {
                    menu.slots[first + slot - 9] = player.slots[slot].clone();
                }
            }
        }
    }
}

/// Target ResolvableProfile: Either<GameProfile, Partial>, then PlayerSkin.Patch.
/// Returns the persistent component shape, retaining UUID as its four signed ints.
pub fn read_profile(r: &mut Reader<'_>) -> Result<Value> {
    let resolved = r.bool()?;
    let mut profile = serde_json::Map::new();
    if resolved {
        profile.insert("id".into(), profile_uuid(r)?);
        profile.insert("name".into(), json!(r.string(16)?));
    } else {
        if r.bool()? {
            profile.insert("name".into(), json!(r.string(16)?));
        }
        if r.bool()? {
            profile.insert("id".into(), profile_uuid(r)?);
        }
    }
    let n = r.count(16)?;
    let mut properties = Vec::with_capacity(n);
    for _ in 0..n {
        let mut property = serde_json::Map::new();
        property.insert("name".into(), json!(r.string(64)?));
        property.insert("value".into(), json!(r.string(32767)?));
        if r.bool()? {
            property.insert("signature".into(), json!(r.string(1024)?));
        }
        properties.push(Value::Object(property));
    }
    if !properties.is_empty() {
        profile.insert("properties".into(), Value::Array(properties));
    }
    for field in ["texture", "cape", "elytra"] {
        if r.bool()? {
            profile.insert(field.into(), json!(r.string(32767)?));
        }
    }
    if r.bool()? {
        profile.insert(
            "model".into(),
            json!(if r.bool()? { "slim" } else { "wide" }),
        );
    }
    Ok(Value::Object(profile))
}
fn profile_uuid(r: &mut Reader<'_>) -> Result<Value> {
    Ok(json!([r.i32()?, r.i32()?, r.i32()?, r.i32()?]))
}

mod decimal_u64 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(value: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&value.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let text = String::deserialize(d)?;
        if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(serde::de::Error::custom("expected decimal u64 string"));
        }
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Ordinary creative catalog stacks have an empty patch and the item's verified defaults.
pub fn creative_stack(content: &Content, id: u32, count: i32) -> Result<Stack> {
    let mut wire = Writer::default();
    wire.var(count);
    wire.var(i32::try_from(id).map_err(|_| "creative item ID")?);
    wire.var(0);
    wire.var(0);
    let mut reader = Reader::new(&wire.0);
    let stack = read_stack(&mut reader, content)?;
    reader.done()?;
    if stack.empty() || count > stack.max_count()? {
        return Err("creative stack count exceeds effective maximum".into());
    }
    Ok(stack)
}
pub fn creative_stack_with_components(
    content: &Content,
    id: u32,
    count: i32,
    components: &BTreeMap<String, Value>,
) -> Result<Stack> {
    stack_with_components(content, &creative_stack(content, id, count)?, components)
}
pub fn stack_with_components(
    content: &Content,
    stack: &Stack,
    components: &BTreeMap<String, Value>,
) -> Result<Stack> {
    persistent::with_components(content, stack, components)
}
/// Canonical persistent templates use the same preset adapter as creative stacks.
/// Received nested templates should use their retained Stack, avoiding re-encoding.
pub fn template_stack(template: &Value, content: &Content) -> Result<Stack> {
    let name = template
        .get("id")
        .and_then(Value::as_str)
        .ok_or("template item ID")?;
    let id = content
        .item_names
        .iter()
        .find_map(|(id, n)| (n == name).then_some(*id))
        .ok_or("template item holder")?;
    let count = template
        .get("count")
        .map(|v| v.as_i64().ok_or("template count"))
        .transpose()?
        .unwrap_or(1);
    if !(1..=99).contains(&count) {
        return Err("template count bound".into());
    }
    let mut stack = creative_stack(content, id, 1)?;
    if let Some(components) = template.get("components") {
        let overrides = components
            .as_object()
            .ok_or("template component patch")?
            .iter()
            .map(|(n, v)| (n.clone(), v.clone()))
            .collect();
        stack = stack_with_components(content, &stack, &overrides)?;
    }
    stack.count = count as i32;
    Ok(stack)
}
pub fn bundle_item(stack: &Stack, index: usize, _content: &Content) -> Result<Stack> {
    stack
        .patch
        .added
        .values()
        .find(|value| value.name == "minecraft:bundle_contents")
        .and_then(|value| value.nested_stacks.get(index))
        .cloned()
        .ok_or_else(|| "bundle selected item outside retained contents".into())
}
pub fn read_stack(r: &mut Reader<'_>, content: &Content) -> Result<Stack> {
    read_stack_depth(r, content, 0)
}
/// ItemStackTemplate uses item, count, patch; ordinary ItemStack uses count, item, patch.
/// Both feed the same component parser; the retained representation is normalized
/// to ordinary ItemStack framing for the existing `Stack::write` consumer.
pub fn read_template(r: &mut Reader<'_>, content: &Content) -> Result<Stack> {
    read_template_depth(r, content, 0)
}
fn read_template_depth(r: &mut Reader<'_>, content: &Content, depth: u8) -> Result<Stack> {
    if depth > 32 {
        return Err("nested stack depth bound".into());
    }
    let id = r.count(1_000_000)? as u32;
    let count = r.count(99)? as i32;
    if count == 0
        || content
            .item_names
            .get(&id)
            .is_some_and(|id| id == "minecraft:air")
    {
        return Err("item stack template must be non-empty".into());
    }
    let patch_start = r.offset;
    let (components, patch) = read_stack_patch(r, content, id, depth)?;
    let mut wire = Writer::default();
    wire.var(count);
    wire.var(id as i32);
    wire.0.extend_from_slice(&r.bytes[patch_start..r.offset]);
    Ok(Stack {
        id,
        count,
        components,
        wire: wire.0.into(),
        patch,
        bundle_selected: -1,
    })
}
fn read_stack_depth(r: &mut Reader<'_>, content: &Content, depth: u8) -> Result<Stack> {
    if depth > 32 {
        return Err("nested stack depth bound".into());
    }
    let start = r.offset;
    let count = r.var()?;
    if count <= 0 {
        return Ok(Stack::default());
    }
    let id = r.count(1_000_000)? as u32;
    let (components, patch) = read_stack_patch(r, content, id, depth)?;
    if content
        .item_names
        .get(&id)
        .is_some_and(|name| name == "minecraft:air")
    {
        return Ok(Stack::default());
    }
    Ok(Stack {
        id,
        count,
        components,
        wire: Arc::from(&r.bytes[start..r.offset]),
        patch,
        bundle_selected: -1,
    })
}
fn read_stack_patch(
    r: &mut Reader<'_>,
    content: &Content,
    id: u32,
    depth: u8,
) -> Result<(Arc<BTreeMap<String, Value>>, Arc<ComponentPatch>)> {
    if !content.item_names.contains_key(&id) {
        return Err(format!("unknown item registry ID {id}"));
    }
    let patch_start = r.offset;
    let added = r.count(256)?;
    let removed = r.count(256)?;
    let mut components = Arc::clone(content.item_components(id)?);
    if added == 0 && removed == 0 {
        return Ok((components, empty_patch()));
    }
    let mut patch = ComponentPatch::default();
    for _ in 0..added {
        let ty = r.var()?;
        let name = content
            .components
            .get(&ty)
            .ok_or("unknown item component registry ID")?;
        let start = r.offset;
        let adventure = if matches!(
            name.as_str(),
            "minecraft:can_break" | "minecraft:can_place_on"
        ) {
            Some(Arc::new(predicates::AdventurePredicate::read(
                r, content, depth,
            )?))
        } else {
            None
        };
        let (value, nested_stacks) = match &adventure {
            Some(predicate) => (predicate.persistent()?, empty_stacks()),
            None => components::read_retained(r, content, name, depth)?,
        };
        Arc::make_mut(&mut components).insert(name.clone(), value.json()?);
        if patch
            .added
            .insert(
                ty,
                ComponentValue {
                    name: name.clone(),
                    hash: (!transient_component(name)).then(|| value.hash()),
                    value,
                    wire_range: start - patch_start..r.offset - patch_start,
                    adventure,
                    nested_stacks,
                },
            )
            .is_some()
        {
            return Err("duplicate item component".into());
        }
    }
    for _ in 0..removed {
        let ty = r.var()?;
        let name = content
            .components
            .get(&ty)
            .ok_or("unknown removed component")?;
        if patch.added.contains_key(&ty) || patch.removed.contains(&ty) {
            return Err("duplicate or contradictory removed component".into());
        }
        Arc::make_mut(&mut components).remove(name);
        patch.removed.push(ty);
    }
    let defaults = content.item_components(id)?;
    patch
        .added
        .retain(|_, value| defaults.get(&value.name) != components.get(&value.name));
    patch.removed.retain(|ty| {
        content
            .components
            .get(ty)
            .is_some_and(|name| defaults.contains_key(name))
    });
    if patch.added.is_empty() && patch.removed.is_empty() {
        Ok((Arc::clone(defaults), empty_patch()))
    } else {
        Ok((components, Arc::new(patch)))
    }
}

impl Stack {
    /// Persistent ItemStackTemplate encoding contains the patch, never effective
    /// default components. The optional count/components fields omit their defaults.
    pub fn persistent_template(&self, content: &Content) -> Result<Tag> {
        if self.empty() {
            return Err("empty item stack template".into());
        }
        let item = content
            .item_names
            .get(&self.id)
            .ok_or("unknown template item")?;
        let mut fields = vec![(Tag::string("id"), Tag::string(item))];
        if self.count != 1 {
            fields.push((Tag::string("count"), Tag::Int(self.count)));
        }
        let mut patch = Vec::with_capacity(self.patch.added.len() + self.patch.removed.len());
        for v in self.patch.added.values() {
            if !transient_component(&v.name) {
                patch.push((Tag::string(&v.name), v.value.clone()));
            }
        }
        for id in &self.patch.removed {
            let name = content
                .components
                .get(id)
                .ok_or("unknown removed component")?;
            if !transient_component(name) {
                patch.push((Tag::string(format!("!{name}")), Tag::Map(vec![])));
            }
        }
        if !patch.is_empty() {
            fields.push((Tag::string("components"), Tag::Map(patch)));
        }
        Ok(Tag::Map(fields))
    }
}
fn transient_component(name: &str) -> bool {
    matches!(
        name,
        "minecraft:creative_slot_lock"
            | "minecraft:additional_trade_cost"
            | "minecraft:map_post_processing"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reopening_rejects_previous_slot_handles() {
        let mut s = InventoryStore::default();
        s.open_inventory().unwrap();
        let old = s.context();
        s.close();
        s.open_inventory().unwrap();
        assert_eq!(s.validate(&old).unwrap_err(), "stale_screen");
    }
    #[test]
    fn state_and_revision_are_independent_fences() {
        let mut s = InventoryStore::default();
        let mut c = s.context();
        c.state_id += 1;
        assert_eq!(s.validate(&c).unwrap_err(), "stale_container_state");
        c = s.context();
        s.revision += 1;
        assert_eq!(s.validate(&c).unwrap_err(), "stale_ui_revision");
    }
    #[test]
    fn crafter_result_is_not_player_hotbar() {
        let mut s = InventoryStore::default();
        let mut m = Menu::new(7, MenuKind::Crafter, Value::Null);
        m.slots[44] = Some(Stack {
            id: 2,
            count: 3,
            ..Stack::default()
        });
        m.slots[45] = Some(Stack {
            id: 9,
            count: 8,
            ..Stack::default()
        });
        s.active = Some(m);
        s.sync_player();
        assert_eq!(s.player.slots[44].as_ref().unwrap().count, 3);
        assert_eq!(s.player.slots[45], None);
    }
    #[test]
    fn component_difference_prevents_merging() {
        let mut a = Stack {
            id: 1,
            count: 1,
            ..Stack::default()
        };
        let b = a.clone();
        Arc::make_mut(&mut a.components).insert("minecraft:damage".into(), json!(3));
        assert!(!a.same_item(&b));
    }

    #[test]
    fn set_held_slot_uses_26_3_play_id() {
        let mut store = InventoryStore::default();
        assert!(store.apply(107, &mut Reader::new(&[3]), &Content::test_cube()).unwrap());
        assert_eq!(store.selected_slot, 3);
        assert!(!store.apply(105, &mut Reader::new(&[3]), &Content::test_cube()).unwrap());
    }
}
