use crate::protocol::packet_ids::play::serverbound as sb;
use super::{InventoryStore, Menu, MenuKind, PacketCommand, Stack, UiContext};
use crate::{
    protocol::{Result, Writer, content::Content},
    ui::{ClickMode, UiIntent},
};
use std::collections::{BTreeMap, BTreeSet};
#[path = "creative.rs"]
mod creative;
#[path = "policies.rs"]
mod policies;
#[path = "results.rs"]
mod results;
#[derive(Clone, Debug, Default)]
pub(super) struct QuickCraft {
    pub status: u8,
    pub kind: u8,
    pub slots: BTreeSet<usize>,
}
#[derive(Clone, Debug)]
pub struct MenuButton {
    pub id: i32,
    pub enabled: bool,
    pub deny_reason: Option<&'static str>,
}
fn context(intent: &UiIntent) -> Option<&UiContext> {
    match intent {
        UiIntent::Close { context }
        | UiIntent::ClickSlot { context, .. }
        | UiIntent::BundleSelect { context, .. }
        | UiIntent::MenuButton { context, .. }
        | UiIntent::Beacon { context, .. }
        | UiIntent::SlotToggle { context, .. }
        | UiIntent::Rename { context, .. }
        | UiIntent::PlaceRecipe { context, .. }
        | UiIntent::RecipeBook { context, .. }
        | UiIntent::CreativeSlot { context, .. } => Some(context),
        _ => None,
    }
}
fn hashed(stack: &Stack, w: &mut Writer) -> Result<()> {
    w.byte(u8::from(!stack.empty()));
    if stack.empty() {
        return Ok(());
    }
    w.var(stack.id as i32);
    w.var(stack.count);
    w.var(stack.patch.added.len() as i32);
    for (id, hash) in stack.component_hashes()? {
        w.var(id);
        w.i32(hash as i32);
    }
    w.var(stack.patch.removed.len() as i32);
    for id in &stack.patch.removed {
        w.var(*id);
    }
    Ok(())
}
fn normalized(mut stack: Stack) -> Stack {
    if stack.count <= 0 {
        stack = Stack::default();
    }
    stack
}
fn content_is_bundle(content: &Content, stack: &Stack) -> Result<bool> {
    Ok(content
        .registry_tag("minecraft:item", "minecraft:bundles")?
        .contains(&(stack.id as i32)))
}
struct Prediction<'a> {
    source: &'a InventoryStore,
    content: &'a Content,
    menu: Menu,
    player: Menu,
    cursor: Stack,
    drag: QuickCraft,
    experience_level: i32,
    player_slot_identities: [u64; 48],
}
impl InventoryStore {
    pub fn execute_bundle_select(
        &mut self,
        context: &UiContext,
        slot: i16,
        selected: i32,
        content: &Content,
    ) -> Result<Vec<PacketCommand>> {
        self.execute_gesture(
            &[UiIntent::BundleSelect {
                context: context.clone(),
                slot,
                index: selected,
            }],
            content,
        )
    }
    pub fn player_slot(&self, slot: u16) -> Result<Option<&Stack>> {
        let slot = usize::from(slot);
        if slot < 46 {
            self.player.slots.get(slot)
        } else {
            self.player_auxiliary.get(slot - 46)
        }
        .map(Option::as_ref)
        .ok_or("player slot bound".into())
    }
    pub fn player_slot_identity(&self, slot: u16) -> Result<u64> {
        self.player_slot(slot)?.ok_or("player slot not observed")?;
        Ok(self.player_slot_identities[slot as usize])
    }
    pub fn source_contains_item(&self, search: &Stack) -> Result<bool> {
        if search.empty() {
            return Ok(false);
        }
        for slot in (36..45).chain(9..36).chain([8, 7, 6, 5, 45, 46, 47]) {
            let stack = self.player_slot(slot)?.ok_or("player slot not observed")?;
            if !stack.empty() && stack.same_item(search) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    pub fn apply_local_equipment(&mut self, slot: u8, stack: Option<Stack>) -> Result<()> {
        // ClientboundSetEquipmentPacket uses enum ordinal, not STREAM_CODEC id.
        let slot = match slot {
            0 => 36 + usize::from(self.selected_slot),
            1 => 45,
            2..=5 => 10 - usize::from(slot),
            6..=7 => 40 + usize::from(slot),
            _ => return Err("player equipment slot bound".into()),
        };
        self.receive_player_slot(slot, stack.unwrap_or_default());
        self.revision += 1;
        Ok(())
    }
    pub fn predict_use_slot(
        &mut self,
        slot: u16,
        stack: Option<Stack>,
        replacement: bool,
    ) -> Result<()> {
        let slot_index = usize::from(slot);
        let target = if slot_index < 46 {
            self.player.slots.get_mut(slot_index)
        } else {
            self.player_auxiliary.get_mut(slot_index - 46)
        }
        .ok_or("player slot bound")?;
        if target.is_none() {
            return Err("player slot not observed".into());
        }
        *target = Some(stack.unwrap_or_default());
        if replacement {
            self.player_slot_identities[slot as usize] =
                self.player_slot_identities[slot as usize].wrapping_add(1);
        }
        if let Some(menu) = self.active.as_mut().filter(|m| m.kind != MenuKind::Lectern) {
            if (9..45).contains(&slot) {
                let first = if menu.kind == MenuKind::Crafter {
                    9
                } else {
                    menu.slots.len().saturating_sub(36)
                };
                if first + 36 <= menu.slots.len() {
                    menu.slots[first + slot as usize - 9] =
                        self.player.slots[slot as usize].clone();
                }
            }
        }
        self.revision += 1;
        Ok(())
    }
    pub fn source_add_item(&self, stack: &Stack) -> Result<(Vec<(u16, Stack, bool)>, Stack)> {
        let mut slots = self.player.slots.clone();
        let mut left = stack.clone();
        let (_, changes) = policies::inventory_add(
            &mut slots,
            self.selected_slot,
            self.game_mode == 1,
            &mut left,
        )?;
        let changes = changes
            .into_iter()
            .enumerate()
            .filter(|(_, change)| *change != policies::SlotChange::Unchanged)
            .map(|(slot, change)| {
                Ok((
                    slot as u16,
                    slots[slot].take().ok_or("player slot not observed")?,
                    change == policies::SlotChange::Replaced,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        Ok((changes, normalized(left)))
    }
    pub fn loom_patterns(&self, content: &Content) -> Result<Vec<String>> {
        if self.menu().kind != MenuKind::Loom {
            return Err("loom unavailable".into());
        }
        let menu = self.menu();
        let banner = menu.slots[0]
            .as_ref()
            .ok_or("loom banner slot not observed")?;
        let dye = menu.slots[1].as_ref().ok_or("loom dye slot not observed")?;
        if banner.empty() || dye.empty() {
            return Ok(Vec::new());
        }
        let pattern = menu.slots[2]
            .as_ref()
            .ok_or("loom pattern slot not observed")?;
        Prediction::loom_set(content, pattern)
    }
    pub fn menu_buttons(&self, content: &Content) -> Result<Vec<MenuButton>> {
        let menu = self.menu();
        let data = |id| menu.data.get(&id).copied();
        let count = match menu.kind {
            MenuKind::Enchantment => 3,
            MenuKind::Lectern => 3,
            MenuKind::Loom => self.loom_patterns(content)?.len(),
            MenuKind::Stonecutter => menu
                .slots
                .first()
                .and_then(Option::as_ref)
                .map(|input| {
                    self.stonecutter_recipes
                        .iter()
                        .filter(|r| r.ingredient_items.contains(&input.id))
                        .count()
                })
                .unwrap_or(0),
            MenuKind::Merchant => menu.trades.as_ref().map(|t| t.offers.len()).unwrap_or(0),
            _ => 0,
        };
        let mut buttons = Vec::with_capacity(count);
        for index in 0..count {
            let id = if menu.kind == MenuKind::Lectern {
                index as i32 + 1
            } else {
                index as i32
            };
            let enabled = match menu.kind {
                MenuKind::Enchantment => {
                    let required = index as i32 + 1;
                    data(index as i16).is_some_and(|cost| {
                        cost > 0
                            && (self.game_mode == 1
                                || (self.experience_level >= cost as i32
                                    && self.experience_level >= required
                                    && menu
                                        .slots
                                        .get(1)
                                        .and_then(Option::as_ref)
                                        .is_some_and(|s| s.count >= required)))
                    })
                }
                MenuKind::Lectern => {
                    menu.slots
                        .first()
                        .and_then(Option::as_ref)
                        .is_some_and(|s| !s.empty())
                        && (id != 3 || self.game_mode != 3)
                }
                MenuKind::Loom => {
                    menu.slots
                        .first()
                        .and_then(Option::as_ref)
                        .is_some_and(|s| {
                            !s.empty()
                                && s.component("minecraft:banner_patterns")
                                    .and_then(serde_json::Value::as_array)
                                    .map_or(0, Vec::len)
                                    < 6
                        })
                        && menu
                            .slots
                            .get(1)
                            .and_then(Option::as_ref)
                            .is_some_and(|s| !s.empty())
                }
                MenuKind::Stonecutter => {
                    data(0) != Some(index as i32)
                        && menu
                            .slots
                            .first()
                            .and_then(Option::as_ref)
                            .is_some_and(|s| !s.empty())
                }
                MenuKind::Merchant => true,
                _ => false,
            };
            buttons.push(MenuButton {
                id,
                enabled,
                deny_reason: (!enabled).then_some("source_menu_button_disabled"),
            });
        }
        Ok(buttons)
    }
    pub fn recipe_craftable(&self, id: i32) -> Result<Option<bool>> {
        let recipe = self.recipes.get(&id).ok_or("unknown recipe display")?;
        let Some(ingredients) = &recipe.ingredient_items else {
            return Ok(None);
        };
        let mut counts = BTreeMap::<u32, usize>::new();
        let account =
            |stack: &Stack, simple: bool, counts: &mut BTreeMap<u32, usize>| -> Result<()> {
                if stack.empty() {
                    return Ok(());
                }
                let damaged = stack
                    .component("minecraft:max_damage")
                    .and_then(serde_json::Value::as_i64)
                    .is_some_and(|max| {
                        stack.component("minecraft:unbreakable").is_none()
                            && stack
                                .component("minecraft:damage")
                                .and_then(serde_json::Value::as_i64)
                                .unwrap_or(0)
                                .clamp(0, max)
                                > 0
                    });
                let enchanted = stack
                    .component("minecraft:enchantments")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|v| !v.is_empty());
                if !simple
                    || !damaged && !enchanted && stack.component("minecraft:custom_name").is_none()
                {
                    *counts.entry(stack.id).or_default() +=
                        stack.count.min(stack.max_count()?) as usize;
                }
                Ok(())
            };
        for slot in self.player.slots.iter().skip(9).take(36) {
            let Some(stack) = slot else {
                return Ok(None);
            };
            account(stack, true, &mut counts)?;
        }
        let furnace = matches!(
            self.menu().kind,
            MenuKind::Furnace | MenuKind::Smoker | MenuKind::BlastFurnace
        );
        let inputs = match self.menu().kind {
            MenuKind::Inventory => 1..5,
            MenuKind::Crafting => 1..10,
            MenuKind::Furnace | MenuKind::Smoker | MenuKind::BlastFurnace => 0..3,
            _ => 0..0,
        };
        for index in inputs {
            let Some(stack) = self.menu().slots.get(index).and_then(Option::as_ref) else {
                return Ok(None);
            };
            account(stack, !furnace, &mut counts)?;
        }
        if ingredients.len() > counts.values().sum() {
            return Ok(Some(false));
        }
        // Capacitated bipartite matching. An overlapping ingredient can displace
        // a previous assignment along an augmenting path, without exponential
        // enumeration of alternatives or mutation of the retained recipe.
        fn augment(
            index: usize,
            ingredients: &[Vec<u32>],
            counts: &BTreeMap<u32, usize>,
            assigned: &mut [Option<u32>],
            visited: &mut BTreeSet<u32>,
        ) -> bool {
            for id in &ingredients[index] {
                let capacity = counts.get(id).copied().unwrap_or(0);
                if capacity == 0 || !visited.insert(*id) {
                    continue;
                }
                if assigned.iter().filter(|item| **item == Some(*id)).count() < capacity {
                    assigned[index] = Some(*id);
                    return true;
                }
                for other in 0..assigned.len() {
                    if assigned[other] == Some(*id)
                        && augment(other, ingredients, counts, assigned, visited)
                    {
                        assigned[index] = Some(*id);
                        return true;
                    }
                }
            }
            false
        }
        let mut assigned = vec![None; ingredients.len()];
        for index in 0..ingredients.len() {
            if !augment(
                index,
                ingredients,
                &counts,
                &mut assigned,
                &mut BTreeSet::new(),
            ) {
                return Ok(Some(false));
            }
        }
        Ok(Some(true))
    }
    pub fn execute_creative_stack(
        &mut self,
        context: &UiContext,
        slot: i16,
        stack: Stack,
        content: &Content,
    ) -> Result<Vec<PacketCommand>> {
        self.validate(context)?;
        if slot != -1 && !(1..=45).contains(&slot) {
            return Err("creative slot unavailable".into());
        }
        self.validate_creative_item(&stack, content)?;
        let mut w = Writer::default();
        w.0.extend(slot.to_be_bytes());
        stack.write(&mut w)?;
        if slot >= 0 {
            self.player.slots[slot as usize] = Some(stack);
            self.player_slot_identities[slot as usize] =
                self.player_slot_identities[slot as usize].wrapping_add(1);
        }
        self.revision += 1;
        Ok(vec![PacketCommand::new(sb::SET_CREATIVE_MODE_SLOT, w)])
    }
    pub fn execute(&mut self, intent: &UiIntent, content: &Content) -> Result<Vec<PacketCommand>> {
        self.execute_gesture(std::slice::from_ref(intent), content)
    }
    pub fn execute_rename(
        &mut self,
        context: &UiContext,
        text: &str,
        hover_name: &str,
        content: &Content,
    ) -> Result<Vec<PacketCommand>> {
        self.execute_with_hover_name(
            &[UiIntent::Rename {
                context: context.clone(),
                text: text.to_owned(),
            }],
            content,
            Some(hover_name),
        )
    }
    pub fn execute_gesture(
        &mut self,
        intents: &[UiIntent],
        content: &Content,
    ) -> Result<Vec<PacketCommand>> {
        self.execute_with_hover_name(intents, content, None)
    }
    fn execute_with_hover_name(
        &mut self,
        intents: &[UiIntent],
        content: &Content,
        hover_name: Option<&str>,
    ) -> Result<Vec<PacketCommand>> {
        if intents.is_empty() || intents.len() > 256 {
            return Err("inventory gesture bound".into());
        }
        let shared = intents.iter().find_map(context);
        for intent in intents {
            if let Some(context) = context(intent) {
                self.validate(context)?;
                if shared.is_some_and(|shared| shared != context) {
                    return Err("mixed_inventory_gesture_context".into());
                }
            } else if !matches!(intent, UiIntent::OpenInventory) {
                return Err("not an inventory intent".into());
            }
        }
        if intents.len() == 1 {
            match &intents[0] {
                UiIntent::OpenInventory => {
                    self.open_inventory()?;
                    return Ok(vec![]);
                }
                UiIntent::Close { .. } => {
                    let mut w = Writer::default();
                    w.var(self.menu().id);
                    self.close();
                    return Ok(vec![PacketCommand::new(sb::CONTAINER_CLOSE, w)]);
                }
                _ => {}
            }
        }
        // Source gameMode handles these player-menu actions without opening a screen.
        // Keep the same context validation, prediction and packet construction below.
        if !self.screen_open
            && (self.menu().id != 0
                || !intents.iter().all(|intent| {
                    matches!(
                        intent,
                        UiIntent::ClickSlot {
                            mode: ClickMode::Swap | ClickMode::QuickMove,
                            ..
                        } | UiIntent::MenuButton { .. }
                    )
                }))
        {
            return Err("container_not_open".into());
        }
        let mut prediction = Prediction {
            source: self,
            content,
            menu: self.menu().clone(),
            player: self.player.clone(),
            cursor: self.cursor.clone().ok_or("cursor not observed")?,
            drag: self.drag.clone(),
            experience_level: self.experience_level,
            player_slot_identities: self.player_slot_identities,
        };
        if let Some(name) = hover_name {
            prediction.menu.anvil_hover_name = Some(name.to_owned());
        }
        let mut commands = Vec::new();
        for intent in intents {
            prediction.execute(intent, &mut commands)?;
        }
        let Prediction {
            menu,
            player,
            cursor,
            drag,
            experience_level,
            player_slot_identities,
            ..
        } = prediction;
        self.player = player;
        if self.active.is_some() {
            self.active = Some(menu);
        } else {
            self.player = menu;
        }
        self.cursor = Some(normalized(cursor));
        self.drag = drag;
        self.sync_player();
        self.experience_level = experience_level;
        self.player_slot_identities = player_slot_identities;
        self.revision += 1;
        Ok(commands)
    }
}
impl Prediction<'_> {
    fn slot(&self, index: usize) -> Result<&Stack> {
        self.menu
            .slots
            .get(index)
            .and_then(Option::as_ref)
            .ok_or("container slot not observed".into())
    }
    fn set(&mut self, index: usize, stack: Stack, replacement: bool) {
        let stack = normalized(stack);
        if let Some(player_slot) = self.menu.player_slot_index(index) {
            self.player.slots[player_slot] = Some(stack.clone());
            if replacement {
                self.player_slot_identities[player_slot] =
                    self.player_slot_identities[player_slot].wrapping_add(1);
            }
        }
        self.menu.slots[index] = Some(stack);
    }
    fn set_player(&mut self, slot: usize, stack: Stack, replacement: bool) {
        if let Some(index) = self.player_menu_slot(slot) {
            self.set(index, stack, replacement);
        } else {
            self.player.slots[slot] = Some(normalized(stack));
            if replacement {
                self.player_slot_identities[slot] =
                    self.player_slot_identities[slot].wrapping_add(1);
            }
        }
    }
    fn execute(&mut self, intent: &UiIntent, commands: &mut Vec<PacketCommand>) -> Result<()> {
        let mut w = Writer::default();
        let container = self.menu.id;
        match intent {
            UiIntent::BundleSelect {
                slot,
                index: selected,
                ..
            } => {
                let index = usize::try_from(*slot).map_err(|_| "bundle slot bound")?;
                let stack = self.slot(index)?;
                if !content_is_bundle(self.content, stack)?
                    || *selected < -1
                    || *selected >= stack.bundle_visible_items() as i32
                {
                    return Err("bundle selection unavailable".into());
                }
                self.select_bundle(index, *selected)?;
                w.var(*slot as i32);
                w.var(*selected);
                commands.push(PacketCommand::new(sb::BUNDLE_ITEM_SELECTED, w));
            }
            UiIntent::ClickSlot {
                slot, button, mode, ..
            } => {
                if *slot >= 0 && matches!(mode, ClickMode::QuickMove | ClickMode::Swap) {
                    let index = *slot as usize;
                    let stack = self.slot(index)?;
                    if content_is_bundle(self.content, stack)? {
                        self.select_bundle(index, -1)?;
                        let mut selected = Writer::default();
                        selected.var(*slot as i32);
                        selected.var(-1);
                        commands.push(PacketCommand::new(sb::BUNDLE_ITEM_SELECTED, selected));
                    }
                }
                if self.source.creative_screen {
                    return self.creative_click(*slot, *button, *mode, commands);
                }
                let before = self.menu.slots.clone();
                if self.menu.kind == MenuKind::Crafter
                    && (0..9).contains(slot)
                    && self.source.game_mode != 3
                {
                    let index = *slot as usize;
                    let disabled = *self
                        .menu
                        .data
                        .get(slot)
                        .ok_or("crafter slot state unknown")?
                        == 1;
                    let toggle = if *mode == ClickMode::Pickup && self.slot(index)?.empty() {
                        if disabled {
                            Some(true)
                        } else if self.cursor.empty() {
                            Some(false)
                        } else {
                            None
                        }
                    } else if *mode == ClickMode::Swap && disabled && *button < 9 {
                        let hotbar = 9 + 27 + *button as usize;
                        (!self.slot(hotbar)?.empty()).then_some(true)
                    } else {
                        None
                    };
                    if let Some(enabled) = toggle {
                        let mut toggle = Writer::default();
                        toggle.var(*slot as i32);
                        toggle.var(container);
                        toggle.byte(u8::from(enabled));
                        self.menu.data.insert(*slot, i32::from(!enabled));
                        commands.push(PacketCommand::new(sb::CONTAINER_SLOT_STATE_CHANGED, toggle));
                    }
                }
                self.click(*slot, *button, *mode)?;
                self.reconcile_input_changes(&before)?;
                w.var(container);
                w.var(self.menu.state_id);
                w.0.extend(slot.to_be_bytes());
                w.byte(*button);
                w.var(mode.wire());
                let changes = self
                    .menu
                    .slots
                    .iter()
                    .zip(&before)
                    .enumerate()
                    .filter(|(_, (a, b))| a != b)
                    .collect::<Vec<_>>();
                if changes.len() > 128 {
                    return Err("changed slot bound".into());
                }
                w.var(changes.len() as i32);
                for (slot, (stack, _)) in changes {
                    w.0.extend((slot as i16).to_be_bytes());
                    hashed(stack.as_ref().ok_or("changed slot unknown")?, &mut w)?;
                }
                hashed(&self.cursor, &mut w)?;
                commands.push(PacketCommand::new(sb::CONTAINER_CLICK, w));
            }
            UiIntent::MenuButton { button, .. } => {
                // InventoryMenu has no local button behavior. Source still sends
                // handleInventoryButtonClick for container zero; only a received
                // state advance can confirm the canonical tool, never this dispatch.
                if container == 0 && self.menu.kind == MenuKind::Inventory {
                    if *button < 0 {
                        return Err("menu button must be non-negative".into());
                    }
                    w.var(container);
                    w.var(*button);
                    commands.push(PacketCommand::new(sb::CONTAINER_BUTTON_CLICK, w));
                    return Ok(());
                }
                let allowed = self
                    .source
                    .menu_buttons(self.content)?
                    .into_iter()
                    .find(|b| b.id == *button)
                    .ok_or("unknown menu button")?;
                if !allowed.enabled {
                    return Err(allowed.deny_reason.unwrap().into());
                }
                if self.menu.kind == MenuKind::Merchant {
                    self.select_trade(*button as usize)?;
                    w.var(*button);
                    commands.push(PacketCommand::new(sb::SELECT_TRADE, w));
                } else {
                    w.var(container);
                    w.var(*button);
                    commands.push(PacketCommand::new(sb::CONTAINER_BUTTON_CLICK, w));
                    self.predict_button(*button)?;
                }
            }
            UiIntent::SlotToggle { slot, enabled, .. } => {
                if self.menu.kind != MenuKind::Crafter
                    || !(0..9).contains(slot)
                    || self.source.game_mode == 3
                    || !self.slot(*slot as usize)?.empty()
                {
                    return Err("crafter slot toggle unavailable".into());
                }
                w.var(*slot as i32);
                w.var(container);
                w.byte(u8::from(*enabled));
                self.menu.data.insert(*slot, i32::from(!*enabled));
                commands.push(PacketCommand::new(sb::CONTAINER_SLOT_STATE_CHANGED, w));
            }
            UiIntent::Rename { text, .. } => {
                if self.menu.kind != MenuKind::Anvil {
                    return Err("anvil rename unavailable".into());
                }
                let text = text
                    .chars()
                    .filter(|c| *c >= ' ' && *c != '\u{7f}' && *c != '\u{a7}')
                    .collect::<String>();
                if text.encode_utf16().count() > 50 {
                    return Err("anvil rename length".into());
                }
                self.menu.anvil_name = Some(text.clone());
                self.recompute_anvil()?;
                w.string(&text);
                commands.push(PacketCommand::new(sb::RENAME_ITEM, w));
            }
            UiIntent::PlaceRecipe {
                display_id,
                craft_all,
                ..
            } => {
                if self.menu.kind.recipe_book().is_none()
                    || !self.source.recipes.contains_key(display_id)
                {
                    return Err("recipe unavailable".into());
                }
                w.var(container);
                w.var(*display_id);
                w.byte(u8::from(*craft_all));
                commands.push(PacketCommand::new(sb::PLACE_RECIPE, w));
            }
            UiIntent::RecipeBook { open, filter, .. } => {
                w.var(
                    self.menu
                        .kind
                        .recipe_book()
                        .ok_or("recipe book unavailable")?,
                );
                w.byte(u8::from(*open));
                w.byte(u8::from(*filter));
                commands.push(PacketCommand::new(sb::RECIPE_BOOK_CHANGE_SETTINGS, w));
            }
            UiIntent::CreativeSlot {
                slot,
                item_id,
                count,
                ..
            } => {
                if self.source.game_mode != 1
                    || self.menu.kind != MenuKind::Inventory
                    || !(-1..=45).contains(slot)
                    || (*slot >= 0 && *slot < 1)
                {
                    return Err("creative slot unavailable".into());
                }
                let stack = if *count == 0 {
                    Stack::default()
                } else {
                    super::creative_stack(self.content, *item_id, *count)?
                };
                self.source.validate_creative_item(&stack, self.content)?;
                w.0.extend(slot.to_be_bytes());
                stack.write(&mut w)?;
                if *slot >= 0 {
                    self.set(*slot as usize, stack, true);
                }
                commands.push(PacketCommand::new(sb::SET_CREATIVE_MODE_SLOT, w));
            }
            UiIntent::Beacon {
                primary, secondary, ..
            } => {
                if self.menu.kind != MenuKind::Beacon || self.slot(0)?.empty() {
                    return Err("beacon payment unavailable".into());
                }
                let levels = *self.menu.data.get(&0).ok_or("beacon levels unknown")?;
                let effects = [
                    ("minecraft:speed", 1),
                    ("minecraft:haste", 1),
                    ("minecraft:resistance", 2),
                    ("minecraft:jump_boost", 2),
                    ("minecraft:strength", 3),
                    ("minecraft:regeneration", 4),
                ];
                let primary = primary.ok_or("beacon primary required")?;
                let name = self
                    .content
                    .registry_name("minecraft:mob_effect", primary)?;
                if !effects[..5].iter().any(|(n, t)| *n == name && levels >= *t) {
                    return Err("beacon primary unavailable".into());
                }
                if let Some(secondary) = secondary {
                    if levels < 4
                        || (*secondary != primary
                            && self
                                .content
                                .registry_name("minecraft:mob_effect", *secondary)?
                                != "minecraft:regeneration")
                    {
                        return Err("beacon secondary unavailable".into());
                    }
                }
                w.byte(1);
                w.var(primary);
                w.byte(u8::from(secondary.is_some()));
                if let Some(id) = secondary {
                    w.var(*id);
                }
                commands.push(PacketCommand::new(sb::SET_BEACON, w));
                let mut close = Writer::default();
                close.var(container);
                commands.push(PacketCommand::new(sb::CONTAINER_CLOSE, close));
            }
            _ => return Err("not an inventory gesture intent".into()),
        }
        Ok(())
    }
    fn click(&mut self, slot: i16, button: u8, mode: ClickMode) -> Result<()> {
        if self.source.game_mode == 3 {
            return Ok(());
        }
        if mode != ClickMode::QuickCraft && self.drag.status != 0 {
            self.drag = QuickCraft::default();
            return Ok(());
        }
        if mode == ClickMode::QuickCraft {
            return self.quick_craft(slot, button);
        }
        if slot == -999 {
            if mode == ClickMode::Pickup && !self.cursor.empty() {
                if button == 0 {
                    self.cursor = Stack::default();
                } else if button == 1 {
                    self.cursor.count -= 1;
                    self.cursor = normalized(self.cursor.clone());
                } else {
                    return Err("pickup button".into());
                }
            }
            return Ok(());
        }
        let index = usize::try_from(slot).map_err(|_| "negative container slot")?;
        let mut stack = self.slot(index)?.clone();
        let mut took = false;
        let mut replacement = false;
        match mode {
            ClickMode::Pickup => {
                if button > 1 {
                    return Err("pickup button".into());
                }
                if self.bundle_override(index, button)? {
                    return Ok(());
                }
                stack = self.slot(index)?.clone();
                if self.cursor.empty() {
                    if self.may_pickup(index)? {
                        let n = if button == 0 {
                            stack.count
                        } else {
                            (stack.count + 1) / 2
                        };
                        self.cursor = stack.clone();
                        self.cursor.count = n;
                        stack.count -= n;
                        took = n > 0;
                        replacement = took && stack.empty();
                    }
                } else if stack.empty() {
                    if self.may_place(index, &self.cursor)? {
                        let n = (if button == 0 { self.cursor.count } else { 1 })
                            .min(self.slot_max(index, &self.cursor)?);
                        stack = self.cursor.clone();
                        stack.count = n;
                        self.cursor.count -= n;
                        replacement = true;
                    }
                } else if self.may_place(index, &self.cursor)? {
                    if stack.same_item(&self.cursor) {
                        let n = (if button == 0 { self.cursor.count } else { 1 })
                            .min((self.slot_max(index, &stack)? - stack.count).max(0));
                        stack.count += n;
                        self.cursor.count -= n;
                    } else if self.may_pickup(index)?
                        && self.cursor.count <= self.slot_max(index, &self.cursor)?
                    {
                        std::mem::swap(&mut stack, &mut self.cursor);
                        took = true;
                        replacement = true;
                    }
                } else if self.may_pickup(index)?
                    && stack.same_item(&self.cursor)
                    && stack.count + self.cursor.count <= self.cursor.max_count()?
                {
                    self.cursor.count += stack.count;
                    stack = Stack::default();
                    took = true;
                    replacement = true;
                }
                self.set(index, stack, replacement);
                if took {
                    self.on_take(index)?;
                }
                self.cursor = normalized(self.cursor.clone());
            }
            ClickMode::QuickMove => {
                if button > 1 {
                    return Err("quick move button".into());
                }
                self.quick_move(index)?;
            }
            ClickMode::Swap => self.swap(index, button)?,
            ClickMode::Clone => {
                if self.source.game_mode == 1 && self.cursor.empty() && !stack.empty() {
                    self.cursor = stack;
                    self.cursor.count = self.cursor.max_count()?;
                }
            }
            ClickMode::Throw => {
                if button > 1 {
                    return Err("throw button".into());
                }
                if self.cursor.empty() && self.may_pickup(index)? {
                    loop {
                        let mut stack = self.slot(index)?.clone();
                        if stack.empty() {
                            break;
                        }
                        let id = stack.id;
                        let n = if button == 0 { 1 } else { stack.count };
                        stack.count -= n;
                        let replacement = stack.empty();
                        self.set(index, stack, replacement);
                        self.on_take(index)?;
                        if button == 0 || self.slot(index)?.empty() || self.slot(index)?.id != id {
                            break;
                        }
                    }
                }
            }
            ClickMode::PickupAll => {
                if button > 1 {
                    return Err("pickup all button".into());
                }
                if !self.cursor.empty() && (stack.empty() || !self.may_pickup(index)?) {
                    let mut indices = (0..self.menu.slots.len()).collect::<Vec<_>>();
                    if button == 1 {
                        indices.reverse();
                    }
                    for pass in 0..2 {
                        for i in &indices {
                            let mut s = self.slot(*i)?.clone();
                            if !s.empty()
                                && s.same_item(&self.cursor)
                                && self.may_pickup(*i)?
                                && self.can_take_all(*i)
                                && (pass == 1 || s.count < s.max_count()?)
                            {
                                let n = (self.cursor.max_count()? - self.cursor.count).min(s.count);
                                if n <= 0 {
                                    return Ok(());
                                }
                                if !self.may_place(*i, &s)? && n < s.count {
                                    continue;
                                }
                                self.cursor.count += n;
                                s.count -= n;
                                let replacement = s.empty();
                                self.set(*i, s, replacement);
                                self.on_take(*i)?;
                            }
                        }
                    }
                }
            }
            ClickMode::QuickCraft => unreachable!(),
        }
        Ok(())
    }
    fn quick_craft(&mut self, slot: i16, button: u8) -> Result<()> {
        let header = button & 3;
        if header != self.drag.status && !(self.drag.status == 1 && header == 2) {
            self.drag = QuickCraft::default();
            return Ok(());
        }
        let kind = if header == 0 {
            (button >> 2) & 3
        } else {
            self.drag.kind
        };
        if self.cursor.empty() || kind > 2 || kind == 2 && self.source.game_mode != 1 {
            self.drag = QuickCraft::default();
            return Ok(());
        }
        match header {
            0 => {
                self.drag = QuickCraft {
                    status: 1,
                    kind,
                    slots: BTreeSet::new(),
                };
            }
            1 if self.drag.status == 1 => {
                if let Ok(i) = usize::try_from(slot) {
                    let s = self.slot(i)?;
                    if (s.empty()
                        || s.same_item(&self.cursor) && s.count <= self.cursor.max_count()?)
                        && self.may_place(i, &self.cursor)?
                        && (kind == 2 || self.cursor.count > self.drag.slots.len() as i32)
                    {
                        self.drag.slots.insert(i);
                    }
                }
            }
            2 if self.drag.status == 1 => {
                let slots = std::mem::take(&mut self.drag.slots);
                self.drag = QuickCraft::default();
                if slots.len() == 1 {
                    if kind == 2 {
                        return Ok(());
                    }
                    return self.click(*slots.first().unwrap() as i16, kind, ClickMode::Pickup);
                }
                let original = self.cursor.count;
                for i in &slots {
                    let mut s = self.slot(*i)?.clone();
                    if !self.may_place(*i, &self.cursor)?
                        || !s.empty()
                            && (!s.same_item(&self.cursor) || s.count > self.cursor.max_count()?)
                        || kind != 2 && original < slots.len() as i32
                    {
                        continue;
                    }
                    let n = match kind {
                        0 => original / (slots.len() as i32),
                        1 => 1,
                        _ => self.cursor.max_count()?,
                    };
                    let wanted = (s.count + n).min(self.slot_max(*i, &self.cursor)?);
                    let inserted = (wanted - s.count).max(0);
                    if s.empty() {
                        s = self.cursor.clone();
                        s.count = 0;
                    }
                    s.count += inserted;
                    self.cursor.count -= inserted;
                    self.set(*i, s, true);
                }
                self.cursor = normalized(self.cursor.clone());
            }
            _ => self.drag = QuickCraft::default(),
        }
        Ok(())
    }
}
