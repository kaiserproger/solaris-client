use super::{InventoryStore, Menu, MenuKind, StonecutterRecipe, Trades, components, menu_data};
use crate::protocol::{Reader, Result, content::Content};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;

impl InventoryStore {
    pub(super) fn apply_menu_data(
        &mut self,
        id: i32,
        r: &mut Reader<'_>,
        content: &Content,
    ) -> Result<bool> {
        match id {
            41 => {
                let container = r.var()?;
                let columns = r.count(5)? as u8;
                let entity = r.i32()?;
                r.done()?;
                if container <= 0 {
                    return Err("mount container ID".into());
                }
                let mut menu = Menu::new(
                    container,
                    MenuKind::Horse,
                    json!({"translate":"container.horse"}),
                );
                menu.slots = vec![None; 38 + 3 * columns as usize];
                menu.horse_columns = Some(columns);
                menu.entity_id = Some(entity);
                self.received_active = Some(menu.clone());
                self.active = Some(menu);
                self.screen_open = true;
                self.screen_instance += 1;
                self.drag = super::actions::QuickCraft::default();
                self.ghost_recipe = None;
                self.ghost_recipe_stacks.clear();
            }
            52 => {
                let container = r.var()?;
                let trades = Arc::new(Trades::read(r, content)?);
                r.done()?;
                if let Some(menu) = self
                    .active
                    .as_mut()
                    .filter(|m| m.id == container && m.kind == MenuKind::Merchant)
                {
                    menu.trades = Some(trades.clone());
                    menu.refresh_active_offer()?;
                }
                if let Some(menu) = self
                    .received_active
                    .as_mut()
                    .filter(|m| m.id == container && m.kind == MenuKind::Merchant)
                {
                    menu.trades = Some(trades);
                    menu.refresh_active_offer()?;
                }
            }
            63 => {
                let container = r.var()?;
                let mut stacks = Vec::new();
                let display = menu_data::recipe_display(r, content, &mut stacks)?;
                r.done()?;
                if self.menu().id == container {
                    self.ghost_recipe = Some((container, display));
                    self.ghost_recipe_stacks = stacks;
                }
            }
            74 => {
                let count = r.count(65536)?;
                let mut entries = BTreeMap::new();
                for _ in 0..count {
                    let recipe = menu_data::recipe(r, content)?;
                    if entries.insert(recipe.id, recipe).is_some() {
                        return Err("duplicate recipe display ID".into());
                    }
                }
                let replace = r.bool()?;
                r.done()?;
                if replace {
                    self.recipes = entries;
                } else {
                    self.recipes.extend(entries);
                }
            }
            75 => {
                let count = r.count(65536)?;
                let mut ids = Vec::with_capacity(count);
                for _ in 0..count {
                    ids.push(r.var()?);
                }
                r.done()?;
                for id in ids {
                    self.recipes.remove(&id);
                }
            }
            76 => {
                let mut settings = [(false, false); 4];
                for settings in &mut settings {
                    *settings = (r.bool()?, r.bool()?);
                }
                r.done()?;
                self.recipe_book_settings = settings;
            }
            103 => {
                r.f32()?;
                let level = r.var()?;
                r.var()?;
                r.done()?;
                self.experience_level = level;
            }
            133 => {
                let mut sets = BTreeMap::new();
                for _ in 0..r.count(65536)? {
                    let name = r.string(32767)?;
                    let mut items = Vec::new();
                    for _ in 0..r.count(65536)? {
                        let id = r.count(1_000_000)? as u32;
                        if !content.item_names.contains_key(&id) {
                            return Err("recipe property item ID".into());
                        }
                        items.push(id);
                    }
                    if sets.insert(name, items).is_some() {
                        return Err("duplicate recipe property set".into());
                    }
                }
                let mut recipes = Vec::new();
                for _ in 0..r.count(65536)? {
                    let ingredient =
                        components::holder_set(r, content, "minecraft:item")?.json()?;
                    let ingredient_items = menu_data::resolve_items(&ingredient, content)?;
                    let mut display_stacks = Vec::new();
                    let display =
                        menu_data::slot_display(r, content, 0, "result", &mut display_stacks)?;
                    recipes.push(StonecutterRecipe {
                        ingredient,
                        display,
                        display_stacks,
                        ingredient_items,
                    });
                }
                r.done()?;
                self.recipe_property_sets = sets;
                self.stonecutter_recipes = recipes;
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
    pub fn configure_mount(&mut self, entity_id: i32, kind: &str) -> Result<()> {
        let menu = self
            .active
            .as_mut()
            .filter(|m| m.kind == MenuKind::Horse && m.entity_id == Some(entity_id))
            .ok_or("stale_mount_menu")?;
        if !matches!(
            kind,
            "minecraft:horse"
                | "minecraft:donkey"
                | "minecraft:mule"
                | "minecraft:llama"
                | "minecraft:trader_llama"
                | "minecraft:skeleton_horse"
                | "minecraft:zombie_horse"
                | "minecraft:camel"
                | "minecraft:nautilus"
                | "minecraft:zombie_nautilus"
        ) {
            return Err("unknown source mount menu kind".into());
        }
        menu.mount_kind = Some(kind.into());
        if matches!(kind, "minecraft:nautilus" | "minecraft:zombie_nautilus") {
            menu.horse_columns = Some(0);
            menu.slots.resize(38, None);
        }
        if let Some(received) = &mut self.received_active {
            received.mount_kind = menu.mount_kind.clone();
            received.horse_columns = menu.horse_columns;
            received.slots.resize(menu.slots.len(), None);
        }
        Ok(())
    }
}
