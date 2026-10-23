use super::state::{NodeAction, Paint, UiState};
use crate::{
    inventory::InventoryStore,
    protocol::{Result, content::Content},
};
impl UiState {
    pub(crate) fn recipe_tree(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
        x: f32,
        y: f32,
    ) -> Result<()> {
        let search = self
            .fields
            .get("recipe/search")
            .map(|f| f.text.to_lowercase())
            .unwrap_or_default();
        let book = inventory
            .menu()
            .kind
            .recipe_book()
            .ok_or("recipe_book_unavailable")?;
        let relevant = |category: &str| match book {
            0 => category.starts_with("minecraft:crafting_"),
            1 => category.starts_with("minecraft:furnace_"),
            2 => category.starts_with("minecraft:blast_furnace_"),
            3 => category.starts_with("minecraft:smoker_"),
            _ => false,
        };
        let categories: std::collections::BTreeSet<_> = inventory
            .recipes
            .values()
            .filter(|r| relevant(&r.category))
            .map(|r| r.category.clone())
            .collect();
        let category_rows: Vec<_> = std::iter::once(String::new()).chain(categories).collect();
        for (index, category) in category_rows.into_iter().enumerate() {
            self.button(
                &format!("recipe/category/{category}"),
                if category.is_empty() {
                    "Search"
                } else {
                    category.strip_prefix("minecraft:").unwrap_or(&category)
                },
                [x - 30., y + index as f32 * 22., 28., 20.],
                NodeAction::RecipeCategory(category.clone()),
                true,
                None,
            )?;
        }
        let rows = ((self.height - y - 40.) / 24.).clamp(0., 8.) as usize;
        let mut entries = Vec::new();
        for recipe in inventory.recipes.values().filter(|r| relevant(&r.category)) {
            if self
                .recipe_category
                .as_ref()
                .is_some_and(|c| c != &recipe.category)
            {
                continue;
            }
            let craftable = inventory.recipe_craftable(recipe.id)?;
            if self.recipe_filter && craftable != Some(true) {
                continue;
            }
            if !self
                .recipe_names
                .get(&recipe.id)
                .is_some_and(|(source, _)| std::sync::Arc::ptr_eq(source, &recipe.display))
            {
                let variants = super::display::display_variants(
                    &recipe.display["result"],
                    "result",
                    &recipe.display_stacks,
                    content,
                )?;
                let mut names = std::collections::BTreeSet::new();
                for stack in &variants {
                    names.insert(
                        self.text
                            .language
                            .plain(super::text::item_name_component(content, stack)?.as_ref())?,
                    );
                }
                self.recipe_names.insert(
                    recipe.id,
                    (
                        std::sync::Arc::clone(&recipe.display),
                        names.into_iter().collect::<Vec<_>>().join("\n"),
                    ),
                );
            }
            let label = self.recipe_names[&recipe.id].1.clone();
            if !search.is_empty() && !label.to_lowercase().contains(&search) {
                continue;
            }
            entries.push((recipe.id, label, craftable));
        }
        for (i, (id, label, craftable)) in entries
            .into_iter()
            .skip(self.scroll * 5)
            .take(rows * 5)
            .enumerate()
        {
            self.add(
                format!("recipe/{id}"),
                "button",
                format!(
                    "{label}\n{}",
                    match craftable {
                        Some(true) => "Craftable",
                        Some(false) => "Missing ingredients",
                        None => "Craftability not observed",
                    }
                ),
                [x + (i % 5) as f32 * 28., y + (i / 5) as f32 * 24., 26., 22.],
                true,
                None,
                NodeAction::Recipe(id),
                Paint::RecipeDisplay(id),
            )?;
        }
        self.button(
            "recipe/filter",
            if self.recipe_filter {
                "Craftable only"
            } else {
                "All recipes"
            },
            [x, y + rows as f32 * 24. + 4., 140., 20.],
            NodeAction::RecipeFilter,
            true,
            None,
        )?;
        Ok(())
    }
    pub(crate) fn trade_tree(&mut self, inventory: &InventoryStore, x: f32, y: f32) -> Result<()> {
        let Some(trades) = &inventory.menu().trades else {
            return Ok(());
        };
        for (i, trade) in trades.offers.iter().enumerate().skip(self.scroll).take(7) {
            let row = y + 18. + (i - self.scroll) as f32 * 20.;
            let enabled = !trade.out_of_stock && trade.uses < trade.max_uses;
            self.button(
                &format!("menu/button/{i}"),
                "",
                [x + 5., row, 88., 20.],
                NodeAction::MenuButton(i as i32),
                enabled,
                if enabled {
                    None
                } else {
                    Some("trade_out_of_stock")
                },
            )?;
            for part in 0..3 {
                if part == 1 && trade.buy_b.is_none() {
                    continue;
                }
                self.add(
                    format!("trade/{i}/item/{part}"),
                    "image",
                    String::new(),
                    [x + 8. + f32::from(part) * 25., row + 2., 16., 16.],
                    true,
                    None,
                    NodeAction::None,
                    Paint::TradeItem { offer: i, part },
                )?;
            }
            self.label(
                &format!("trade/{i}/cost"),
                trade.cost_a()?.to_string(),
                [x + 18., row + 9., 12., 9.],
            )?;
        }
        if trades.show_progress {
            self.label(
                "trade/level",
                format!("Level {} — {} XP", trades.level, trades.xp),
                [x + 108., y + 6., 160., 10.],
            )?;
        }
        Ok(())
    }
}
