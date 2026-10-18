use super::*;

impl Player {
    pub(in crate::player) fn queue_use_ui(&mut self, trigger: UiTrigger) -> Result<()> {
        if self.ordinary_ui.len() >= 16 {
            return Err("ordinary_ui_trigger_queue_bound".into());
        }
        self.ordinary_ui.push_back(trigger);
        Ok(())
    }

    pub(in crate::player) fn use_block_result(
        &mut self,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        if self.game_mode == 3 {
            return Ok(Outcome::CONSUME);
        }
        let item = super::super::interaction::held(inventory, hand).filter(|item| !item.empty());
        let carrying = (0..=1).any(|hand| {
            super::super::interaction::held(inventory, hand).is_some_and(|item| !item.empty())
        });
        if self.last_sent_input & 32 == 0 || !carrying {
            if !content
                .block_required_features(hit.state)?
                .iter()
                .all(|feature| self.enabled_features.contains(feature))
            {
                return Ok(Outcome::Fail);
            }
            let (item_owner, empty_owner) = content.block_use_owners(hit.state)?;
            if short_class(item_owner) == "RedStoneOreBlock" {
                self.redstone_ore_particles(hit.position, world, content)?;
            }
            let result =
                self.block_item_result(item_owner, hit, hand, world, content, inventory, entities)?;
            if result.consumes() {
                self.predict_block_interaction(
                    item_owner, hit, hand, false, world, content, inventory, entities, tick,
                )?;
                return Ok(result);
            }
            if result == Outcome::TryEmptyHand && hand == 0 {
                let result =
                    self.block_empty_result(empty_owner, hit, world, content, inventory, entities)?;
                if result.consumes() {
                    self.predict_block_interaction(
                        empty_owner,
                        hit,
                        hand,
                        true,
                        world,
                        content,
                        inventory,
                        entities,
                        tick,
                    )?;
                    return Ok(result);
                }
            }
        }
        let Some(item) = item else {
            return Ok(Outcome::Pass);
        };
        if self.on_cooldown(item, content, tick)? {
            return Ok(Outcome::Pass);
        }
        if !self.abilities.may_build
            && !super::super::adventure::matches(
                Some(item),
                "minecraft:can_place_on",
                hit,
                world,
                entities,
                content,
            )?
        {
            return Ok(Outcome::Pass);
        }
        self.item_on_result(hit, hand, world, content, inventory, entities, tick)
    }

    pub(super) fn item_on_result(
        &mut self,
        hit: &BlockHit,
        hand: u8,
        world: &World,
        content: &Content,
        inventory: &InventoryStore,
        entities: &EntityStore,
        tick: u64,
    ) -> Result<Outcome> {
        let Some(item) =
            super::super::interaction::held(inventory, hand).filter(|item| !item.empty())
        else {
            return Ok(Outcome::Pass);
        };
        let owner = content.item_use_owner(item.id, true)?;
        let first_prediction = self.use_predictions.len();
        let result =
            self.source_item_on(owner, hit, hand, world, content, inventory, entities, tick)?;
        if self.abilities.instabuild {
            let slot = if hand == 1 {
                45
            } else {
                36 + u16::from(inventory.selected_slot)
            };
            for prediction in self.use_predictions.iter_mut().skip(first_prediction) {
                if let UsePrediction::PlayerSlot {
                    slot: changed,
                    stack,
                    replacement: false,
                } = prediction
                {
                    if *changed == slot {
                        if let Some(stack) = stack {
                            stack.count = item.count;
                        } else {
                            *stack = Some(item.clone());
                        }
                    }
                }
            }
        }
        // ItemStack.useOn does not apply the air/finish-use remainder and cooldown callback.
        Ok(result)
    }
}
