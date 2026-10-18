//! Recipe displays use ordinary canonical Stack patches, including demo transformations.
use crate::{
    inventory::{DisplayStack, Stack, creative_stack, stack_with_components},
    protocol::{Result, content::Content},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
pub(crate) fn display_variants(
    v: &Value,
    path: &str,
    templates: &[DisplayStack],
    content: &Content,
) -> Result<Vec<Stack>> {
    resolve(v, path, templates, content, 0)
}
fn resolve(
    v: &Value,
    path: &str,
    templates: &[DisplayStack],
    c: &Content,
    depth: u8,
) -> Result<Vec<Stack>> {
    if depth > 32 {
        return Err("recipe_display_depth".into());
    }
    let child = |key: &str| resolve(&v[key], &format!("{path}.{key}"), templates, c, depth + 1);
    let item = |name: &str| -> Result<Stack> {
        let id = c
            .item_names
            .iter()
            .find_map(|(id, value)| (value == name).then_some(*id))
            .ok_or_else(|| format!("recipe_item_not_observed:{name}"))?;
        creative_stack(c, id, 1)
    };
    let mut result = match v["type"].as_str().ok_or("recipe_display_type_required")? {
        "minecraft:empty" => Vec::new(),
        "minecraft:item" => vec![item(v["item"].as_str().ok_or("recipe_item_required")?)?],
        "minecraft:item_stack" => vec![
            templates
                .iter()
                .find(|entry| entry.display_path == path)
                .ok_or("recipe_template_not_retained")?
                .stack
                .clone(),
        ],
        "minecraft:tag" => {
            let mut values = Vec::new();
            for id in c
                .registry_tag_optional(
                    "minecraft:item",
                    v["tag"].as_str().ok_or("recipe_tag_required")?,
                )?
                .into_iter()
                .flatten()
            {
                values.push(creative_stack(
                    c,
                    u32::try_from(*id).map_err(|_| "negative_recipe_item")?,
                    1,
                )?);
            }
            values
        }
        "minecraft:composite" => {
            let mut values = Vec::new();
            for (index, value) in v["contents"]
                .as_array()
                .ok_or("recipe_composite_required")?
                .iter()
                .enumerate()
            {
                values.extend(resolve(
                    value,
                    &format!("{path}.contents.{index}"),
                    templates,
                    c,
                    depth + 1,
                )?);
                if values.len() > 4096 {
                    return Err("recipe_display_variant_limit".into());
                }
            }
            values
        }
        "minecraft:with_remainder" => child("input")?,
        "minecraft:only_with_component" => {
            let key = v["component"].as_str().ok_or("recipe_component_required")?;
            child("contents")?
                .into_iter()
                .filter(|value| value.components.contains_key(key))
                .collect()
        }
        "minecraft:with_any_potion" => {
            let base = child("contents")?;
            let mut values = Vec::new();
            for (potion, _) in c.registry_entries("minecraft:potion")? {
                for stack in &base {
                    values.push(patch(
                        c,
                        stack,
                        "minecraft:potion_contents",
                        json!({"potion":potion}),
                        false,
                    )?);
                    if values.len() > 4096 {
                        return Err("recipe_display_variant_limit".into());
                    }
                }
            }
            values
        }
        "minecraft:dyed" => {
            let base = child("target")?;
            let dyes = child("dye")?;
            let mut values = Vec::new();
            'outer: for dye in &dyes {
                let color = dye_color(
                    dye.components
                        .get("minecraft:dye")
                        .and_then(Value::as_str)
                        .unwrap_or("white"),
                )?;
                for stack in &base {
                    let old = stack
                        .components
                        .get("minecraft:dyed_color")
                        .and_then(Value::as_u64)
                        .map(|v| v as u32);
                    values.push(patch(
                        c,
                        stack,
                        "minecraft:dyed_color",
                        json!(mix_dye(old, color)),
                        true,
                    )?);
                    if values.len() == 16 {
                        break 'outer;
                    }
                }
            }
            values
        }
        "minecraft:smithing_trim" => {
            let base = child("base")?;
            let materials = child("material")?;
            let pattern = v.get("pattern").ok_or("recipe_trim_pattern_required")?;
            super::sample::demo(
                v as *const Value as usize as i32,
                base.len(),
                materials.len(),
                |i, j| {
                    let stack = &base[i];
                    let Some(material) = materials[j]
                        .components
                        .get("minecraft:provides_trim_material")
                    else {
                        return Ok(None);
                    };
                    let trim = json!({"material":material,"pattern":pattern});
                    if stack.components.get("minecraft:trim") == Some(&trim) {
                        return Ok(None);
                    }
                    patch(c, stack, "minecraft:trim", trim, true).map(Some)
                },
            )?
        }
        "minecraft:any_fuel" => c
            .fuel_items()
            .iter()
            .map(|&id| creative_stack(c, id, 1))
            .collect::<Result<_>>()?,
        name => return Err(format!("unregistered_recipe_display:{name}")),
    };
    result.retain(|value| !value.empty());
    if result.len() > 4096 {
        return Err("recipe_display_variant_limit".into());
    }
    Ok(result)
}
fn patch(c: &Content, stack: &Stack, key: &str, value: Value, single: bool) -> Result<Stack> {
    let mut base = stack.clone();
    if single {
        base.count = 1;
    }
    stack_with_components(c, &base, &BTreeMap::from([(key.to_owned(), value)]))
}
// DyeFacts queried DyeColor.getTextureDiffuseColor on the pinned target; ARGB alpha is immaterial here.
fn dye_color(name: &str) -> Result<u32> {
    Ok(match name {
        "white" => -393218i32,
        "orange" => -425955,
        "magenta" => -3715395,
        "light_blue" => -12930086,
        "yellow" => -75715,
        "lime" => -8337633,
        "pink" => -816214,
        "gray" => -12103854,
        "light_gray" => -6447721,
        "cyan" => -15295332,
        "purple" => -7785800,
        "blue" => -12827478,
        "brown" => -8170446,
        "green" => -10585066,
        "red" => -5231066,
        "black" => -14869215,
        _ => return Err("unregistered_dye_color".into()),
    } as u32
        & 0xffffff)
}
fn mix_dye(old: Option<u32>, dye: u32) -> u32 {
    let channels = |color: u32| [(color >> 16) & 255, (color >> 8) & 255, color & 255];
    let mut rgb = channels(dye);
    let mut brightness = *rgb.iter().max().unwrap();
    let count = if let Some(old) = old {
        let previous = channels(old);
        brightness += *previous.iter().max().unwrap();
        for i in 0..3 {
            rgb[i] += previous[i];
        }
        2
    } else {
        1
    };
    for value in &mut rgb {
        *value /= count;
    }
    let maximum = *rgb.iter().max().unwrap() as f32;
    let intensity = brightness as f32 / count as f32;
    if maximum > 0. {
        for value in &mut rgb {
            *value = (*value as f32 * intensity / maximum) as u32;
        }
    }
    (rgb[0] << 16) | (rgb[1] << 8) | rgb[2]
}
