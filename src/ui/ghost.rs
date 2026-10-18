use super::{
    layout::Layout,
    state::{NodeAction, Paint, UiState},
};
use crate::{
    inventory::{InventoryStore, MenuKind},
    protocol::{Result, content::Content},
};
use std::sync::Arc;
impl UiState {
    pub(crate) fn ghost_tree(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
        layout: &Layout,
        x: f32,
        y: f32,
    ) -> Result<()> {
        let Some((container, display)) = &inventory.ghost_recipe else {
            self.ghost_presentation = None;
            return Ok(());
        };
        if *container != inventory.menu().id {
            return Err("stale_ghost_container".into());
        }
        if !self
            .ghost_presentation
            .as_ref()
            .is_some_and(|(source, _)| source == display)
        {
            let mut slots = Vec::new();
            let mut add = |slot: usize, value: &serde_json::Value, path: &str| -> Result<()> {
                let variants = super::display::display_variants(
                    value,
                    path,
                    &inventory.ghost_recipe_stacks,
                    content,
                )?;
                if !variants.is_empty() {
                    slots.push((slot, Arc::new(variants)));
                }
                Ok(())
            };
            match inventory.menu().kind {
                MenuKind::Inventory | MenuKind::Crafting => {
                    let grid = if inventory.menu().kind == MenuKind::Inventory {
                        2
                    } else {
                        3
                    };
                    let ingredients = display["ingredients"]
                        .as_array()
                        .ok_or("ghost_ingredients_required")?;
                    if ingredients.len() > grid * grid {
                        return Err("ghost_ingredient_bound".into());
                    }
                    let targets = if display["type"] == "minecraft:crafting_shaped" {
                        let width =
                            display["width"].as_u64().ok_or("ghost_width_required")? as usize;
                        let height =
                            display["height"].as_u64().ok_or("ghost_height_required")? as usize;
                        if width.checked_mul(height) != Some(ingredients.len()) {
                            return Err("ghost_ingredient_dimensions".into());
                        }
                        super::recipe_grid::shaped_slots(grid, width, height)?
                    } else if display["type"] == "minecraft:crafting_shapeless" {
                        (1..=ingredients.len()).collect()
                    } else {
                        return Err("ghost_recipe_menu_mismatch".into());
                    };
                    add(0, &display["result"], "result")?;
                    for (index, (&slot, ingredient)) in targets.iter().zip(ingredients).enumerate()
                    {
                        add(slot, ingredient, &format!("ingredients.{index}"))?;
                    }
                }
                MenuKind::Furnace | MenuKind::BlastFurnace | MenuKind::Smoker
                    if display["type"] == "minecraft:furnace" =>
                {
                    add(2, &display["result"], "result")?;
                    add(0, &display["ingredient"], "ingredient")?;
                    add(1, &display["fuel"], "fuel")?;
                }
                _ => return Err("ghost_recipe_menu_mismatch".into()),
            }
            self.ghost_presentation = Some((display.clone(), slots));
        }
        let slots = self
            .ghost_presentation
            .as_ref()
            .expect("prepared ghost")
            .1
            .clone();
        for (index, variants) in slots {
            if index == 1
                && matches!(
                    inventory.menu().kind,
                    MenuKind::Furnace | MenuKind::BlastFurnace | MenuKind::Smoker
                )
                && !inventory.menu().slots[1]
                    .as_ref()
                    .is_some_and(|s| s.empty())
            {
                continue;
            }
            let slot = layout
                .slots
                .iter()
                .find(|slot| slot.index == index)
                .ok_or("ghost_slot_not_in_layout")?;
            // Ghosts do not expose click-slot handles: ordinary inventory slots retain the action.
            self.add(
                format!("recipe/ghost/{index}"),
                "image",
                String::new(),
                [x + slot.x, y + slot.y, 16., 16.],
                true,
                None,
                NodeAction::None,
                Paint::Variants(variants, true),
            )?;
        }
        Ok(())
    }
}
