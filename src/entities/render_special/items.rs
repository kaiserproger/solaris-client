use super::*;

fn observed_stack(content: &Content, value: &Value) -> Result<Option<Stack>> {
    let Some(id) = value.get("id").and_then(Value::as_str) else {
        return if value.is_null() || value.as_object().is_some_and(|o| o.is_empty()) {
            Ok(None)
        } else {
            Err("observed item has no namespaced id".into())
        };
    };
    let count = value.get("count").and_then(Value::as_i64).unwrap_or(1);
    if count == 0 || id == "minecraft:air" {
        return Ok(None);
    }
    let count = i32::try_from(count).map_err(|_| "observed item count bound")?;
    let id = content
        .item_names
        .iter()
        .find_map(|(key, name)| (name == id).then_some(*key))
        .ok_or_else(|| format!("observed item registry missing {id}"))?;
    let components = match value.get("components") {
        None => BTreeMap::new(),
        Some(Value::Object(values)) => values
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
        _ => return Err("observed item components object required".into()),
    };
    crate::inventory::creative_stack_with_components(content, id, count, &components).map(Some)
}
impl Cache {
    fn observed_stack<'a>(
        &'a mut self,
        content: &Content,
        value: &Value,
    ) -> Result<Option<&'a Stack>> {
        let key = value as *const Value as usize;
        let changed = self
            .observed_items
            .get(&key)
            .is_none_or(|cached| cached.source != *value);
        if changed {
            // Source NBT is immutable within its revision. Retain only cold-bound stacks;
            // pointer reuse is fenced by the owned source value, never by a raw address alone.
            if self.observed_items.len() >= 4096 && !self.observed_items.contains_key(&key) {
                self.observed_items.pop_first();
            }
            let stack = observed_stack(content, value)?;
            self.observed_items.insert(
                key,
                ObservedItem {
                    source: value.clone(),
                    stack,
                },
            );
        }
        Ok(self.observed_items[&key].stack.as_ref())
    }
}
pub(crate) fn append_observed_item(
    content: &Content,
    observed: &Value,
    context: &str,
    seed: i32,
    root: Mat4,
    light: [f32; 2],
    items: &mut ItemModels,
    cache: &mut Cache,
    out: &mut DynamicGeometry,
) -> Result<()> {
    let Some(stack) = cache.observed_stack(content, observed)? else {
        return Ok(());
    };
    let display = match context {
        "ground_cluster" => "ground",
        "on_shelf_bottom" | "on_shelf_center" => "on_shelf",
        value => value,
    };
    let state = ItemContext {
        display: display.into(),
        seed,
        ..Default::default()
    };
    // A stack whose model cannot be baked is drawn as nothing (logged once), never an error.
    let (model, bounds) = match items.model_with_bounds(content, stack, &state) {
        Ok(baked) => baked,
        Err(error) => {
            crate::ui::items::report_item_model_failure(content, stack, &error);
            return Ok(());
        }
    };
    let matrix = crate::ui::items::display_matrix(model, display);
    if context == "ground_cluster" {
        let damage = stack
            .component("minecraft:damage")
            .and_then(Value::as_i64)
            .unwrap_or(0) as i32;
        let seed = (stack.id as i32).wrapping_add(damage);
        let offsets = kernels::cluster_offsets(seed, stack.count, bounds[5] - bounds[2]);
        for offset in offsets.iter().take(kernels::item_amount(stack.count)) {
            p::append_baked(
                model,
                root * Mat4::from_translation(*offset) * matrix,
                light,
                out,
            )?;
        }
    } else {
        let offset = if context == "on_shelf_bottom" {
            -bounds[1]
        } else if context == "on_shelf_center" {
            -(bounds[1] + bounds[4]) * 0.5
        } else {
            0.
        };
        p::append_baked(
            model,
            root * Mat4::from_translation(Vec3::new(0., offset, 0.)) * matrix,
            light,
            out,
        )?;
    }
    Ok(())
}
