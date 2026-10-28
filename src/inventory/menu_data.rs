//! Server-observed merchant and recipe displays, using pinned 26.1.2 stream layouts.
use super::{Stack, components, read_stack, read_template};
use crate::protocol::{Reader, Result, Writer, content::Content};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::Arc;
#[derive(Clone, Debug, Serialize)]
pub struct DisplayStack {
    pub display_path: String,
    pub stack: Stack,
}
#[derive(Clone, Debug, Serialize)]
pub struct Recipe {
    pub id: i32,
    pub display: Arc<Value>,
    pub group: Option<i32>,
    pub category: String,
    pub ingredients: Option<Vec<Value>>,
    pub flags: u8,
    pub display_stacks: Vec<DisplayStack>,
    pub ingredient_items: Option<Vec<Vec<u32>>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct StonecutterRecipe {
    pub ingredient: Value,
    pub display: Value,
    pub display_stacks: Vec<DisplayStack>,
    pub ingredient_items: Vec<u32>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ItemCost {
    pub stack: Stack,
    pub components: Arc<BTreeMap<String, Value>>,
}
impl ItemCost {
    pub fn matches(&self, stack: &Stack) -> bool {
        !stack.empty()
            && stack.id == self.stack.id
            && self
                .components
                .iter()
                .all(|(k, v)| stack.component(k) == Some(v))
    }
    fn read(r: &mut Reader<'_>, content: &Content) -> Result<Self> {
        let id = r.count(1_000_000)? as u32;
        let count = r.var()?;
        if count < 1 {
            return Err("merchant cost count".into());
        }
        let added = r.count(65536)?;
        let mut patch = Writer::default();
        patch.var(count);
        patch.var(id as i32);
        patch.var(added as i32);
        patch.var(0);
        let mut components = BTreeMap::new();
        for _ in 0..added {
            let start = r.offset;
            let ty = r.var()?;
            let name = content.components.get(&ty).ok_or("merchant component ID")?;
            let value = components::read(r, content, name, 0)?;
            if components.insert(name.clone(), value.json()?).is_some() {
                return Err("duplicate merchant component".into());
            }
            patch.0.extend_from_slice(&r.bytes[start..r.offset]);
        }
        let mut reader = Reader::new(&patch.0);
        let stack = read_stack(&mut reader, content)?;
        reader.done()?;
        Ok(Self {
            stack,
            components: Arc::new(components),
        })
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Trade {
    pub buy_a: ItemCost,
    pub buy_b: Option<ItemCost>,
    pub sell: Stack,
    pub out_of_stock: bool,
    pub uses: i32,
    pub max_uses: i32,
    pub xp: i32,
    pub special_price: i32,
    pub price_multiplier: f32,
    pub demand: i32,
}
impl Trade {
    pub fn cost_a(&self) -> Result<i32> {
        let base = self.buy_a.stack.count;
        let demand = base.wrapping_mul(self.demand) as f32 * self.price_multiplier;
        let demand = demand.floor() as i32;
        Ok(base
            .wrapping_add(demand.max(0))
            .wrapping_add(self.special_price)
            .clamp(1, self.buy_a.stack.max_count()?))
    }
    pub(super) fn accepts(&self, a: &Stack, b: &Stack) -> Result<bool> {
        Ok(self.buy_a.matches(a)
            && a.count >= self.cost_a()?
            && match &self.buy_b {
                Some(cost) => cost.matches(b) && b.count >= cost.stack.count,
                None => b.empty(),
            })
    }
    pub(super) fn available(&self) -> bool {
        !self.out_of_stock && self.uses < self.max_uses
    }
    fn read(r: &mut Reader<'_>, content: &Content) -> Result<Self> {
        let buy_a = ItemCost::read(r, content)?;
        let sell = read_stack(r, content)?;
        let buy_b = if r.bool()? {
            Some(ItemCost::read(r, content)?)
        } else {
            None
        };
        let out_of_stock = r.bool()?;
        let uses = r.i32()?;
        let max_uses = r.i32()?;
        let xp = r.i32()?;
        let special_price = r.i32()?;
        let price_multiplier = r.f32()?;
        let demand = r.i32()?;
        Ok(Self {
            buy_a,
            buy_b,
            sell,
            out_of_stock,
            uses,
            max_uses,
            xp,
            special_price,
            price_multiplier,
            demand,
        })
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Trades {
    pub offers: Vec<Trade>,
    pub level: i32,
    pub xp: i32,
    pub show_progress: bool,
    pub can_restock: bool,
    pub selected: usize,
}
impl Trades {
    fn find_offer(&self, a: &Stack, b: &Stack) -> Result<Option<usize>> {
        if self.selected > 0 && self.selected < self.offers.len() {
            return Ok(self.offers[self.selected]
                .accepts(a, b)?
                .then_some(self.selected));
        }
        for (index, offer) in self.offers.iter().enumerate() {
            if offer.accepts(a, b)? {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }
    pub(super) fn active_offer(&self, a: &Stack, b: &Stack) -> Result<Option<usize>> {
        let empty = Stack::default();
        let (a, b) = if a.empty() { (b, &empty) } else { (a, b) };
        if a.empty() || self.offers.is_empty() {
            return Ok(None);
        }
        let normal = self.find_offer(a, b)?;
        if normal.is_some_and(|i| self.offers[i].available()) {
            return Ok(normal);
        }
        let swapped = self.find_offer(b, a)?;
        Ok(swapped.filter(|&i| self.offers[i].available()).or(normal))
    }
    pub(super) fn read(r: &mut Reader<'_>, content: &Content) -> Result<Self> {
        let count = r.count(65536)?;
        let mut offers = Vec::with_capacity(count);
        for _ in 0..count {
            offers.push(Trade::read(r, content)?);
        }
        Ok(Self {
            offers,
            level: r.var()?,
            xp: r.var()?,
            show_progress: r.bool()?,
            can_restock: r.bool()?,
            selected: 0,
        })
    }
}
impl super::Menu {
    pub(super) fn refresh_active_offer(&mut self) -> Result<()> {
        if self.kind == super::MenuKind::Merchant {
            self.active_offer = match (&self.trades, &self.slots[0], &self.slots[1]) {
                (Some(trades), Some(a), Some(b)) => trades.active_offer(a, b)?,
                _ => None,
            };
        }
        Ok(())
    }
}
pub(super) fn recipe(r: &mut Reader<'_>, content: &Content) -> Result<Recipe> {
    let id = r.var()?;
    let mut display_stacks = Vec::new();
    let display = recipe_display(r, content, &mut display_stacks)?;
    let group = r.var()?;
    let group = if group == 0 { None } else { Some(group - 1) };
    let category = content
        .registry_name("minecraft:recipe_book_category", r.var()?)?
        .to_owned();
    let ingredients = if r.bool()? {
        let count = r.count(65536)?;
        let mut values = Vec::with_capacity(count);
        for _ in 0..count {
            values.push(components::holder_set(r, content, "minecraft:item")?.json()?);
        }
        Some(values)
    } else {
        None
    };
    let ingredient_items = ingredients
        .as_ref()
        .map(|values| {
            values
                .iter()
                .map(|value| resolve_items(value, content))
                .collect::<Result<Vec<_>>>()
        })
        .transpose()?;
    let flags = r.u8()?;
    Ok(Recipe {
        id,
        display: Arc::new(display),
        group,
        category,
        ingredients,
        flags,
        display_stacks,
        ingredient_items,
    })
}
pub(super) fn recipe_display(
    r: &mut Reader<'_>,
    content: &Content,
    stacks: &mut Vec<DisplayStack>,
) -> Result<Value> {
    let ty = content.registry_name("minecraft:recipe_display", r.var()?)?;
    let mut f = serde_json::Map::new();
    f.insert("type".into(), json!(ty));
    let slots: &[&str] = match ty {
        "minecraft:crafting_shaped" => {
            f.insert("width".into(), json!(r.var()?));
            f.insert("height".into(), json!(r.var()?));
            f.insert(
                "ingredients".into(),
                slot_list(r, content, 0, "ingredients", stacks)?,
            );
            &["result", "crafting_station"]
        }
        "minecraft:crafting_shapeless" => {
            f.insert(
                "ingredients".into(),
                slot_list(r, content, 0, "ingredients", stacks)?,
            );
            &["result", "crafting_station"]
        }
        "minecraft:furnace" => &["ingredient", "fuel", "result", "crafting_station"],
        "minecraft:stonecutter" => &["input", "result", "crafting_station"],
        "minecraft:smithing" => &["template", "base", "addition", "result", "crafting_station"],
        _ => return Err(format!("unregistered target recipe display {ty}")),
    };
    for key in slots {
        f.insert((*key).into(), slot_display(r, content, 0, key, stacks)?);
    }
    if ty == "minecraft:furnace" {
        f.insert("duration".into(), json!(r.var()?));
        f.insert("experience".into(), json!(r.f32()?));
    }
    Ok(Value::Object(f))
}
fn slot_list(
    r: &mut Reader<'_>,
    content: &Content,
    depth: u8,
    path: &str,
    stacks: &mut Vec<DisplayStack>,
) -> Result<Value> {
    let count = r.count(65536)?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        values.push(slot_display(
            r,
            content,
            depth + 1,
            &format!("{path}.{index}"),
            stacks,
        )?);
    }
    Ok(Value::Array(values))
}
pub(super) fn slot_display(
    r: &mut Reader<'_>,
    content: &Content,
    depth: u8,
    path: &str,
    stacks: &mut Vec<DisplayStack>,
) -> Result<Value> {
    if depth > 32 {
        return Err("recipe slot display depth".into());
    }
    let ty = content.registry_name("minecraft:slot_display", r.var()?)?;
    let mut f = serde_json::Map::new();
    f.insert("type".into(), json!(ty));
    match ty {
        "minecraft:empty" | "minecraft:any_fuel" => {}
        "minecraft:item" => {
            let id = r.count(1_000_000)? as u32;
            f.insert(
                "item".into(),
                json!(content.item_names.get(&id).ok_or("slot display item ID")?),
            );
        }
        "minecraft:item_stack" => {
            let stack = read_template(r, content)?;
            f.insert("item".into(), stack.persistent_template(content)?.json()?);
            stacks.push(DisplayStack {
                display_path: path.into(),
                stack,
            });
        }
        "minecraft:tag" => {
            f.insert("tag".into(), item_holder_set(r, content)?);
        }
        "minecraft:with_any_potion" => {
            f.insert(
                "contents".into(),
                slot_display(r, content, depth + 1, &format!("{path}.contents"), stacks)?,
            );
        }
        "minecraft:only_with_component" => {
            f.insert(
                "contents".into(),
                slot_display(r, content, depth + 1, &format!("{path}.contents"), stacks)?,
            );
            f.insert(
                "component".into(),
                json!(
                    content
                        .components
                        .get(&r.var()?)
                        .ok_or("display component ID")?
                ),
            );
        }
        "minecraft:dyed" => {
            for key in ["dye", "target"] {
                f.insert(
                    key.into(),
                    slot_display(r, content, depth + 1, &format!("{path}.{key}"), stacks)?,
                );
            }
        }
        "minecraft:smithing_trim" => {
            for key in ["base", "material"] {
                f.insert(
                    key.into(),
                    slot_display(r, content, depth + 1, &format!("{path}.{key}"), stacks)?,
                );
            }
            f.insert(
                "pattern".into(),
                components::trim_pattern(r, content)?.json()?,
            );
        }
        "minecraft:with_remainder" => {
            for key in ["input", "remainder"] {
                f.insert(
                    key.into(),
                    slot_display(r, content, depth + 1, &format!("{path}.{key}"), stacks)?,
                );
            }
        }
        "minecraft:composite" => {
            f.insert(
                "contents".into(),
                slot_list(r, content, depth, &format!("{path}.contents"), stacks)?,
            );
        }
        _ => return Err(format!("unregistered target slot display {ty}")),
    }
    Ok(Value::Object(f))
}
fn item_holder_set(r: &mut Reader<'_>, content: &Content) -> Result<Value> {
    let length = r.count(1_000_000)?;
    if length == 0 {
        return Ok(json!(format!("#{}", r.string(32767)?)));
    }
    let mut items = Vec::with_capacity(length - 1);
    for _ in 1..length {
        let id = r.count(1_000_000)? as u32;
        items.push(json!(
            content
                .item_names
                .get(&id)
                .ok_or("slot display item holder ID")?
        ));
    }
    Ok(Value::Array(items))
}
pub(super) fn resolve_items(value: &Value, content: &Content) -> Result<Vec<u32>> {
    match value {
        Value::String(tag) if tag.starts_with('#') => content
            .registry_tag("minecraft:item", tag)?
            .iter()
            .map(|id| u32::try_from(*id).map_err(|_| "negative item holder".into()))
            .collect(),
        Value::String(name) => content
            .item_names
            .iter()
            .find_map(|(id, item)| (name == item).then_some(vec![*id]))
            .ok_or_else(|| format!("unknown item holder {name}")),
        Value::Array(values) => {
            let mut items = Vec::new();
            for value in values {
                items.extend(resolve_items(value, content)?);
            }
            items.sort_unstable();
            items.dedup();
            Ok(items)
        }
        _ => Err("invalid item holder set".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_slot_display_decodes_26_3_named_item_holder_set() {
        let content = Content::test_cube();
        let mut bytes = vec![0, 20];
        bytes.extend(b"minecraft:birch_logs");
        let tag = item_holder_set(&mut Reader::new(&bytes), &content).unwrap();
        assert_eq!(tag, json!("#minecraft:birch_logs"));
    }

    #[test]
    fn tag_slot_display_decodes_26_3_direct_item_holder_set() {
        let mut content = Content::test_cube();
        content.item_names.insert(4, "minecraft:birch_log".into());
        content.item_names.insert(7, "minecraft:stripped_birch_log".into());
        let tag = item_holder_set(&mut Reader::new(&[3, 4, 7]), &content).unwrap();
        assert_eq!(
            tag,
            json!(["minecraft:birch_log", "minecraft:stripped_birch_log"])
        );
    }
}
