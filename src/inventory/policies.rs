//! Slot and quick-move policies from the pinned menu classes. Ranges are half-open.
use super::{Prediction, normalized};
use crate::{
    inventory::{InventoryStore, MenuKind, Stack, persistent},
    protocol::{Result, Writer, content::Content},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::sync::Arc;
fn damaged(stack: &Stack) -> bool {
    stack
        .component("minecraft:max_damage")
        .and_then(Value::as_i64)
        .is_some_and(|max| {
            stack.component("minecraft:unbreakable").is_none()
                && stack
                    .component("minecraft:damage")
                    .and_then(Value::as_i64)
                    .unwrap_or(0)
                    .clamp(0, max)
                    > 0
        })
}
fn enchanted(stack: &Stack) -> bool {
    stack
        .component("minecraft:enchantments")
        .and_then(Value::as_object)
        .is_some_and(|v| !v.is_empty())
}
fn equippable_slot(stack: &Stack) -> Option<&str> {
    stack
        .component("minecraft:equippable")
        .and_then(|v| v.get("slot"))
        .and_then(Value::as_str)
}
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SlotChange {
    Unchanged,
    Mutated,
    Replaced,
}
pub(super) fn inventory_add(
    slots: &mut [Option<Stack>],
    selected_slot: u8,
    creative: bool,
    stack: &mut Stack,
) -> Result<(bool, [SlotChange; 46])> {
    let mut changes = [SlotChange::Unchanged; 46];
    if stack.empty() {
        return Ok((false, changes));
    }
    // Inventory.add copies a damaged stack whole, without merging or splitting.
    if damaged(stack) {
        for slot in (36..45).chain(9..36) {
            if slots[slot]
                .as_ref()
                .ok_or("player slot not observed")?
                .empty()
            {
                slots[slot] = Some(std::mem::take(stack));
                changes[slot] = SlotChange::Replaced;
                return Ok((true, changes));
            }
        }
        if creative {
            stack.count = 0;
        }
        return Ok((creative, changes));
    }
    let order = std::iter::once(36 + selected_slot as usize)
        .chain(std::iter::once(45))
        .chain(36..45)
        .chain(9..36);
    for slot in order {
        if stack.empty() {
            break;
        }
        let target = slots[slot].as_mut().ok_or("player slot not observed")?;
        if !target.empty() && target.max_count()? > 1 && !damaged(target) && target.same_item(stack)
        {
            let count = (target.max_count()? - target.count).max(0).min(stack.count);
            if count > 0 {
                target.count += count;
                stack.count -= count;
                changes[slot] = SlotChange::Mutated;
            }
        }
    }
    for slot in (36..45).chain(9..36) {
        if stack.empty() {
            break;
        }
        let target = slots[slot].as_mut().ok_or("player slot not observed")?;
        if target.empty() {
            let mut inserted = stack.clone();
            inserted.count = stack.count.min(stack.max_count()?);
            stack.count -= inserted.count;
            *target = inserted;
            changes[slot] = SlotChange::Replaced;
        }
    }
    if !stack.empty() && creative {
        stack.count = 0;
    }
    Ok((stack.empty(), changes))
}
struct BundleMutable {
    stacks: Vec<Stack>,
    weight: (i128, i128),
    selected: i32,
}
impl Prediction<'_> {
    fn item_name(&self, stack: &Stack) -> Result<&str> {
        self.content
            .item_names
            .get(&stack.id)
            .map(String::as_str)
            .ok_or("unknown item ID".into())
    }
    fn tagged(&self, stack: &Stack, tag: &str) -> Result<bool> {
        Ok(self
            .content
            .registry_tag("minecraft:item", tag)?
            .contains(&(stack.id as i32)))
    }
    fn output(&self, index: usize) -> bool {
        match self.menu.kind {
            MenuKind::Inventory | MenuKind::Crafting => index == 0,
            MenuKind::Furnace
            | MenuKind::Smoker
            | MenuKind::BlastFurnace
            | MenuKind::Anvil
            | MenuKind::Grindstone
            | MenuKind::Merchant
            | MenuKind::Cartography => index == 2,
            MenuKind::Smithing | MenuKind::Loom => index == 3,
            MenuKind::Stonecutter => index == 1,
            MenuKind::Crafter => index == 45,
            _ => false,
        }
    }
    fn player_start(&self) -> usize {
        match self.menu.kind {
            MenuKind::Inventory => 9,
            MenuKind::Lectern => 1,
            MenuKind::Crafter => 9,
            _ => self.menu.slots.len().saturating_sub(36),
        }
    }
    pub(super) fn player_menu_slot(&self, player: usize) -> Option<usize> {
        if self.menu.kind == MenuKind::Inventory {
            Some(player)
        } else if (9..45).contains(&player) && self.menu.kind != MenuKind::Lectern {
            Some(self.player_start() + player - 9)
        } else {
            None
        }
    }
    fn equippable_for(&self, stack: &Stack, slot: &str, entity: &str) -> Result<bool> {
        if equippable_slot(stack) != Some(slot) {
            return Ok(false);
        }
        let Some(allowed) = stack
            .component("minecraft:equippable")
            .and_then(|v| v.get("allowed_entities"))
        else {
            return Ok(true);
        };
        match allowed {
            Value::String(s) if s.starts_with('#') => {
                let id = self
                    .content
                    .registry_entries("minecraft:entity_type")?
                    .iter()
                    .position(|(n, _)| n == entity)
                    .ok_or("equipment entity holder")?;
                Ok(self
                    .content
                    .registry_tag("minecraft:entity_type", s)?
                    .contains(&(id as i32)))
            }
            Value::String(s) => Ok(s == entity),
            Value::Array(a) => Ok(a.iter().any(|v| v.as_str() == Some(entity))),
            _ => Err("equipment holder set".into()),
        }
    }
    pub(super) fn may_place(&self, index: usize, stack: &Stack) -> Result<bool> {
        if stack.empty() {
            return Ok(false);
        }
        if self.output(index) {
            return Ok(false);
        }
        let name = self.item_name(stack)?;
        Ok(match self.menu.kind {
            MenuKind::Inventory if (5..9).contains(&index) => self.equippable_for(
                stack,
                ["head", "chest", "legs", "feet"][index - 5],
                "minecraft:player",
            )?,
            MenuKind::Inventory => true,
            MenuKind::Crafter if index < 9 => {
                self.menu
                    .data
                    .get(&(index as i16))
                    .copied()
                    .ok_or("crafter slot state unknown")?
                    != 1
            }
            MenuKind::ShulkerBox if index < 27 => self.content.item_can_fit_container(stack.id)?,
            MenuKind::BrewingStand if index < 3 => matches!(
                name,
                "minecraft:potion"
                    | "minecraft:splash_potion"
                    | "minecraft:lingering_potion"
                    | "minecraft:glass_bottle"
            ),
            MenuKind::BrewingStand if index == 3 => {
                self.content.item_is_brewing_ingredient(stack.id)?
            }
            MenuKind::BrewingStand if index == 4 => self.tagged(stack, "minecraft:brewing_fuel")?,
            MenuKind::Enchantment if index == 1 => name == "minecraft:lapis_lazuli",
            MenuKind::Beacon if index == 0 => {
                self.tagged(stack, "minecraft:beacon_payment_items")?
            }
            MenuKind::Loom if index == 0 => self.content.item_is_banner(stack.id)?,
            MenuKind::Loom if index == 1 => {
                self.tagged(stack, "minecraft:loom_dyes")?
                    && stack.component("minecraft:dye").is_some()
            }
            MenuKind::Loom if index == 2 => {
                self.tagged(stack, "minecraft:loom_patterns")?
                    && stack
                        .component("minecraft:provides_banner_patterns")
                        .is_some()
            }
            MenuKind::Cartography if index == 0 => stack.component("minecraft:map_id").is_some(),
            MenuKind::Cartography if index == 1 => matches!(
                name,
                "minecraft:paper" | "minecraft:map" | "minecraft:glass_pane"
            ),
            MenuKind::Grindstone if index < 2 => {
                stack.component("minecraft:max_damage").is_some()
                    && stack.component("minecraft:unbreakable").is_none()
                    || enchanted(stack)
                    || stack
                        .component("minecraft:stored_enchantments")
                        .and_then(Value::as_object)
                        .is_some_and(|v| !v.is_empty())
            }
            MenuKind::Furnace | MenuKind::Smoker | MenuKind::BlastFurnace if index == 1 => {
                self.content.item_is_fuel(stack.id)? || name == "minecraft:bucket"
            }
            MenuKind::Smithing if index < 3 => self.source.recipe_properties_contains(
                [
                    "minecraft:smithing_template",
                    "minecraft:smithing_base",
                    "minecraft:smithing_addition",
                ][index],
                stack.id,
            ),
            MenuKind::Horse if index < 2 => {
                let entity = self
                    .menu
                    .mount_kind
                    .as_deref()
                    .ok_or("mount kind unknown")?;
                self.equippable_for(stack, if index == 0 { "saddle" } else { "body" }, entity)?
            }
            _ => true,
        })
    }
    pub(super) fn slot_max(&self, index: usize, stack: &Stack) -> Result<i32> {
        let capped = matches!(
            (self.menu.kind, index),
            (MenuKind::Inventory, 5..=8)
                | (MenuKind::BrewingStand, 0..=2)
                | (MenuKind::Enchantment, 0)
                | (MenuKind::Beacon, 0)
                | (MenuKind::Horse, 0..=1)
        );
        Ok(
            if capped
                || matches!(
                    self.menu.kind,
                    MenuKind::Furnace | MenuKind::Smoker | MenuKind::BlastFurnace
                ) && index == 1
                    && self.item_name(stack)? == "minecraft:bucket"
            {
                1
            } else {
                stack.max_count()?
            },
        )
    }
    pub(super) fn may_pickup(&self, index: usize) -> Result<bool> {
        if self.source.game_mode == 3 {
            return Ok(false);
        }
        if self.menu.kind == MenuKind::Crafter && index == 45 {
            return Ok(false);
        }
        if self.menu.kind == MenuKind::Inventory
            && (5..9).contains(&index)
            && self.source.game_mode != 1
        {
            if let Some(enchantments) = self
                .slot(index)?
                .component("minecraft:enchantments")
                .and_then(Value::as_object)
            {
                for (name, level) in enchantments {
                    if level.as_i64().unwrap_or(0) > 0 {
                        let entry = self
                            .content
                            .registry_entries("minecraft:enchantment")?
                            .iter()
                            .find(|(id, _)| id == name)
                            .ok_or("equipment enchantment holder")?;
                        if entry
                            .1
                            .get("effects")
                            .and_then(|v| v.get("minecraft:prevent_armor_change"))
                            .is_some()
                        {
                            return Ok(false);
                        }
                    }
                }
            }
        }
        if self.menu.kind == MenuKind::Anvil && index == 2 {
            let cost = *self.menu.data.get(&0).ok_or("anvil cost unknown")? as i32;
            return Ok(cost > 0 && (self.source.game_mode == 1 || self.experience_level >= cost));
        }
        Ok(true)
    }
    pub(super) fn can_take_all(&self, index: usize) -> bool {
        match self.menu.kind {
            MenuKind::Inventory | MenuKind::Crafting => index != 0,
            MenuKind::Cartography | MenuKind::Grindstone | MenuKind::Anvil => index != 2,
            MenuKind::Smithing | MenuKind::Loom => index != 3,
            MenuKind::Stonecutter => index != 1,
            _ => true,
        }
    }
    fn move_to(&mut self, left: &mut Stack, range: Range<usize>, reverse: bool) -> Result<bool> {
        let before = left.count;
        let indices = if reverse {
            range.rev().collect::<Vec<_>>()
        } else {
            range.collect::<Vec<_>>()
        };
        if left.max_count()? > 1 && !damaged(left) {
            for index in &indices {
                if left.empty() {
                    break;
                }
                let mut target = self.slot(*index)?.clone();
                if !target.empty() && target.same_item(left) {
                    let count = (self.slot_max(*index, left)? - target.count)
                        .max(0)
                        .min(left.count);
                    if count > 0 {
                        target.count += count;
                        left.count -= count;
                        self.set(*index, target, false);
                    }
                }
            }
        }
        if !left.empty() {
            for index in indices {
                if self.slot(index)?.empty() && self.may_place(index, left)? {
                    let mut target = left.clone();
                    target.count = left.count.min(self.slot_max(index, left)?);
                    left.count -= target.count;
                    self.set(index, target, true);
                    break;
                }
            }
        }
        Ok(left.count != before)
    }
    fn main_hotbar(&mut self, left: &mut Stack, index: usize) -> Result<bool> {
        let start = self.player_start();
        if index < start + 27 {
            self.move_to(left, start + 27..start + 36, false)
        } else {
            self.move_to(left, start..start + 27, false)
        }
    }
    pub(super) fn quick_move(&mut self, index: usize) -> Result<()> {
        if !self.may_pickup(index)? {
            return Ok(());
        }
        loop {
            let mut left = self.slot(index)?.clone();
            if left.empty() {
                break;
            }
            let original = left.clone();
            let start = self.player_start();
            let moved = match self.menu.kind {
                MenuKind::Lectern => false,
                MenuKind::Inventory => {
                    if index == 0 {
                        self.move_to(&mut left, 9..45, true)?
                    } else if index < 9 {
                        self.move_to(&mut left, 9..45, false)?
                    } else {
                        let armor = equippable_slot(&left)
                            .and_then(|slot| {
                                ["head", "chest", "legs", "feet"]
                                    .iter()
                                    .position(|v| *v == slot)
                            })
                            .map(|i| i + 5);
                        if let Some(armor) =
                            armor.filter(|slot| self.slot(*slot).is_ok_and(Stack::empty))
                        {
                            self.move_to(&mut left, armor..armor + 1, false)?
                        } else if equippable_slot(&left) == Some("offhand")
                            && self.slot(45)?.empty()
                        {
                            self.move_to(&mut left, 45..46, false)?
                        } else {
                            if index == 45 {
                                self.move_to(&mut left, 9..45, false)?
                            } else {
                                self.main_hotbar(&mut left, index)?
                            }
                        }
                    }
                }
                MenuKind::Crafting => {
                    if index == 0 {
                        self.move_to(&mut left, 10..46, true)?
                    } else if index < 10 {
                        self.move_to(&mut left, 10..46, false)?
                    } else {
                        self.move_to(&mut left, 1..10, false)?
                            || self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Chest(_)
                | MenuKind::Dispenser
                | MenuKind::Hopper
                | MenuKind::Crafter
                | MenuKind::ShulkerBox => {
                    if index < start {
                        self.move_to(&mut left, start..start + 36, true)?
                    } else {
                        self.move_to(&mut left, 0..start, false)?
                    }
                }
                MenuKind::Enchantment => {
                    if index < 2 {
                        self.move_to(&mut left, 2..38, true)?
                    } else if self.item_name(&left)? == "minecraft:lapis_lazuli" {
                        self.move_to(&mut left, 1..2, true)?
                    } else if self.slot(0)?.empty() {
                        let original_count = left.count;
                        left.count = 1;
                        let moved = self.move_to(&mut left, 0..1, false)?;
                        left.count += original_count - 1;
                        moved
                    } else {
                        false
                    }
                }
                MenuKind::Beacon => {
                    if index == 0 {
                        self.move_to(&mut left, 1..37, true)?
                    } else if self.slot(0)?.empty()
                        && left.count == 1
                        && self.may_place(0, &left)?
                    {
                        self.move_to(&mut left, 0..1, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::BrewingStand => {
                    if index < 5 {
                        self.move_to(&mut left, 5..41, true)?
                    } else if self.may_place(4, &left)? && self.move_to(&mut left, 4..5, false)? {
                        let replacement = left.empty();
                        self.set(index, left, replacement);
                        break;
                    } else if self.may_place(3, &left)? {
                        self.move_to(&mut left, 3..4, false)?
                    } else if self.may_place(0, &left)? {
                        self.move_to(&mut left, 0..3, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Furnace | MenuKind::Smoker | MenuKind::BlastFurnace => {
                    if index == 2 {
                        self.move_to(&mut left, 3..39, true)?
                    } else if index < 2 {
                        self.move_to(&mut left, 3..39, false)?
                    } else if self.source.recipe_properties_contains(
                        match self.menu.kind {
                            MenuKind::Furnace => "minecraft:furnace_input",
                            MenuKind::Smoker => "minecraft:smoker_input",
                            _ => "minecraft:blast_furnace_input",
                        },
                        left.id,
                    ) {
                        self.move_to(&mut left, 0..1, false)?
                    } else if self.content.item_is_fuel(left.id)? {
                        self.move_to(&mut left, 1..2, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Loom => {
                    if index == 3 {
                        self.move_to(&mut left, 4..40, true)?
                    } else if index < 3 {
                        self.move_to(&mut left, 4..40, false)?
                    } else if self.may_place(0, &left)? {
                        self.move_to(&mut left, 0..1, false)?
                    } else if self.may_place(1, &left)? {
                        self.move_to(&mut left, 1..2, false)?
                    } else if self.may_place(2, &left)? {
                        self.move_to(&mut left, 2..3, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Merchant => {
                    if index == 2 {
                        self.move_to(&mut left, 3..39, true)?
                    } else if index < 2 {
                        self.move_to(&mut left, 3..39, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Stonecutter => {
                    if index == 1 {
                        self.move_to(&mut left, 2..38, true)?
                    } else if index == 0 {
                        self.move_to(&mut left, 2..38, false)?
                    } else if self
                        .source
                        .stonecutter_recipes
                        .iter()
                        .any(|r| r.ingredient_items.contains(&left.id))
                    {
                        self.move_to(&mut left, 0..1, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Cartography => {
                    if index == 2 {
                        self.move_to(&mut left, 3..39, true)?
                    } else if index < 2 {
                        self.move_to(&mut left, 3..39, false)?
                    } else if self.may_place(0, &left)? {
                        self.move_to(&mut left, 0..1, false)?
                    } else if self.may_place(1, &left)? {
                        self.move_to(&mut left, 1..2, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Grindstone => {
                    if index == 2 {
                        self.move_to(&mut left, 3..39, true)?
                    } else if index < 2 {
                        self.move_to(&mut left, 3..39, false)?
                    } else if self.slot(0)?.empty() || self.slot(1)?.empty() {
                        self.move_to(&mut left, 0..2, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Anvil | MenuKind::Smithing => {
                    let result = start - 1;
                    if index == result {
                        self.move_to(&mut left, start..start + 36, true)?
                    } else if index < result {
                        self.move_to(&mut left, start..start + 36, false)?
                    } else if (0..result).any(|i| {
                        self.slot(i).is_ok_and(Stack::empty)
                            && self.may_place(i, &left).unwrap_or(false)
                    }) {
                        self.move_to(&mut left, 0..result, false)?
                    } else {
                        self.main_hotbar(&mut left, index)?
                    }
                }
                MenuKind::Horse => {
                    if index < start {
                        self.move_to(&mut left, start..start + 36, true)?
                    } else if self.slot(1)?.empty() && self.may_place(1, &left)? {
                        self.move_to(&mut left, 1..2, false)?
                    } else if self.slot(0)?.empty() && self.may_place(0, &left)? {
                        self.move_to(&mut left, 0..1, false)?
                    } else if self.move_to(&mut left, 2..start, false)? {
                        true
                    } else {
                        self.main_hotbar(&mut left, index)?;
                        let replacement = left.empty();
                        self.set(index, left, replacement);
                        break;
                    }
                }
            };
            if !moved {
                break;
            }
            let count = original.count - left.count;
            let replacement = left.empty();
            self.set(index, left, replacement);
            if !self.slot(index)?.empty()
                && self.output(index)
                && matches!(
                    self.menu.kind,
                    MenuKind::Inventory | MenuKind::Crafting | MenuKind::Stonecutter
                )
            {
                self.set(index, Stack::default(), true);
            }
            if count > 0 {
                self.on_take(index)?;
            }
            let next = self.slot(index)?;
            if next.empty() || next.id != original.id {
                break;
            }
        }
        Ok(())
    }
    pub(super) fn swap(&mut self, index: usize, button: u8) -> Result<()> {
        let player = match button {
            0..=8 => 36 + button as usize,
            40 => 45,
            _ => return Err("swap button".into()),
        };
        let target = self.player_menu_slot(player);
        if target == Some(index) {
            return Ok(());
        }
        let mut hotbar = if let Some(target) = target {
            self.slot(target)?.clone()
        } else {
            self.player.slots[player]
                .clone()
                .ok_or("player swap slot not observed")?
        };
        let mut current = self.slot(index)?.clone();
        if hotbar.empty() && current.empty() {
            return Ok(());
        }
        if hotbar.empty() {
            if self.may_pickup(index)? {
                self.set_player(player, current, true);
                self.set(index, Stack::default(), true);
                self.on_take(index)?;
            }
        } else if self.may_place(index, &hotbar)? {
            let cap = self.slot_max(index, &hotbar)?;
            if current.empty() {
                if hotbar.count > cap {
                    let mut inserted = hotbar.clone();
                    inserted.count = cap;
                    hotbar.count -= cap;
                    self.set_player(player, hotbar, false);
                    self.set(index, inserted, true);
                } else {
                    self.set_player(player, Stack::default(), true);
                    self.set(index, hotbar, true);
                }
            } else if self.may_pickup(index)? {
                if hotbar.count <= cap {
                    self.set_player(player, current, true);
                    self.set(index, hotbar, true);
                    self.on_take(index)?;
                } else {
                    let mut inserted = hotbar.clone();
                    inserted.count = cap;
                    hotbar.count -= cap;
                    self.set_player(player, hotbar, false);
                    self.set(index, inserted, true);
                    self.on_take(index)?;
                    self.add_to_player(&mut current)?;
                }
            }
        }
        Ok(())
    }
    fn add_to_player(&mut self, stack: &mut Stack) -> Result<bool> {
        let (added, changes) = inventory_add(
            &mut self.player.slots,
            self.source.selected_slot,
            self.source.game_mode == 1,
            stack,
        )?;
        for (player, change) in changes.into_iter().enumerate() {
            if change == SlotChange::Unchanged {
                continue;
            }
            if let Some(target) = self.player_menu_slot(player) {
                self.menu.slots[target] = self.player.slots[player].clone();
            }
            if change == SlotChange::Replaced {
                self.player_slot_identities[player] =
                    self.player_slot_identities[player].wrapping_add(1);
            }
        }
        Ok(added)
    }
    pub(super) fn predict_button(&mut self, button: i32) -> Result<()> {
        match self.menu.kind {
            MenuKind::Loom => {
                self.menu.data.insert(0, button);
                let banner = self.slot(0)?.clone();
                let dye = self
                    .slot(1)?
                    .component("minecraft:dye")
                    .cloned()
                    .ok_or("loom dye unavailable")?;
                let pattern = self
                    .source
                    .loom_patterns(self.content)?
                    .get(button as usize)
                    .cloned()
                    .ok_or("loom pattern unavailable")?;
                let mut patterns = banner
                    .component("minecraft:banner_patterns")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                patterns.push(json!({"pattern":pattern,"color":dye}));
                let mut stack = crate::inventory::stack_with_components(
                    self.content,
                    &banner,
                    &BTreeMap::from([("minecraft:banner_patterns".into(), Value::Array(patterns))]),
                )?;
                stack.count = 1;
                self.set(3, stack, true);
            }
            MenuKind::Stonecutter => {
                self.menu.data.insert(0, button);
                // The received SingleInputSet deliberately omits its full
                // recipe holder. Vanilla's client setupResultSlot clears the
                // actual output; the display remains a UI preview only.
                self.set(1, Stack::default(), true);
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn select_trade(&mut self, index: usize) -> Result<()> {
        if self.menu.kind != MenuKind::Merchant {
            return Err("merchant unavailable".into());
        }
        let trade = self
            .source
            .menu()
            .trades
            .as_ref()
            .and_then(|t| t.offers.get(index))
            .ok_or("trade unavailable")?;
        Arc::make_mut(self.menu.trades.as_mut().unwrap()).selected = index;
        for slot in 0..2 {
            let mut input = self.slot(slot)?.clone();
            if !input.empty() {
                if !self.move_to(&mut input, 3..39, true)? {
                    return Ok(());
                }
                self.set(slot, input, false);
            }
        }
        if self.slot(0)?.empty() && self.slot(1)?.empty() {
            for (slot, cost) in [(0, Some(&trade.buy_a)), (1, trade.buy_b.as_ref())] {
                if let Some(cost) = cost {
                    for i in 3..39 {
                        let mut s = self.slot(i)?.clone();
                        if cost.matches(&s) {
                            let mut input = self.slot(slot)?.clone();
                            if input.empty() {
                                input = s.clone();
                                input.count = 0;
                            }
                            if s.same_item(&input) {
                                let count = (input.max_count()? - input.count).max(0).min(s.count);
                                input.count += count;
                                s.count -= count;
                                self.set(i, s, false);
                                self.set(slot, input, true);
                            }
                        }
                    }
                }
            }
        }
        self.refresh_trade()
    }
    pub(super) fn refresh_trade(&mut self) -> Result<()> {
        self.menu.refresh_active_offer()?;
        let trades = self.menu.trades.as_ref().ok_or("merchant offers unknown")?;
        let result = self
            .menu
            .active_offer
            .filter(|&i| trades.offers[i].available())
            .map_or_else(Stack::default, |i| trades.offers[i].sell.clone());
        self.set(2, result, true);
        Ok(())
    }
    pub(super) fn on_take(&mut self, index: usize) -> Result<()> {
        match self.menu.kind {
            MenuKind::Inventory | MenuKind::Crafting if index == 0 => {
                let end = if self.menu.kind == MenuKind::Inventory {
                    5
                } else {
                    10
                };
                for i in 1..end {
                    let mut input = self.slot(i)?.clone();
                    if input.empty() {
                        continue;
                    }
                    let remainder = self.content.item_crafting_remainder(input.id)?;
                    let mut remainder = match remainder {
                        None => Stack::default(),
                        Some(value) => {
                            let name = value
                                .get("id")
                                .and_then(Value::as_str)
                                .ok_or("crafting remainder item")?;
                            let id = self
                                .content
                                .item_names
                                .iter()
                                .find_map(|(id, item)| (item == name).then_some(*id))
                                .ok_or("crafting remainder holder")?;
                            let count =
                                value.get("count").and_then(Value::as_i64).unwrap_or(1) as i32;
                            let overrides = value
                                .get("components")
                                .and_then(Value::as_object)
                                .map(|o| {
                                    o.iter()
                                        .map(|(k, v)| (k.clone(), v.clone()))
                                        .collect::<BTreeMap<_, _>>()
                                })
                                .unwrap_or_default();
                            crate::inventory::creative_stack_with_components(
                                self.content,
                                id,
                                count,
                                &overrides,
                            )?
                        }
                    };
                    let mut replacement = false;
                    input.count -= 1;
                    input = normalized(input);
                    if !remainder.empty() {
                        if input.empty() {
                            input = remainder;
                            replacement = true;
                        } else if input.same_item(&remainder) {
                            remainder.count += input.count;
                            input = remainder;
                            replacement = true;
                        } else {
                            self.add_to_player(&mut remainder)?;
                        }
                    }
                    self.set(i, input, replacement);
                }
            }
            MenuKind::Loom if index == 3 => {
                for i in 0..2 {
                    let mut s = self.slot(i)?.clone();
                    s.count -= 1;
                    self.set(i, s, false);
                }
                if !self.slot(0)?.empty() && !self.slot(1)?.empty() {
                    let selected = self.menu.data.get(&0).copied().unwrap_or(-1);
                    if selected >= 0 {
                        self.predict_button(selected as i32)?;
                    }
                } else {
                    self.menu.data.insert(0, -1);
                    self.set(3, Stack::default(), true);
                }
            }
            MenuKind::Stonecutter if index == 1 => {
                let mut s = self.slot(0)?.clone();
                s.count -= 1;
                self.set(0, s, false);
                if !self.slot(0)?.empty() {
                    let selected = self.menu.data.get(&0).copied().unwrap_or(-1);
                    if selected >= 0 {
                        self.predict_button(selected as i32)?;
                    }
                }
            }
            MenuKind::Merchant if index == 2 => {
                if let Some(i) = self.menu.active_offer {
                    let trade = self
                        .menu
                        .trades
                        .as_ref()
                        .and_then(|trades| trades.offers.get(i))
                        .ok_or("merchant active offer unavailable")?;
                    let xp = trade.xp;
                    let a = self.slot(0)?;
                    let b = self.slot(1)?;
                    let swapped = if trade.accepts(a, b)? {
                        Some(false)
                    } else if trade.accepts(b, a)? {
                        Some(true)
                    } else {
                        None
                    };
                    if let Some(swapped) = swapped {
                        let a = trade.cost_a()?;
                        let b = trade.buy_b.as_ref().map_or(0, |cost| cost.stack.count);
                        for (slot, count) in [(usize::from(swapped), a), (usize::from(!swapped), b)]
                        {
                            let mut stack = self.slot(slot)?.clone();
                            stack.count -= count;
                            self.set(slot, stack, false);
                        }
                        let trades = Arc::make_mut(self.menu.trades.as_mut().unwrap());
                        trades.offers[i].uses = trades.offers[i].uses.wrapping_add(1);
                        self.refresh_trade()?;
                    }
                    // MerchantResultSlot awards the active offer's XP even
                    // when neither orientation of MerchantOffer.take succeeds.
                    let trades = Arc::make_mut(self.menu.trades.as_mut().unwrap());
                    trades.xp = trades.xp.wrapping_add(xp);
                }
            }
            MenuKind::Cartography if index == 2 => {
                for i in 0..2 {
                    let mut s = self.slot(i)?.clone();
                    s.count -= 1;
                    self.set(i, s, false);
                }
            }
            MenuKind::Grindstone if index == 2 => {
                self.set(0, Stack::default(), true);
                self.set(1, Stack::default(), true);
            }
            MenuKind::Smithing if index == 3 => {
                for i in 0..3 {
                    let mut s = self.slot(i)?.clone();
                    s.count -= 1;
                    self.set(i, s, false);
                }
            }
            MenuKind::Anvil if index == 2 => {
                if self.source.game_mode != 1 {
                    self.experience_level -= self.menu.data.get(&0).copied().unwrap_or(0) as i32;
                }
                let material = self.menu.repair_material_count;
                if material > 0 {
                    let mut s = self.slot(1)?.clone();
                    s.count -= material;
                    let replacement = s.empty();
                    self.set(1, s, replacement);
                } else if !self.menu.only_renaming {
                    self.set(1, Stack::default(), true);
                }
                self.set(0, Stack::default(), true);
                self.menu.data.insert(0, 0);
            }
            _ => {}
        }
        Ok(())
    }
    fn bundle_stacks(bundle: &Stack) -> Result<&[Stack]> {
        if let Some(component) = bundle
            .patch
            .added
            .values()
            .find(|v| v.name == "minecraft:bundle_contents")
        {
            return Ok(&component.nested_stacks);
        }
        if bundle
            .component("minecraft:bundle_contents")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
        {
            return Ok(&[]);
        }
        Err("bundle lacks retained nested stacks".into())
    }
    fn bundle_template_valid(stack: &Stack) -> Result<bool> {
        if stack.count > stack.max_count()?
            || stack.component("minecraft:max_damage").is_some() && stack.max_count()? > 1
        {
            return Ok(false);
        }
        for name in [
            "minecraft:container",
            "minecraft:bundle_contents",
            "minecraft:charged_projectiles",
        ] {
            if let Some(value) = stack.patch.added.values().find(|value| value.name == name) {
                for item in value.nested_stacks.iter() {
                    if item.count > item.max_count()? {
                        return Ok(false);
                    }
                }
            }
        }
        if stack.component("minecraft:bundle_contents").is_some()
            && Self::bundle_contents_weight(Self::bundle_stacks(stack)?, 0)?.is_none()
        {
            return Ok(false);
        }
        Ok(true)
    }
    fn bundle_mutable(bundle: &Stack) -> Result<BundleMutable> {
        let stacks = Self::bundle_stacks(bundle)?;
        // Mutable clears arithmetic-invalid contents, not simply weight > 1.
        let Some(weight) = Self::bundle_contents_weight(stacks, 0)? else {
            return Ok(BundleMutable {
                stacks: Vec::new(),
                weight: (0, 1),
                selected: -1,
            });
        };
        let stacks = stacks
            .iter()
            .map(|stack| {
                Ok(if Self::bundle_template_valid(stack)? {
                    stack.clone()
                } else {
                    Stack::default()
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(BundleMutable {
            stacks,
            weight,
            selected: bundle.bundle_selected,
        })
    }
    pub(super) fn select_bundle(&mut self, index: usize, selected: i32) -> Result<()> {
        let mut bundle = self.slot(index)?.clone();
        if bundle.component("minecraft:bundle_contents").is_none() {
            return Ok(());
        }
        let mut contents = Self::bundle_mutable(&bundle)?;
        contents.selected = if selected == contents.selected
            || selected < 0
            || selected as usize >= contents.stacks.len()
        {
            -1
        } else {
            selected
        };
        if contents.stacks != Self::bundle_stacks(&bundle)? {
            bundle =
                Self::bundle_write(self.content, &bundle, &contents.stacks, contents.selected)?;
        } else {
            bundle.bundle_selected = contents.selected;
        }
        self.set(index, bundle, false);
        Ok(())
    }
    fn bundle_remove(contents: &mut BundleMutable) -> Result<Option<Stack>> {
        if contents.stacks.is_empty() {
            return Ok(None);
        }
        let index = usize::try_from(contents.selected)
            .ok()
            .filter(|&i| i < contents.stacks.len())
            .unwrap_or(0);
        let item = contents.stacks.remove(index);
        if !item.empty() {
            let weight =
                Self::bundle_weight(&item, 0)?.ok_or("mutable bundle item weight invalid")?;
            let removed = multiply_fraction(weight, (item.count as i128, 1))
                .ok_or("bundle fraction overflow")?;
            contents.weight = add_fraction(contents.weight, (-removed.0, removed.1))
                .ok_or("bundle fraction overflow")?;
        }
        contents.selected = -1;
        Ok(Some(item))
    }
    fn bundle_write(
        content: &Content,
        bundle: &Stack,
        stacks: &[Stack],
        selected: i32,
    ) -> Result<Stack> {
        let id = content
            .components
            .iter()
            .find_map(|(id, name)| (name == "minecraft:bundle_contents").then_some(*id))
            .ok_or("bundle component type unknown")?;
        let mut output = if stacks.is_empty() {
            persistent::with_components(
                content,
                bundle,
                &BTreeMap::from([("minecraft:bundle_contents".into(), json!([]))]),
            )?
        } else {
            let mut w = Writer::default();
            w.var(stacks.len() as i32);
            for stack in stacks {
                stack.write_template(&mut w)?;
            }
            persistent::rewrite(
                content,
                bundle,
                BTreeMap::from([(id, Some(w.0))]),
                BTreeSet::new(),
            )?
        };
        output.bundle_selected = selected;
        if let Some(component) = Arc::make_mut(&mut output.patch).added.get_mut(&id) {
            component.nested_stacks = stacks.to_vec().into();
        }
        Ok(output)
    }
    fn bundle_contents_weight(stacks: &[Stack], depth: u8) -> Result<Option<(i128, i128)>> {
        let mut weight = (0, 1);
        for stack in stacks {
            let Some(value) = Self::bundle_weight(stack, depth)? else {
                return Ok(None);
            };
            let Some(value) = multiply_fraction(value, (stack.count as i128, 1))
                .and_then(|value| add_fraction(weight, value))
            else {
                return Ok(None);
            };
            weight = value;
        }
        Ok(Some(weight))
    }
    fn bundle_weight(stack: &Stack, depth: u8) -> Result<Option<(i128, i128)>> {
        if depth > 32 {
            return Err("bundle depth bound".into());
        }
        if stack.component("minecraft:bundle_contents").is_some() {
            return Ok(
                Self::bundle_contents_weight(Self::bundle_stacks(stack)?, depth + 1)?
                    .and_then(|weight| add_fraction(weight, (1, 16))),
            );
        }
        if stack
            .component("minecraft:bees")
            .and_then(Value::as_array)
            .is_some_and(|bees| !bees.is_empty())
        {
            return Ok(Some((1, 1)));
        }
        Ok(Some((1, stack.max_count()? as i128)))
    }
    fn bundle_insert(
        content: &Content,
        contents: &mut BundleMutable,
        incoming: &mut Stack,
        whole_only: bool,
    ) -> Result<i32> {
        if incoming.empty() || !content.item_can_fit_container(incoming.id)? {
            return Ok(0);
        }
        let weight = contents.weight;
        let Some(incoming_weight) = Self::bundle_weight(incoming, 0)? else {
            return Ok(0);
        };
        let remaining =
            add_fraction((1, 1), (-weight.0, weight.1)).ok_or("bundle fraction overflow")?;
        let quotient = multiply_fraction(remaining, (incoming_weight.1, incoming_weight.0))
            .ok_or("bundle fraction overflow")?;
        let count = (quotient.0 / quotient.1).max(0).min(incoming.count as i128) as i32;
        if count == 0 || whole_only && count < incoming.count {
            return Ok(0);
        }
        // Java updates its checked Fraction before touching either item list.
        contents.weight = multiply_fraction(incoming_weight, (count as i128, 1))
            .and_then(|value| add_fraction(weight, value))
            .ok_or("bundle fraction overflow")?;
        let existing = if incoming.max_count()? > 1 && !damaged(incoming) {
            contents.stacks.iter().position(|s| s.same_item(incoming))
        } else {
            None
        };
        let mut value = existing
            .map(|i| contents.stacks.remove(i))
            .unwrap_or_else(|| {
                let mut s = incoming.clone();
                s.count = 0;
                s
            });
        value.count += count;
        incoming.count -= count;
        contents.stacks.insert(0, value);
        Ok(count)
    }
    pub(super) fn bundle_override(&mut self, index: usize, button: u8) -> Result<bool> {
        let mut target = self.slot(index)?.clone();
        let mut cursor = self.cursor.clone();
        if cursor.component("minecraft:bundle_contents").is_some()
            && self.content.item_is_bundle(cursor.id)?
        {
            let mut contents = Self::bundle_mutable(&cursor)?;
            if button == 0 && !target.empty() {
                if self.may_pickup(index)? {
                    let before = target.count;
                    let whole_only = !self.may_place(index, &target)?;
                    Self::bundle_insert(self.content, &mut contents, &mut target, whole_only)?;
                    let took = before > target.count;
                    let replacement = took && target.empty();
                    self.set(index, target, replacement);
                    if took {
                        self.on_take(index)?;
                    }
                }
                self.cursor =
                    Self::bundle_write(self.content, &cursor, &contents.stacks, contents.selected)?;
                return Ok(true);
            }
            if button == 1 && target.empty() {
                let mut replacement = false;
                if let Some(mut value) = Self::bundle_remove(&mut contents)? {
                    if !value.empty() && self.may_place(index, &value)? {
                        let mut inserted = value.clone();
                        inserted.count = value.count.min(self.slot_max(index, &value)?);
                        value.count -= inserted.count;
                        target = inserted;
                        replacement = true;
                    }
                    if !value.empty() {
                        Self::bundle_insert(self.content, &mut contents, &mut value, false)?;
                    }
                }
                self.cursor =
                    Self::bundle_write(self.content, &cursor, &contents.stacks, contents.selected)?;
                self.set(index, target, replacement);
                return Ok(true);
            }
        }
        if target.component("minecraft:bundle_contents").is_some()
            && self.content.item_is_bundle(target.id)?
        {
            if button == 0 && cursor.empty() {
                self.select_bundle(index, -1)?;
                return Ok(false);
            }
            let mut contents = Self::bundle_mutable(&target)?;
            if button == 0 && !cursor.empty() {
                if self.may_pickup(index)? && self.may_place(index, &target)? {
                    Self::bundle_insert(self.content, &mut contents, &mut cursor, false)?;
                }
                self.set(
                    index,
                    Self::bundle_write(self.content, &target, &contents.stacks, contents.selected)?,
                    false,
                );
                self.cursor = normalized(cursor);
                return Ok(true);
            }
            if button == 1 && cursor.empty() {
                if self.may_pickup(index)? && self.may_place(index, &target)? {
                    if let Some(item) = Self::bundle_remove(&mut contents)? {
                        cursor = item;
                    }
                }
                self.set(
                    index,
                    Self::bundle_write(self.content, &target, &contents.stacks, contents.selected)?,
                    false,
                );
                self.cursor = cursor;
                return Ok(true);
            }
            self.select_bundle(index, -1)?;
        }
        Ok(false)
    }
}
impl InventoryStore {
    pub fn remove_one_bundle_for_use(
        &self,
        bundle: &Stack,
        content: &Content,
    ) -> Result<Option<(Stack, Stack)>> {
        let mut contents = Prediction::bundle_mutable(bundle)?;
        let Some(removed) = Prediction::bundle_remove(&mut contents)? else {
            return Ok(None);
        };
        let updated =
            Prediction::bundle_write(content, bundle, &contents.stacks, contents.selected)?;
        Ok(Some((updated, removed)))
    }
}
fn gcd(mut a: i128, mut b: i128) -> i128 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.abs()
}
fn checked_fraction(n: i128, d: i128) -> Option<(i128, i128)> {
    if !(i32::MIN as i128..=i32::MAX as i128).contains(&n) || !(1..=i32::MAX as i128).contains(&d) {
        return None;
    }
    let common = gcd(n, d);
    Some((n / common, d / common))
}
// Commons Fraction checks i32 intermediates, not just the final rational value.
fn add_fraction(a: (i128, i128), b: (i128, i128)) -> Option<(i128, i128)> {
    if a.0 == 0 {
        return Some(b);
    }
    if b.0 == 0 {
        return Some(a);
    }
    let common = gcd(a.1, b.1);
    let left = a.0 * (b.1 / common);
    let right = b.0 * (a.1 / common);
    if common == 1 {
        if i32::try_from(left).is_err() || i32::try_from(right).is_err() {
            return None;
        }
        return checked_fraction(left + right, a.1 * b.1);
    }
    let numerator = left + right;
    let reduce = gcd(numerator, common);
    checked_fraction(numerator / reduce, (a.1 / common) * (b.1 / reduce))
}
fn multiply_fraction(a: (i128, i128), b: (i128, i128)) -> Option<(i128, i128)> {
    if a.0 == 0 || b.0 == 0 {
        return Some((0, 1));
    }
    let left = gcd(a.0, b.1);
    let right = gcd(b.0, a.1);
    checked_fraction((a.0 / left) * (b.0 / right), (a.1 / right) * (b.1 / left))
}

#[cfg(test)]
mod fraction_tests {
    use super::{add_fraction, multiply_fraction};

    #[test]
    fn addition_rejects_overflow_before_final_cancellation() {
        let max = i32::MAX as i128;
        assert_eq!(add_fraction((max, 1), (-max, 2)), None);
    }

    #[test]
    fn multiplication_reduces_cross_factors_before_overflow() {
        let max = i32::MAX as i128;
        assert_eq!(multiply_fraction((max, 2), (2, max)), Some((1, 1)));
    }

    #[test]
    fn sub_capacity_bundle_weight_can_overflow_its_denominator() {
        let mut weight = (0, 1);
        for max_stack_size in [89, 83, 79, 73] {
            weight = add_fraction(weight, (1, max_stack_size)).unwrap();
        }
        assert!(weight.0 < weight.1);
        assert_eq!(add_fraction(weight, (1, 71)), None);
    }
}
