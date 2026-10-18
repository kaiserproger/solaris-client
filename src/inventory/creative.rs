//! The creative screen uses ordinary set-slot packets, never container-click hashes.
use super::{InventoryStore, Prediction, normalized};
use crate::{
    inventory::{MenuKind, PacketCommand, Stack, UiContext},
    protocol::{Result, Writer, content::Content},
    ui::ClickMode,
};
fn packet(slot: i16, stack: &Stack) -> Result<PacketCommand> {
    let mut w = Writer::default();
    w.0.extend(slot.to_be_bytes());
    stack.write(&mut w)?;
    Ok(PacketCommand::new(56, w))
}
impl InventoryStore {
    pub(super) fn validate_creative_item(&self, stack: &Stack, content: &Content) -> Result<()> {
        if !self.screen_open
            || !self.creative_screen
            || self.game_mode != 1
            || self.menu().kind != MenuKind::Inventory
        {
            return Err("creative screen unavailable".into());
        }
        if !stack.empty() {
            if !(1..=99).contains(&stack.count) {
                return Err("creative stack count".into());
            }
            if !content
                .item_required_features(stack.id)?
                .iter()
                .all(|flag| self.enabled_features.contains(flag))
            {
                return Err("creative item feature disabled".into());
            }
        }
        Ok(())
    }
    pub fn execute_creative_pick(
        &mut self,
        context: &UiContext,
        stack: &Stack,
        button: u8,
        mode: ClickMode,
        content: &Content,
    ) -> Result<Vec<PacketCommand>> {
        self.validate(context)?;
        self.validate_creative_item(stack, content)?;
        if self.creative_inventory_tab {
            return Err("creative palette unavailable".into());
        }
        let mut cursor = self.cursor.clone().ok_or("cursor not observed")?;
        let mut commands = Vec::new();
        let mut changed_slot = None;
        match mode {
            ClickMode::Swap if button < 9 && !stack.empty() => {
                let mut value = stack.clone();
                value.count = value.max_count()?;
                let slot = 36 + button as i16;
                commands.push(packet(slot, &value)?);
                changed_slot = Some((slot as usize, value));
            }
            ClickMode::Clone if cursor.empty() && !stack.empty() => {
                cursor = stack.clone();
                cursor.count = cursor.max_count()?;
            }
            ClickMode::Throw if !stack.empty() => {
                let mut value = stack.clone();
                value.count = if button == 0 { 1 } else { value.max_count()? };
                commands.push(packet(-1, &value)?);
            }
            ClickMode::Pickup | ClickMode::QuickMove => {
                if button > 1 {
                    return Err("creative pickup button".into());
                }
                if cursor.empty() {
                    if !stack.empty() {
                        cursor = stack.clone();
                        cursor.count = if mode == ClickMode::QuickMove {
                            cursor.max_count()?
                        } else {
                            1
                        };
                    }
                } else if !stack.empty() && cursor.same_item(stack) {
                    if button == 0 {
                        cursor.count = if mode == ClickMode::QuickMove {
                            cursor.max_count()?
                        } else {
                            (cursor.count + 1).min(cursor.max_count()?)
                        };
                    } else {
                        cursor.count -= 1;
                    }
                } else if button == 0 {
                    cursor = Stack::default();
                } else {
                    cursor.count -= 1;
                }
            }
            ClickMode::QuickCraft | ClickMode::PickupAll => {
                return Err("creative palette drag unavailable".into());
            }
            ClickMode::Swap => return Err("creative swap button".into()),
            _ => {}
        }
        if let Some((slot, value)) = changed_slot {
            self.player.slots[slot] = Some(value);
            self.player_slot_identities[slot] = self.player_slot_identities[slot].wrapping_add(1);
        }
        self.cursor = Some(normalized(cursor));
        self.revision += 1;
        Ok(commands)
    }
    pub fn execute_creative_clear(
        &mut self,
        context: &UiContext,
        all: bool,
        content: &Content,
    ) -> Result<Vec<PacketCommand>> {
        self.validate(context)?;
        self.validate_creative_item(&Stack::default(), content)?;
        if !self.creative_inventory_tab {
            return Err("creative trash unavailable".into());
        }
        let mut commands = Vec::new();
        if all {
            commands.reserve(46);
            for slot in 0..46 {
                commands.push(packet(slot, &Stack::default())?);
            }
            for (slot, stack) in self.player.slots.iter_mut().enumerate() {
                *stack = Some(Stack::default());
                self.player_slot_identities[slot] =
                    self.player_slot_identities[slot].wrapping_add(1);
            }
        } else {
            self.cursor = Some(Stack::default());
        }
        self.revision += 1;
        Ok(commands)
    }
}
impl Prediction<'_> {
    pub(super) fn creative_click(
        &mut self,
        slot: i16,
        button: u8,
        mode: ClickMode,
        commands: &mut Vec<PacketCommand>,
    ) -> Result<()> {
        self.source
            .validate_creative_item(&Stack::default(), self.content)?;
        let before = self.menu.slots.clone();
        if slot == -999 && mode == ClickMode::Throw {
            if !self.cursor.empty() {
                commands.push(packet(-1, &self.cursor)?);
                self.cursor = Stack::default();
            }
            return Ok(());
        }
        if slot == -999 && mode == ClickMode::Pickup {
            if button > 1 {
                return Err("creative pickup button".into());
            }
            if !self.cursor.empty() {
                let mut value = self.cursor.clone();
                if button == 1 {
                    value.count = 1;
                }
                self.cursor.count -= value.count;
                self.cursor = normalized(self.cursor.clone());
                commands.push(packet(-1, &value)?);
            }
            return Ok(());
        }
        if slot >= 0 && (!self.source.creative_inventory_tab && !(36..45).contains(&slot)) {
            return Err("creative screen slot unavailable".into());
        }
        if mode == ClickMode::Throw && slot >= 0 {
            if button > 1 {
                return Err("creative throw button".into());
            }
            let index = slot as usize;
            let mut value = self.slot(index)?.clone();
            if !value.empty() && self.may_pickup(index)? {
                value.count = if button == 0 {
                    1
                } else {
                    value.count.min(value.max_count()?)
                };
                let mut left = self.slot(index)?.clone();
                left.count -= value.count;
                let replacement = left.empty();
                self.set(index, left, replacement);
                self.on_take(index)?;
                commands.push(packet(-1, &value)?);
            }
        } else if !self.source.creative_inventory_tab && mode == ClickMode::QuickMove && slot >= 0 {
            self.set(slot as usize, Stack::default(), true);
        } else {
            self.click(slot, button, mode)?;
        }
        self.reconcile_input_changes(&before)?;
        for (index, (after, before)) in self.menu.slots.iter().zip(&before).enumerate() {
            if after != before
                || !self.source.creative_inventory_tab
                    && mode == ClickMode::QuickCraft
                    && (button & 3) == 2
                    && (36..45).contains(&index)
            {
                commands.push(packet(
                    index as i16,
                    after.as_ref().ok_or("creative slot not observed")?,
                )?);
            }
        }
        Ok(())
    }
}
