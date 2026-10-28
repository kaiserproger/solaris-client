//! Target 26.3 component stream values, expressed in persistent Codec/HashOps types.
//! Registration and field order are sourced from pinned DataComponents and component javap.
use super::{Stack, Tag as H, nbt, read_template_depth};
use crate::protocol::{Reader, Result, content::Content};
use std::sync::{Arc, LazyLock};

type Fields = Vec<(H, H)>;
fn put(f: &mut Fields, key: &str, value: H) {
    f.push((H::string(key), value));
}
fn optional(f: &mut Fields, key: &str, value: H, default: H) {
    if value != default {
        put(f, key, value);
    }
}
fn boolean(r: &mut Reader<'_>) -> Result<H> {
    Ok(H::Bool(r.bool()?))
}
fn float(r: &mut Reader<'_>) -> Result<H> {
    Ok(H::Float(r.f32()?))
}
fn integer(r: &mut Reader<'_>) -> Result<H> {
    Ok(H::Int(r.var()?))
}
fn string(r: &mut Reader<'_>) -> Result<H> {
    Ok(H::string(r.string(32767)?))
}
fn list(r: &mut Reader<'_>, mut read: impl FnMut(&mut Reader<'_>) -> Result<H>) -> Result<H> {
    let count = r.count(65536)?;
    let mut values = Vec::with_capacity(count);
    for _ in 0..count {
        values.push(read(r)?);
    }
    Ok(H::List(values))
}
fn opt(
    r: &mut Reader<'_>,
    f: &mut Fields,
    key: &str,
    read: impl FnOnce(&mut Reader<'_>) -> Result<H>,
) -> Result<()> {
    if r.bool()? {
        put(f, key, read(r)?);
    }
    Ok(())
}
fn enum_value(r: &mut Reader<'_>, values: &[&str]) -> Result<H> {
    let id = r.var()?;
    // Target ByIdMap ZERO enum codecs map malformed/out-of-range values to the first entry.
    Ok(H::string(
        *values
            .get(usize::try_from(id).unwrap_or(usize::MAX))
            .unwrap_or(&values[0]),
    ))
}
pub(super) fn holder_registry(r: &mut Reader<'_>, c: &Content, registry: &str) -> Result<H> {
    Ok(H::string(c.registry_name(registry, r.var()?)?))
}
pub(super) fn holder_set(r: &mut Reader<'_>, c: &Content, registry: &str) -> Result<H> {
    let size = r.count(65537)?;
    if size == 0 {
        return Ok(H::string(format!("#{}", r.string(32767)?)));
    }
    let mut values = Vec::with_capacity(size - 1);
    for _ in 1..size {
        values.push(holder_registry(r, c, registry)?);
    }
    // RegistryCodecs.homogeneousList defaults allowSingle=true.
    Ok(if values.len() == 1 {
        values.pop().unwrap()
    } else {
        H::List(values)
    })
}
fn holder(
    r: &mut Reader<'_>,
    c: &Content,
    registry: &str,
    inline: impl FnOnce(&mut Reader<'_>, &Content) -> Result<H>,
) -> Result<H> {
    let id = r.count(1_000_001)?;
    if id == 0 {
        inline(r, c)
    } else {
        Ok(H::string(c.registry_name(registry, id as i32 - 1)?))
    }
}
fn sound(r: &mut Reader<'_>, c: &Content) -> Result<H> {
    holder(r, c, "minecraft:sound_event", |r, _| {
        let mut f = vec![];
        put(&mut f, "sound_id", string(r)?);
        opt(r, &mut f, "range", float)?;
        Ok(H::Map(f))
    })
}
fn text(r: &mut Reader<'_>) -> Result<H> {
    normalize_text(nbt::read(r)?)
}
pub(super) fn normalize_text(value: H) -> Result<H> {
    match value {
        H::String(_) => Ok(value),
        H::List(v) => Ok(H::List(
            v.into_iter().map(normalize_text).collect::<Result<_>>()?,
        )),
        H::Map(mut f) => {
            f.retain(|(k,v)| !matches!((k,v),(H::String(k),H::String(v)) if k=="type" && matches!(v.as_str(),"text"|"translatable"|"score"|"selector"|"keybind"|"nbt")));
            for (k, v) in &mut f {
                let H::String(k) = k else {
                    return Err("rich text field is not string".into());
                };
                if matches!(
                    k.as_str(),
                    "bold" | "italic" | "underlined" | "strikethrough" | "obfuscated" | "interpret"
                ) {
                    if let H::Byte(n) = v {
                        *v = H::Bool(*n != 0);
                    }
                }
                if matches!(k.as_str(), "extra" | "with" | "separator") {
                    *v = normalize_text(std::mem::replace(v, H::Empty))?;
                }
            }
            if f.len() == 1 && f[0].0 == H::string("text") {
                return Ok(f.remove(0).1);
            }
            Ok(H::Map(f))
        }
        _ => Err("invalid rich text component".into()),
    }
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

pub(super) fn read(r: &mut Reader<'_>, c: &Content, name: &str, depth: u8) -> Result<H> {
    Ok(read_retained(r, c, name, depth)?.0)
}
pub(super) fn read_retained(
    r: &mut Reader<'_>,
    c: &Content,
    name: &str,
    depth: u8,
) -> Result<(H, Arc<[Stack]>)> {
    static EMPTY: LazyLock<Arc<[Stack]>> = LazyLock::new(|| Arc::from([]));
    let mut stacks = Vec::new();
    let mut template = |r: &mut Reader<'_>| -> Result<H> {
        let stack = read_template_depth(r, c, depth + 1)?;
        let persistent = stack.persistent_template(c)?;
        stacks.push(stack);
        Ok(persistent)
    };
    let value = match name {
        "minecraft:use_remainder" | "minecraft:sulfur_cube_content" => template(r)?,
        "minecraft:charged_projectiles" | "minecraft:bundle_contents" => list(r, template)?,
        "minecraft:pot_decorations" => {
            let mut sides = Vec::new();
            for side in ["back", "left", "right", "front"] {
                opt(r, &mut sides, side, &mut template)?;
            }
            H::Map(sides)
        }
        "minecraft:container" => {
            let count = r.count(256)?;
            let mut entries = Vec::new();
            for slot in 0..count {
                if r.bool()? {
                    entries.push(H::record([
                        ("slot", H::Int(slot as i32)),
                        ("item", template(r)?),
                    ]));
                }
            }
            H::List(entries)
        }
        _ => read_value(r, c, name, depth)?,
    };
    let stacks = if stacks.is_empty() {
        EMPTY.clone()
    } else {
        Arc::from(stacks)
    };
    Ok((value, stacks))
}
fn read_value(r: &mut Reader<'_>, c: &Content, name: &str, depth: u8) -> Result<H> {
    let name = name
        .strip_prefix("minecraft:")
        .ok_or("non-target component namespace")?;
    if let Some(registry) = match name {
        "villager/variant" => Some("minecraft:villager_type"),
        "wolf/variant" => Some("minecraft:wolf_variant"),
        "wolf/sound_variant" => Some("minecraft:wolf_sound_variant"),
        "pig/variant" => Some("minecraft:pig_variant"),
        "pig/sound_variant" => Some("minecraft:pig_sound_variant"),
        "cow/variant" => Some("minecraft:cow_variant"),
        "cow/sound_variant" => Some("minecraft:cow_sound_variant"),
        "chicken/variant" => Some("minecraft:chicken_variant"),
        "chicken/sound_variant" => Some("minecraft:chicken_sound_variant"),
        "zombie_nautilus/variant" => Some("minecraft:zombie_nautilus_variant"),
        "frog/variant" => Some("minecraft:frog_variant"),
        "cat/variant" => Some("minecraft:cat_variant"),
        "cat/sound_variant" => Some("minecraft:cat_sound_variant"),
        _ => None,
    } {
        return holder_registry(r, c, registry);
    }
    Ok(match name {
        "max_stack_size"
        | "max_damage"
        | "damage"
        | "repair_cost"
        | "map_id"
        | "ominous_bottle_amplifier"
        | "additional_trade_cost" => integer(r)?,
        "minimum_attack_charge" | "potion_duration_scale" => float(r)?,
        "dyed_color" | "map_color" => H::Int(r.i32()?),
        "unbreakable" | "creative_slot_lock" | "glider" | "waxed" => H::Map(vec![]),
        "intangible_projectile" => {
            let _ = nbt::read(r)?;
            H::Map(vec![])
        }
        "enchantment_glint_override" => boolean(r)?,
        "item_model" | "tooltip_style" | "note_block_sound" => string(r)?,
        "custom_data" | "bucket_entity_data" | "map_decorations" | "debug_stick_state" | "lock"
        | "container_loot" | "recipes" => nbt::read(r)?,
        "custom_name" | "item_name" => text(r)?,
        "lore" => list(r, text)?,
        "block_transformer" => holder_registry(r, c, "minecraft:block_transformer")?,
        "provides_pottery_pattern" => holder_registry(r, c, "minecraft:decorated_pot_pattern")?,
        "villager_food" => H::record([("nutrition", integer(r)?)]),
        "compostable" => H::record([("layers", resolvable(r, false)?)]),
        "cooking_fuel" | "brewing_fuel" => H::record([
            (if name == "cooking_fuel" { "burn_time" } else { "uses" }, resolvable(r, false)?),
            ("speed_multiplier", resolvable(r, true)?),
        ]),
        "mob_visibility" => H::record([
            ("targeting_entity_types", holder_set(r, c, "minecraft:entity_type")?),
            ("visibility", float(r)?),
        ]),
        "sign_text_front" | "sign_text_back" => sign_text(r)?,
        "rarity" => enum_value(r, &["common", "uncommon", "rare", "epic"])?,
        "dye"
        | "base_color"
        | "wolf/collar"
        | "cat/collar"
        | "sheep/color"
        | "shulker/color"
        | "tropical_fish/base_color"
        | "tropical_fish/pattern_color" | "cushion/color" => enum_value(r, COLORS)?,
        "fox/variant" => enum_value(r, &["red", "snow"])?,
        "salmon/size" => enum_value(r, &["small", "medium", "large"])?,
        "parrot/variant" => enum_value(r, &["red_blue", "blue", "green", "yellow_blue", "gray"])?,
        "tropical_fish/pattern" => {
            let id = r.var()?;
            H::string(match id {
                256 => "sunstreak",
                512 => "snooper",
                768 => "dasher",
                1024 => "brinely",
                1280 => "spotty",
                1 => "flopper",
                257 => "stripey",
                513 => "glitter",
                769 => "blockfish",
                1025 => "betty",
                1281 => "clayfish",
                _ => "kob",
            })
        }
        "mooshroom/variant" => enum_value(r, &["red", "brown"])?,
        "rabbit/variant" => {
            let id = r.var()?;
            H::string(match id {
                1 => "white",
                2 => "black",
                3 => "white_splotched",
                4 => "gold",
                5 => "salt",
                99 => "evil",
                _ => "brown",
            })
        }
        "horse/variant" => enum_value(
            r,
            &[
                "white",
                "creamy",
                "chestnut",
                "brown",
                "black",
                "gray",
                "dark_brown",
            ],
        )?,
        "llama/variant" => enum_value(r, &["creamy", "white", "brown", "gray"])?,
        "axolotl/variant" => enum_value(r, &["lucy", "wild", "gold", "cyan", "blue"])?,
        "map_post_processing" => enum_value(r, &["lock", "scale"])?,
        "use_effects" => {
            let mut f = vec![];
            optional(&mut f, "can_sprint", boolean(r)?, H::Bool(false));
            optional(&mut f, "interact_vibrations", boolean(r)?, H::Bool(true));
            optional(&mut f, "speed_multiplier", float(r)?, H::Float(0.2));
            H::Map(f)
        }
        "food" => {
            let mut f = vec![];
            put(&mut f, "nutrition", integer(r)?);
            put(&mut f, "saturation", float(r)?);
            optional(&mut f, "can_always_eat", boolean(r)?, H::Bool(false));
            H::Map(f)
        }
        "use_cooldown" => {
            let mut f = vec![];
            put(&mut f, "seconds", float(r)?);
            opt(r, &mut f, "cooldown_group", string)?;
            H::Map(f)
        }
        "repairable" => H::record([("items", holder_set(r, c, "minecraft:item")?)]),
        "damage_resistant" => H::record([("types", holder_set(r, c, "minecraft:damage_type")?)]),
        "enchantable" => H::record([("value", integer(r)?)]),
        "enchantments" | "stored_enchantments" => {
            let count = r.count(65536)?;
            let mut f = vec![];
            for _ in 0..count {
                let key = holder_registry(r, c, "minecraft:enchantment")?;
                let level = integer(r)?;
                f.push((key, level));
            }
            H::Map(f)
        }
        "tooltip_display" => {
            let mut f = vec![];
            optional(&mut f, "hide_tooltip", boolean(r)?, H::Bool(false));
            let values = list(r, |r| {
                holder_registry(r, c, "minecraft:data_component_type")
            })?;
            optional(&mut f, "hidden_components", values, H::List(vec![]));
            H::Map(f)
        }
        "custom_model_data" => {
            let mut f = vec![];
            optional(&mut f, "floats", list(r, float)?, H::List(vec![]));
            optional(&mut f, "flags", list(r, boolean)?, H::List(vec![]));
            optional(&mut f, "strings", list(r, string)?, H::List(vec![]));
            optional(
                &mut f,
                "colors",
                list(r, |r| Ok(H::Int(r.i32()?)))?,
                H::List(vec![]),
            );
            H::Map(f)
        }
        "tool" => {
            let rules = list(r, |r| {
                let mut f = vec![];
                put(&mut f, "blocks", holder_set(r, c, "minecraft:block")?);
                opt(r, &mut f, "speed", float)?;
                opt(r, &mut f, "correct_for_drops", boolean)?;
                Ok(H::Map(f))
            })?;
            let mut f = vec![];
            put(&mut f, "rules", rules);
            optional(&mut f, "default_mining_speed", float(r)?, H::Float(1.));
            optional(&mut f, "damage_per_block", integer(r)?, H::Int(1));
            optional(
                &mut f,
                "can_destroy_blocks_in_creative",
                boolean(r)?,
                H::Bool(true),
            );
            H::Map(f)
        }
        "weapon" => {
            let mut f = vec![];
            optional(&mut f, "item_damage_per_attack", integer(r)?, H::Int(1));
            optional(
                &mut f,
                "disable_blocking_for_seconds",
                float(r)?,
                H::Float(0.),
            );
            H::Map(f)
        }
        "attack_range" => {
            let mut f = vec![];
            for (key, default) in [
                ("min_reach", 0.),
                ("max_reach", 3.),
                ("min_creative_reach", 0.),
                ("max_creative_reach", 5.),
                ("hitbox_margin", 0.3),
                ("mob_factor", 1.),
            ] {
                optional(&mut f, key, float(r)?, H::Float(default));
            }
            H::Map(f)
        }
        "attack_animation" | "interact_animation" => {
            let mut f = vec![];
            optional(
                &mut f,
                "type",
                enum_value(r, &["none", "whack", "stab"])?,
                H::string("whack"),
            );
            optional(&mut f, "duration", integer(r)?, H::Int(6));
            H::Map(f)
        }
        "piercing_weapon" => {
            let mut f = vec![];
            optional(&mut f, "deals_knockback", boolean(r)?, H::Bool(true));
            optional(&mut f, "dismounts", boolean(r)?, H::Bool(false));
            opt(r, &mut f, "sound", |r| sound(r, c))?;
            opt(r, &mut f, "hit_sound", |r| sound(r, c))?;
            H::Map(f)
        }
        "kinetic_weapon" => {
            let mut f = vec![];
            optional(&mut f, "contact_cooldown_ticks", integer(r)?, H::Int(10));
            optional(&mut f, "delay_ticks", integer(r)?, H::Int(0));
            for key in [
                "dismount_conditions",
                "knockback_conditions",
                "damage_conditions",
            ] {
                opt(r, &mut f, key, |r| {
                    let mut f = vec![];
                    put(&mut f, "max_duration_ticks", integer(r)?);
                    optional(&mut f, "min_speed", float(r)?, H::Float(0.));
                    optional(&mut f, "min_relative_speed", float(r)?, H::Float(0.));
                    Ok(H::Map(f))
                })?;
            }
            optional(&mut f, "forward_movement", float(r)?, H::Float(0.));
            optional(&mut f, "damage_multiplier", float(r)?, H::Float(1.));
            opt(r, &mut f, "sound", |r| sound(r, c))?;
            opt(r, &mut f, "hit_sound", |r| sound(r, c))?;
            H::Map(f)
        }
        "block_state" => {
            let n = r.count(4096)?;
            let mut f = vec![];
            for _ in 0..n {
                f.push((string(r)?, string(r)?));
            }
            H::Map(f)
        }
        "provides_banner_patterns" => holder_set(r, c, "minecraft:banner_pattern")?,
        "break_sound" => sound(r, c)?,
        "lodestone_tracker" => {
            let mut f = vec![];
            opt(r, &mut f, "target", |r| {
                let dimension = string(r)?;
                let p = crate::protocol::unpack_pos(r.i64()?);
                Ok(H::record([
                    ("dimension", dimension),
                    ("pos", H::Ints(p.to_vec())),
                ]))
            })?;
            optional(&mut f, "tracked", boolean(r)?, H::Bool(true));
            H::Map(f)
        }
        _ => return read_complex(r, c, name, depth),
    })
}
fn read_complex(r: &mut Reader<'_>, c: &Content, name: &str, depth: u8) -> Result<H> {
    Ok(match name {
        "damage_type" => holder_registry(r, c, "minecraft:damage_type")?,
        "consumable" => {
            let mut f = vec![];
            optional(&mut f, "consume_seconds", float(r)?, H::Float(1.6));
            optional(
                &mut f,
                "animation",
                enum_value(
                    r,
                    &[
                        "none",
                        "eat",
                        "drink",
                        "block",
                        "bow",
                        "trident",
                        "crossbow",
                        "spyglass",
                        "toot_horn",
                        "brush",
                        "bundle",
                        "spear",
                    ],
                )?,
                H::string("eat"),
            );
            optional(
                &mut f,
                "sound",
                sound(r, c)?,
                H::string("minecraft:entity.generic.eat"),
            );
            optional(&mut f, "has_consume_particles", boolean(r)?, H::Bool(true));
            optional(
                &mut f,
                "on_consume_effects",
                list(r, |r| consume_effect(r, c, depth))?,
                H::List(vec![]),
            );
            H::Map(f)
        }
        "death_protection" => {
            let mut f = vec![];
            optional(
                &mut f,
                "death_effects",
                list(r, |r| consume_effect(r, c, depth))?,
                H::List(vec![]),
            );
            H::Map(f)
        }
        "potion_contents" => {
            let mut f = vec![];
            opt(r, &mut f, "potion", |r| {
                holder_registry(r, c, "minecraft:potion")
            })?;
            opt(r, &mut f, "custom_color", |r| Ok(H::Int(r.i32()?)))?;
            optional(
                &mut f,
                "custom_effects",
                list(r, |r| effect(r, c, depth))?,
                H::List(vec![]),
            );
            opt(r, &mut f, "custom_name", string)?;
            H::Map(f)
        }
        "suspicious_stew_effects" => list(r, |r| {
            let mut f = vec![];
            put(&mut f, "id", holder_registry(r, c, "minecraft:mob_effect")?);
            optional(&mut f, "duration", integer(r)?, H::Int(160));
            Ok(H::Map(f))
        })?,
        "blocks_attacks" => {
            let mut f = vec![];
            optional(&mut f, "block_delay_seconds", float(r)?, H::Float(0.));
            optional(&mut f, "disable_cooldown_scale", float(r)?, H::Float(1.));
            let reductions = list(r, |r| {
                let mut f = vec![];
                optional(
                    &mut f,
                    "horizontal_blocking_angle",
                    float(r)?,
                    H::Float(90.),
                );
                opt(r, &mut f, "type", |r| {
                    holder_set(r, c, "minecraft:damage_type")
                })?;
                put(&mut f, "base", float(r)?);
                put(&mut f, "factor", float(r)?);
                Ok(H::Map(f))
            })?;
            optional(
                &mut f,
                "damage_reductions",
                reductions,
                H::List(vec![H::record([
                    ("base", H::Float(0.)),
                    ("factor", H::Float(1.)),
                ])]),
            );
            let damage = H::record([
                ("threshold", float(r)?),
                ("base", float(r)?),
                ("factor", float(r)?),
            ]);
            optional(
                &mut f,
                "item_damage",
                damage,
                H::record([
                    ("threshold", H::Float(1.)),
                    ("base", H::Float(0.)),
                    ("factor", H::Float(1.)),
                ]),
            );
            opt(r, &mut f, "bypassed_by", |r| {
                holder_set(r, c, "minecraft:damage_type")
            })?;
            opt(r, &mut f, "block_sound", |r| sound(r, c))?;
            opt(r, &mut f, "disabled_sound", |r| sound(r, c))?;
            H::Map(f)
        }
        "equippable" => {
            let mut f = vec![];
            put(
                &mut f,
                "slot",
                enum_value(
                    r,
                    &[
                        "mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle",
                    ],
                )?,
            );
            optional(
                &mut f,
                "equip_sound",
                sound(r, c)?,
                H::string("minecraft:item.armor.equip_generic"),
            );
            opt(r, &mut f, "asset_id", string)?;
            opt(r, &mut f, "camera_overlay", string)?;
            opt(r, &mut f, "allowed_entities", |r| {
                holder_set(r, c, "minecraft:entity_type")
            })?;
            for (key, default) in [
                ("dispensable", true),
                ("swappable", true),
                ("damage_on_hurt", true),
                ("equip_on_interact", false),
                ("can_be_sheared", false),
            ] {
                optional(&mut f, key, boolean(r)?, H::Bool(default));
            }
            optional(
                &mut f,
                "shearing_sound",
                sound(r, c)?,
                H::string("minecraft:item.shears.snip"),
            );
            H::Map(f)
        }
        "painting/variant" => holder(r, c, "minecraft:painting_variant", |r, _| {
            let mut f = vec![];
            put(&mut f, "width", integer(r)?);
            put(&mut f, "height", integer(r)?);
            put(&mut f, "asset_id", string(r)?);
            opt(r, &mut f, "title", text)?;
            opt(r, &mut f, "author", text)?;
            Ok(H::Map(f))
        })?,
        "entity_data" => typed_entity(r, c, "minecraft:entity_type")?,
        "block_entity_data" => typed_entity(r, c, "minecraft:block_entity_type")?,
        "instrument" => holder(r, c, "minecraft:instrument", |r, c| {
            Ok(H::record([
                ("sound_event", sound(r, c)?),
                ("use_duration", float(r)?),
                ("range", float(r)?),
                ("description", text(r)?),
            ]))
        })?,
        "jukebox_playable" => holder(r, c, "minecraft:jukebox_song", |r, c| {
            Ok(H::record([
                ("sound_event", sound(r, c)?),
                ("description", text(r)?),
                ("length_in_seconds", float(r)?),
                ("comparator_output", integer(r)?),
            ]))
        })?,
        "provides_trim_material" => trim_material(r, c)?,
        "trim" => H::record([
            ("material", trim_material(r, c)?),
            ("pattern", trim_pattern(r, c)?),
        ]),
        "banner_patterns" => list(r, |r| {
            let pattern = holder(r, c, "minecraft:banner_pattern", |r, _| {
                Ok(H::record([
                    ("asset_id", string(r)?),
                    ("translation_key", string(r)?),
                ]))
            })?;
            Ok(H::record([
                ("pattern", pattern),
                ("color", enum_value(r, COLORS)?),
            ]))
        })?,
        "firework_explosion" => explosion(r)?,
        "fireworks" => {
            let mut f = vec![];
            optional(
                &mut f,
                "flight_duration",
                H::Byte(r.var()? as u8 as i8),
                H::Byte(0),
            );
            optional(&mut f, "explosions", list(r, explosion)?, H::List(vec![]));
            H::Map(f)
        }
        "bees" => list(r, |r| {
            Ok(H::record([
                ("entity_data", typed_entity(r, c, "minecraft:entity_type")?),
                ("ticks_in_hive", integer(r)?),
                ("min_ticks_in_hive", integer(r)?),
            ]))
        })?,
        "writable_book_content" => {
            let mut f = vec![];
            optional(
                &mut f,
                "pages",
                list(r, |r| filterable(r, |r| Ok(H::string(r.string(1024)?))))?,
                H::List(vec![]),
            );
            H::Map(f)
        }
        "written_book_content" => {
            let mut f = vec![];
            put(
                &mut f,
                "title",
                filterable(r, |r| Ok(H::string(r.string(32)?)))?,
            );
            put(&mut f, "author", string(r)?);
            optional(&mut f, "generation", integer(r)?, H::Int(0));
            optional(
                &mut f,
                "pages",
                list(r, |r| filterable(r, text))?,
                H::List(vec![]),
            );
            optional(&mut f, "resolved", boolean(r)?, H::Bool(false));
            H::Map(f)
        }
        "attribute_modifiers" => list(r, |r| {
            let mut f = vec![];
            put(
                &mut f,
                "type",
                holder_registry(r, c, "minecraft:attribute")?,
            );
            put(&mut f, "id", string(r)?);
            put(&mut f, "amount", H::Double(r.f64()?));
            put(
                &mut f,
                "operation",
                enum_value(
                    r,
                    &["add_value", "add_multiplied_base", "add_multiplied_total"],
                )?,
            );
            optional(
                &mut f,
                "slot",
                enum_value(
                    r,
                    &[
                        "any", "mainhand", "offhand", "hand", "feet", "legs", "chest", "head",
                        "armor", "body", "saddle",
                    ],
                )?,
                H::string("any"),
            );
            match r.var()? {
                1 => put(
                    &mut f,
                    "display",
                    H::record([("type", H::string("hidden"))]),
                ),
                2 => put(
                    &mut f,
                    "display",
                    H::record([("type", H::string("override")), ("value", text(r)?)]),
                ),
                _ => {}
            }
            Ok(H::Map(f))
        })?,
        "profile" => profile(r)?,
        "can_break" | "can_place_on" => super::predicates::read_adventure(r, c, depth)?,
        _ => return Err(format!("unknown target component {name}")),
    })
}
// ResolvableValue's Either selector is true for a constant; integer constants
// use fixed INT (not VAR_INT), while references are resource identifiers.
fn resolvable(r: &mut Reader<'_>, is_float: bool) -> Result<H> {
    if !r.bool()? {
        string(r)
    } else if is_float {
        float(r)
    } else {
        Ok(H::Int(r.i32()?))
    }
}
fn sign_text(r: &mut Reader<'_>) -> Result<H> {
    let lines = |r: &mut Reader<'_>| -> Result<H> {
        let mut values = Vec::with_capacity(4);
        for _ in 0..4 {
            values.push(text(r)?);
        }
        Ok(H::List(values))
    };
    let mut fields = vec![];
    put(&mut fields, "messages", lines(r)?);
    opt(r, &mut fields, "filtered_messages", lines)?;
    optional(&mut fields, "color", enum_value(r, COLORS)?, H::string("black"));
    optional(&mut fields, "has_glowing_text", boolean(r)?, H::Bool(false));
    Ok(H::Map(fields))
}
fn filterable(r: &mut Reader<'_>, mut read: impl FnMut(&mut Reader<'_>) -> Result<H>) -> Result<H> {
    let raw = read(r)?;
    let mut f = vec![(H::string("raw"), raw)];
    if r.bool()? {
        put(&mut f, "filtered", read(r)?);
    }
    Ok(H::Map(f))
}
fn typed_entity(r: &mut Reader<'_>, c: &Content, registry: &str) -> Result<H> {
    let id = holder_registry(r, c, registry)?;
    let H::Map(mut f) = nbt::read(r)? else {
        return Err("typed entity payload is not compound".into());
    };
    f.retain(|(key, _)| key != &H::string("id"));
    put(&mut f, "id", id);
    Ok(H::Map(f))
}
fn trim_material(r: &mut Reader<'_>, c: &Content) -> Result<H> {
    holder(r, c, "minecraft:trim_material", |r, _| {
        let mut f = vec![];
        put(&mut f, "asset_name", string(r)?);
        let n = r.count(4096)?;
        let mut overrides = vec![];
        for _ in 0..n {
            overrides.push((string(r)?, string(r)?));
        }
        optional(
            &mut f,
            "override_armor_assets",
            H::Map(overrides),
            H::Map(vec![]),
        );
        put(&mut f, "description", text(r)?);
        Ok(H::Map(f))
    })
}
fn explosion(r: &mut Reader<'_>) -> Result<H> {
    let mut f = vec![];
    put(
        &mut f,
        "shape",
        enum_value(r, &["small_ball", "large_ball", "star", "creeper", "burst"])?,
    );
    for key in ["colors", "fade_colors"] {
        let n = r.count(65536)?;
        let mut colors = Vec::with_capacity(n);
        for _ in 0..n {
            colors.push(r.i32()?);
        }
        optional(
            &mut f,
            key,
            H::List(colors.into_iter().map(H::Int).collect()),
            H::List(vec![]),
        );
    }
    optional(&mut f, "has_trail", boolean(r)?, H::Bool(false));
    optional(&mut f, "has_twinkle", boolean(r)?, H::Bool(false));
    Ok(H::Map(f))
}
fn consume_effect(r: &mut Reader<'_>, c: &Content, depth: u8) -> Result<H> {
    let ty = c.registry_name("minecraft:consume_effect_type", r.var()?)?;
    let mut f = vec![];
    put(&mut f, "type", H::string(ty));
    match ty {
        "minecraft:apply_effects" => {
            put(&mut f, "effects", list(r, |r| effect(r, c, depth))?);
            optional(&mut f, "probability", float(r)?, H::Float(1.));
        }
        "minecraft:remove_effects" => {
            put(&mut f, "effects", holder_set(r, c, "minecraft:mob_effect")?)
        }
        "minecraft:clear_all_effects" => {}
        "minecraft:teleport_randomly" => optional(&mut f, "diameter", float(r)?, H::Float(16.)),
        "minecraft:play_sound" => put(&mut f, "sound", sound(r, c)?),
        _ => return Err(format!("unknown consume effect type {ty}")),
    }
    Ok(H::Map(f))
}
fn effect(r: &mut Reader<'_>, c: &Content, depth: u8) -> Result<H> {
    let id = holder_registry(r, c, "minecraft:mob_effect")?;
    let mut f = effect_details(r, depth)?;
    put(&mut f, "id", id);
    Ok(H::Map(f))
}
fn effect_details(r: &mut Reader<'_>, depth: u8) -> Result<Fields> {
    if depth > 32 {
        return Err("effect nesting bound".into());
    }
    let mut f = vec![];
    optional(
        &mut f,
        "amplifier",
        H::Byte(r.var()? as u8 as i8),
        H::Byte(0),
    );
    optional(&mut f, "duration", integer(r)?, H::Int(0));
    optional(&mut f, "ambient", boolean(r)?, H::Bool(false));
    optional(&mut f, "show_particles", boolean(r)?, H::Bool(true));
    put(&mut f, "show_icon", boolean(r)?);
    opt(r, &mut f, "hidden_effect", |r| {
        Ok(H::Map(effect_details(r, depth + 1)?))
    })?;
    Ok(f)
}
fn profile(r: &mut Reader<'_>) -> Result<H> {
    let resolved = r.bool()?;
    let mut f = vec![];
    if resolved {
        put(
            &mut f,
            "id",
            H::Ints(vec![r.i32()?, r.i32()?, r.i32()?, r.i32()?]),
        );
        put(&mut f, "name", H::string(r.string(16)?));
    } else {
        opt(r, &mut f, "name", |r| Ok(H::string(r.string(16)?)))?;
        opt(r, &mut f, "id", |r| {
            Ok(H::Ints(vec![r.i32()?, r.i32()?, r.i32()?, r.i32()?]))
        })?;
    }
    let properties = list(r, |r| {
        let mut f = vec![];
        put(&mut f, "name", string(r)?);
        put(&mut f, "value", string(r)?);
        opt(r, &mut f, "signature", string)?;
        Ok(H::Map(f))
    })?;
    optional(&mut f, "properties", properties, H::List(vec![]));
    for field in ["texture", "cape", "elytra"] {
        opt(r, &mut f, field, string)?;
    }
    opt(r, &mut f, "model", |r| {
        Ok(H::string(if r.bool()? { "slim" } else { "wide" }))
    })?;
    Ok(H::Map(f))
}
pub(super) fn trim_pattern(r: &mut Reader<'_>, c: &Content) -> Result<H> {
    holder(r, c, "minecraft:trim_pattern", |r, _| {
        Ok(H::record([
            ("asset_id", string(r)?),
            ("description", text(r)?),
            ("decal", boolean(r)?),
        ]))
    })
}

#[cfg(test)]
#[path = "components_263_tests.rs"]
mod tests_263;
