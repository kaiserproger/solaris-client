//! Immutable numerical tint facts captured by TintFacts on verified 26.1.2; not gameplay simulation.
use crate::protocol::Result;
use serde_json::Value;
fn effect_color(id: &str) -> Result<u32> {
    Ok(match id {
        "minecraft:absorption" => 2445989,
        "minecraft:bad_omen" => 745784,
        "minecraft:blindness" => 2039587,
        "minecraft:breath_of_the_nautilus" => 65518,
        "minecraft:conduit_power" => 1950417,
        "minecraft:darkness" => 2696993,
        "minecraft:dolphins_grace" => 8954814,
        "minecraft:fire_resistance" => 16750848,
        "minecraft:glowing" => 9740385,
        "minecraft:haste" => 14270531,
        "minecraft:health_boost" => 16284963,
        "minecraft:hero_of_the_village" => 4521796,
        "minecraft:hunger" => 5797459,
        "minecraft:infested" => 9214860,
        "minecraft:instant_damage" => 11101546,
        "minecraft:instant_health" => 16262179,
        "minecraft:invisibility" => 16185078,
        "minecraft:jump_boost" => 16646020,
        "minecraft:levitation" => 13565951,
        "minecraft:luck" => 5882118,
        "minecraft:mining_fatigue" => 4866583,
        "minecraft:nausea" => 5578058,
        "minecraft:night_vision" => 12779366,
        "minecraft:oozing" => 10092451,
        "minecraft:poison" => 8889187,
        "minecraft:raid_omen" => 14565464,
        "minecraft:regeneration" => 13458603,
        "minecraft:resistance" => 9520880,
        "minecraft:saturation" => 16262179,
        "minecraft:slow_falling" => 15978425,
        "minecraft:slowness" => 9154528,
        "minecraft:speed" => 3402751,
        "minecraft:strength" => 16762624,
        "minecraft:trial_omen" => 1484454,
        "minecraft:unluck" => 12624973,
        "minecraft:water_breathing" => 10017472,
        "minecraft:weakness" => 4738376,
        "minecraft:weaving" => 7891290,
        "minecraft:wind_charged" => 12438015,
        "minecraft:wither" => 7561558,
        _ => return Err(format!("unknown potion effect tint {id}")),
    })
}
fn potion_effects(id: &str) -> Result<&'static [(&'static str, u32)]> {
    Ok(match id {
        "minecraft:fire_resistance" => &[("minecraft:fire_resistance", 1)],
        "minecraft:harming" => &[("minecraft:instant_damage", 1)],
        "minecraft:healing" => &[("minecraft:instant_health", 1)],
        "minecraft:infested" => &[("minecraft:infested", 1)],
        "minecraft:invisibility" => &[("minecraft:invisibility", 1)],
        "minecraft:leaping" => &[("minecraft:jump_boost", 1)],
        "minecraft:long_fire_resistance" => &[("minecraft:fire_resistance", 1)],
        "minecraft:long_invisibility" => &[("minecraft:invisibility", 1)],
        "minecraft:long_leaping" => &[("minecraft:jump_boost", 1)],
        "minecraft:long_night_vision" => &[("minecraft:night_vision", 1)],
        "minecraft:long_poison" => &[("minecraft:poison", 1)],
        "minecraft:long_regeneration" => &[("minecraft:regeneration", 1)],
        "minecraft:long_slow_falling" => &[("minecraft:slow_falling", 1)],
        "minecraft:long_slowness" => &[("minecraft:slowness", 1)],
        "minecraft:long_strength" => &[("minecraft:strength", 1)],
        "minecraft:long_swiftness" => &[("minecraft:speed", 1)],
        "minecraft:long_turtle_master" => &[("minecraft:slowness", 4), ("minecraft:resistance", 3)],
        "minecraft:long_water_breathing" => &[("minecraft:water_breathing", 1)],
        "minecraft:long_weakness" => &[("minecraft:weakness", 1)],
        "minecraft:luck" => &[("minecraft:luck", 1)],
        "minecraft:night_vision" => &[("minecraft:night_vision", 1)],
        "minecraft:oozing" => &[("minecraft:oozing", 1)],
        "minecraft:poison" => &[("minecraft:poison", 1)],
        "minecraft:regeneration" => &[("minecraft:regeneration", 1)],
        "minecraft:slow_falling" => &[("minecraft:slow_falling", 1)],
        "minecraft:slowness" => &[("minecraft:slowness", 1)],
        "minecraft:strength" => &[("minecraft:strength", 1)],
        "minecraft:strong_harming" => &[("minecraft:instant_damage", 2)],
        "minecraft:strong_healing" => &[("minecraft:instant_health", 2)],
        "minecraft:strong_leaping" => &[("minecraft:jump_boost", 2)],
        "minecraft:strong_poison" => &[("minecraft:poison", 2)],
        "minecraft:strong_regeneration" => &[("minecraft:regeneration", 2)],
        "minecraft:strong_slowness" => &[("minecraft:slowness", 4)],
        "minecraft:strong_strength" => &[("minecraft:strength", 2)],
        "minecraft:strong_swiftness" => &[("minecraft:speed", 2)],
        "minecraft:strong_turtle_master" => {
            &[("minecraft:slowness", 6), ("minecraft:resistance", 4)]
        }
        "minecraft:swiftness" => &[("minecraft:speed", 1)],
        "minecraft:turtle_master" => &[("minecraft:slowness", 4), ("minecraft:resistance", 3)],
        "minecraft:water_breathing" => &[("minecraft:water_breathing", 1)],
        "minecraft:weakness" => &[("minecraft:weakness", 1)],
        "minecraft:weaving" => &[("minecraft:weaving", 1)],
        "minecraft:wind_charged" => &[("minecraft:wind_charged", 1)],
        "minecraft:water" | "minecraft:mundane" | "minecraft:thick" | "minecraft:awkward" => &[],
        _ => return Err(format!("unknown potion tint {id}")),
    })
}
pub fn potion(contents: Option<&Value>, default: u32) -> Result<u32> {
    let Some(v) = contents else {
        return Ok(default);
    };
    if let Some(c) = v["custom_color"].as_u64() {
        return Ok(c as u32);
    }
    let mut sum = [0u64; 3];
    let mut weight = 0u64;
    let mut add = |id: &str, n: u32| -> Result<()> {
        let color = effect_color(id)?;
        for (i, shift) in [16, 8, 0].into_iter().enumerate() {
            sum[i] += u64::from((color >> shift) & 255) * u64::from(n);
        }
        weight += u64::from(n);
        Ok(())
    };
    if let Some(id) = v["potion"].as_str() {
        for &(effect, n) in potion_effects(id)? {
            add(effect, n)?;
        }
    }
    if let Some(es) = v["custom_effects"].as_array() {
        if es.len() > 256 {
            return Err("potion_effect_limit".into());
        }
        for e in es {
            if e["show_particles"].as_bool() == Some(false) {
                continue;
            }
            let id = e["id"].as_str().ok_or("potion_effect_id")?;
            let amplifier = e["amplifier"].as_u64().unwrap_or(0).min(255) as u32;
            add(id, amplifier + 1)?;
        }
    }
    if weight == 0 {
        Ok(default)
    } else {
        Ok((((sum[0] / weight) << 16) | ((sum[1] / weight) << 8) | (sum[2] / weight)) as u32)
    }
}
