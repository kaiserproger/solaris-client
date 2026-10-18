//! Persistent preset components are encoded against the received registry IDs,
//! then decoded by the ordinary retained Stack codec. No display-only overrides.
use super::{Stack, Tag, nbt, predicates};
use crate::protocol::{Reader, Result, Writer, content::Content};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

fn integer(v: &Value) -> Result<i32> {
    i32::try_from(v.as_i64().ok_or("component integer")?)
        .map_err(|_| "component integer bound".into())
}
fn text(v: &Value) -> Result<&str> {
    v.as_str().ok_or("component string".into())
}
fn array(v: &Value) -> Result<&[Value]> {
    v.as_array()
        .map(Vec::as_slice)
        .ok_or("component list".into())
}
fn object(v: &Value) -> Result<&serde_json::Map<String, Value>> {
    v.as_object().ok_or("component object".into())
}
fn number(v: Option<&Value>, default: i32) -> Result<i32> {
    v.map(integer).transpose().map(|v| v.unwrap_or(default))
}
fn flag(v: Option<&Value>, default: bool) -> Result<bool> {
    v.map(|v| v.as_bool().ok_or("component boolean".into()))
        .transpose()
        .map(|v| v.unwrap_or(default))
}
fn registry_id(c: &Content, registry: &str, name: &str) -> Result<i32> {
    c.registry_entries(registry)?
        .iter()
        .position(|(key, _)| key == name)
        .map(|i| i as i32)
        .ok_or_else(|| format!("unknown received {registry} holder {name}"))
}
fn holder(w: &mut Writer, c: &Content, registry: &str, v: &Value, inline: bool) -> Result<()> {
    w.var(registry_id(c, registry, text(v)?)? + i32::from(inline));
    Ok(())
}
fn enumeration(w: &mut Writer, v: &Value, values: &[&str]) -> Result<()> {
    w.var(
        values
            .iter()
            .position(|s| *s == text(v).unwrap_or(""))
            .ok_or("component enum")? as i32,
    );
    Ok(())
}
const COLORS: &[&str] = &[
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];
fn nbt_json(v: &Value) -> Result<Tag> {
    Ok(match v {
        Value::Bool(b) => Tag::Byte(i8::from(*b)),
        Value::Array(a) => Tag::List(a.iter().map(nbt_json).collect::<Result<_>>()?),
        Value::Object(o) => Tag::record(
            o.iter()
                .map(|(k, v)| Ok((k, nbt_json(v)?)))
                .collect::<Result<Vec<_>>>()?,
        ),
        _ => predicates::from_json(v)?,
    })
}
fn effect(w: &mut Writer, c: &Content, v: &Value, depth: u8) -> Result<()> {
    holder(
        w,
        c,
        "minecraft:mob_effect",
        v.get("id").ok_or("effect id")?,
        false,
    )?;
    effect_details(w, v, depth)
}
fn effect_details(w: &mut Writer, v: &Value, depth: u8) -> Result<()> {
    if depth > 32 {
        return Err("effect nesting bound".into());
    }
    w.var(number(v.get("amplifier"), 0)?);
    w.var(number(v.get("duration"), 0)?);
    w.byte(u8::from(flag(v.get("ambient"), false)?));
    let particles = flag(v.get("show_particles"), true)?;
    w.byte(u8::from(particles));
    w.byte(u8::from(flag(v.get("show_icon"), particles)?));
    w.byte(u8::from(v.get("hidden_effect").is_some()));
    if let Some(v) = v.get("hidden_effect") {
        effect_details(w, v, depth + 1)?;
    }
    Ok(())
}
fn explosion(w: &mut Writer, v: &Value) -> Result<()> {
    enumeration(
        w,
        v.get("shape").ok_or("firework shape")?,
        &["small_ball", "large_ball", "star", "creeper", "burst"],
    )?;
    for key in ["colors", "fade_colors"] {
        let vals = v.get(key).map(array).transpose()?.unwrap_or(&[]);
        w.var(vals.len() as i32);
        for v in vals {
            w.i32(integer(v)?);
        }
    }
    w.byte(u8::from(flag(v.get("has_trail"), false)?));
    w.byte(u8::from(flag(v.get("has_twinkle"), false)?));
    Ok(())
}
fn encode(w: &mut Writer, c: &Content, name: &str, v: &Value) -> Result<()> {
    match name {
        "minecraft:stored_enchantments" | "minecraft:enchantments" => {
            let vals = object(v)?;
            w.var(vals.len() as i32);
            for (name, level) in vals {
                w.var(registry_id(c, "minecraft:enchantment", name)?);
                w.var(integer(level)?);
            }
        }
        "minecraft:potion_contents" => {
            w.byte(u8::from(v.get("potion").is_some()));
            if let Some(v) = v.get("potion") {
                holder(w, c, "minecraft:potion", v, false)?;
            }
            w.byte(u8::from(v.get("custom_color").is_some()));
            if let Some(v) = v.get("custom_color") {
                w.i32(integer(v)?);
            }
            let effects = v
                .get("custom_effects")
                .map(array)
                .transpose()?
                .unwrap_or(&[]);
            w.var(effects.len() as i32);
            for v in effects {
                effect(w, c, v, 0)?;
            }
            w.byte(u8::from(v.get("custom_name").is_some()));
            if let Some(v) = v.get("custom_name") {
                w.string(text(v)?);
            }
        }
        "minecraft:instrument" => holder(w, c, "minecraft:instrument", v, true)?,
        "minecraft:painting/variant" => holder(w, c, "minecraft:painting_variant", v, true)?,
        "minecraft:suspicious_stew_effects" => {
            let vals = array(v)?;
            w.var(vals.len() as i32);
            for v in vals {
                holder(
                    w,
                    c,
                    "minecraft:mob_effect",
                    v.get("id").ok_or("stew id")?,
                    false,
                )?;
                w.var(number(v.get("duration"), 160)?);
            }
        }
        "minecraft:ominous_bottle_amplifier"
        | "minecraft:damage"
        | "minecraft:repair_cost"
        | "minecraft:max_stack_size"
        | "minecraft:max_damage" => w.var(integer(v)?),
        "minecraft:dyed_color" | "minecraft:map_color" => w.i32(integer(v)?),
        "minecraft:rarity" => enumeration(w, v, &["common", "uncommon", "rare", "epic"])?,
        "minecraft:firework_explosion" => explosion(w, v)?,
        "minecraft:fireworks" => {
            w.var(number(v.get("flight_duration"), 0)?);
            let vals = v.get("explosions").map(array).transpose()?.unwrap_or(&[]);
            w.var(vals.len() as i32);
            for v in vals {
                explosion(w, v)?;
            }
        }
        "minecraft:banner_patterns" => {
            let vals = array(v)?;
            w.var(vals.len() as i32);
            for v in vals {
                holder(
                    w,
                    c,
                    "minecraft:banner_pattern",
                    v.get("pattern").ok_or("banner pattern")?,
                    true,
                )?;
                enumeration(w, v.get("color").ok_or("banner color")?, COLORS)?;
            }
        }
        "minecraft:trim" => {
            holder(
                w,
                c,
                "minecraft:trim_material",
                v.get("material").ok_or("trim material")?,
                true,
            )?;
            holder(
                w,
                c,
                "minecraft:trim_pattern",
                v.get("pattern").ok_or("trim pattern")?,
                true,
            )?;
        }
        "minecraft:block_state" => {
            let vals = object(v)?;
            w.var(vals.len() as i32);
            for (k, v) in vals {
                w.string(k);
                w.string(text(v)?);
            }
        }
        "minecraft:tooltip_display" => {
            w.byte(u8::from(flag(v.get("hide_tooltip"), false)?));
            let vals = v
                .get("hidden_components")
                .map(array)
                .transpose()?
                .unwrap_or(&[]);
            w.var(vals.len() as i32);
            for v in vals {
                holder(w, c, "minecraft:data_component_type", v, false)?;
            }
        }
        "minecraft:entity_data" | "minecraft:block_entity_data" => {
            holder(
                w,
                c,
                if name == "minecraft:entity_data" {
                    "minecraft:entity_type"
                } else {
                    "minecraft:block_entity_type"
                },
                v.get("id").ok_or("typed entity id")?,
                false,
            )?;
            nbt::write(&nbt_json(v)?, w)?;
        }
        "minecraft:item_name" | "minecraft:custom_name" => nbt::write(&nbt_json(v)?, w)?,
        "minecraft:lore" => {
            let vals = array(v)?;
            w.var(vals.len() as i32);
            for v in vals {
                nbt::write(&nbt_json(v)?, w)?;
            }
        }
        "minecraft:enchantment_glint_override" => w.byte(u8::from(flag(Some(v), false)?)),
        "minecraft:item_model" | "minecraft:tooltip_style" | "minecraft:note_block_sound" => {
            w.string(text(v)?)
        }
        "minecraft:custom_data" | "minecraft:bucket_entity_data" => nbt::write(&nbt_json(v)?, w)?,
        _ => {
            return Err(format!(
                "persistent preset encoder requires source codec for {name}"
            ));
        }
    }
    Ok(())
}

pub(super) fn with_components(
    c: &Content,
    base: &Stack,
    overrides: &BTreeMap<String, Value>,
) -> Result<Stack> {
    let mut payloads = BTreeMap::new();
    let mut restored = BTreeSet::new();
    let defaults = c.item_components(base.id)?;
    for (name, value) in overrides {
        let removing = name.starts_with('!');
        let name = name.strip_prefix('!').unwrap_or(name);
        let id = c
            .components
            .iter()
            .find_map(|(id, n)| (n == name).then_some(*id))
            .ok_or("unknown preset component")?;
        if payloads.contains_key(&id) || restored.contains(&id) {
            return Err("contradictory preset component".into());
        }
        if removing && !defaults.contains_key(name)
            || !removing && defaults.get(name) == Some(value)
        {
            restored.insert(id);
            continue;
        }
        let payload = if removing {
            None
        } else {
            let mut w = Writer::default();
            encode(&mut w, c, name, value)?;
            Some(w.0)
        };
        if payloads.insert(id, payload).is_some() {
            return Err("contradictory preset component".into());
        }
    }
    rewrite(c, base, payloads, restored)
}

/// Copy only the resulting wire, retaining immutable component payloads until
/// serialization. Unchanged payloads are never copied into intermediate maps.
pub(super) fn rewrite(
    c: &Content,
    base: &Stack,
    payloads: BTreeMap<i32, Option<Vec<u8>>>,
    restored: BTreeSet<i32>,
) -> Result<Stack> {
    if base.empty() {
        return Err("cannot patch empty stack".into());
    }
    let mut source = Reader::new(&base.wire);
    source.var()?;
    source.var()?;
    let patch_start = source.offset;
    let mut removed = base
        .patch
        .removed
        .iter()
        .copied()
        .filter(|id| !payloads.contains_key(id) && !restored.contains(id))
        .collect::<Vec<_>>();
    removed.extend(
        payloads
            .iter()
            .filter_map(|(id, payload)| payload.is_none().then_some(*id)),
    );
    let added = base
        .patch
        .added
        .keys()
        .filter(|id| !payloads.contains_key(id) && !restored.contains(id))
        .count()
        + payloads.values().filter(|p| p.is_some()).count();
    let mut w = Writer::default();
    w.var(base.count);
    w.var(base.id as i32);
    w.var(added as i32);
    w.var(removed.len() as i32);
    for (id, value) in &base.patch.added {
        if !payloads.contains_key(id) && !restored.contains(id) {
            w.var(*id);
            w.0.extend_from_slice(
                &base.wire
                    [patch_start + value.wire_range.start..patch_start + value.wire_range.end],
            );
        }
    }
    for (&id, payload) in &payloads {
        if let Some(payload) = payload {
            w.var(id);
            w.0.extend_from_slice(payload);
        }
    }
    for id in removed {
        w.var(id);
    }
    let mut r = Reader::new(&w.0);
    let mut stack = super::read_stack(&mut r, c)?;
    r.done()?;
    let bundle_changed = payloads.keys().chain(restored.iter()).any(|id| {
        c.components
            .get(id)
            .is_some_and(|name| name == "minecraft:bundle_contents")
    });
    if !bundle_changed {
        stack.bundle_selected = base.bundle_selected;
    }
    for (&id, source) in &base.patch.added {
        if !source.nested_stacks.is_empty()
            && !payloads.contains_key(&id)
            && !restored.contains(&id)
        {
            if let Some(value) = std::sync::Arc::make_mut(&mut stack.patch)
                .added
                .get_mut(&id)
            {
                value.nested_stacks = source.nested_stacks.clone();
            }
        }
    }
    Ok(stack)
}
