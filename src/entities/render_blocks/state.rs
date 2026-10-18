use super::{dye, math};
use crate::{
    entities::{
        BlockEntity,
        blocks::{BeamSection, SourceAnimation},
    },
    protocol::{
        Result,
        content::{Content, FluidKind},
        world::World,
    },
};

fn random(state: &mut SourceAnimation, bound: u32) -> u32 {
    // Java's legacy 48-bit generator: local visual randomness, never packet authority.
    loop {
        state.random_step();
        let bits = (state.random_bits() >> 17) as u32;
        if bound.is_power_of_two() {
            return ((u64::from(bound) * u64::from(bits)) >> 31) as u32;
        }
        let value = bits % bound;
        if bits.wrapping_sub(value).wrapping_add(bound - 1) <= i32::MAX as u32 {
            return value;
        }
    }
}
fn nearby(position: [i32; 3], players: &[[f64; 3]], radius: f64) -> Option<[f64; 3]> {
    let center = position.map(|v| f64::from(v) + 0.5);
    players
        .iter()
        .copied()
        .filter_map(|p| {
            let distance = (0..3).map(|i| (p[i] - center[i]).powi(2)).sum::<f64>();
            (distance < radius * radius).then_some((p, distance))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|p| p.0)
}
pub(crate) fn tick(
    block: &mut BlockEntity,
    world: &World,
    content: &Content,
    game_time: i64,
    players: &[[f64; 3]],
) -> Result<()> {
    if block.type_name == "minecraft:jukebox" {
        let signals = &mut block.animation.signals;
        if let (Some(elapsed), Some(duration)) = (signals.jukebox_elapsed, signals.jukebox_duration)
        {
            if elapsed >= duration {
                signals.jukebox_elapsed = None;
                signals.jukebox_duration = None;
            } else {
                signals.jukebox_elapsed = Some(elapsed + 1);
            }
        }
    }
    let source = &mut block.animation.source;
    source.ticks = source.ticks.saturating_add(1);
    let state = world.block(block.position).or(block.state);
    let props = state.and_then(|s| content.state_properties(s));
    match block.type_name.as_str() {
        "minecraft:skull" => {
            source.skull_active = props
                .and_then(|p| p.get("powered"))
                .is_some_and(|v| v == "true");
            if source.skull_active {
                source.skull_ticks += 1;
            }
        }
        "minecraft:bell" => {
            if source.bell_direction.is_some() {
                source.bell_ticks += 1;
                if source.bell_ticks >= 50 {
                    source.bell_direction = None;
                    source.bell_ticks = 0;
                }
            }
        }
        "minecraft:end_gateway" => {
            source.gateway_age += 1;
            source.gateway_cooldown = source.gateway_cooldown.saturating_sub(1);
        }
        "minecraft:enchanting_table" => {
            source.book.previous_open = source.book.open;
            source.book.previous_rotation = source.book.rotation;
            if let Some(player) = nearby(block.position, players, 3.) {
                source.book.target_rotation = (player[2] - f64::from(block.position[2]) - 0.5)
                    .atan2(player[0] - f64::from(block.position[0]) - 0.5)
                    as f32;
                source.book.open += 0.1;
                if source.book.open < 0.5 || random(source, 40) == 0 {
                    let old = source.book.target_flip;
                    while old == source.book.target_flip {
                        source.book.target_flip +=
                            random(source, 4) as f32 - random(source, 4) as f32;
                    }
                }
            } else {
                source.book.target_rotation += 0.02;
                source.book.open -= 0.1;
            }
            source.book.rotation = math::wrap_radians(source.book.rotation);
            source.book.target_rotation = math::wrap_radians(source.book.target_rotation);
            source.book.rotation +=
                math::wrap_radians(source.book.target_rotation - source.book.rotation) * 0.4;
            source.book.open = source.book.open.clamp(0., 1.);
            source.book.previous_flip = source.book.flip;
            let speed = ((source.book.target_flip - source.book.flip) * 0.4).clamp(-0.2, 0.2);
            source.book.flip_speed += (speed - source.book.flip_speed) * 0.9;
            source.book.flip += source.book.flip_speed;
        }
        "minecraft:mob_spawner" => {
            source.previous_spawn_spin = source.spawn_spin;
            let radius = block.data["RequiredPlayerRange"].as_f64().unwrap_or(16.);
            if nearby(block.position, players, radius).is_some()
                && block.data["SpawnData"]["entity"]["id"].is_string()
            {
                if source.spawn_delay > 0 {
                    source.spawn_delay -= 1;
                }
                source.spawn_spin = (source.spawn_spin
                    + f64::from(1000. / (source.spawn_delay as f32 + 200.)))
                    % 360.;
            }
        }
        "minecraft:trial_spawner" => {
            let state = props
                .and_then(|p| p.get("trial_spawner_state"))
                .map(String::as_str)
                .unwrap_or("inactive");
            let speed = match state {
                "waiting_for_players" => 200.,
                "active" => 1000.,
                _ => -1.,
            };
            if speed >= 0. {
                let delay = block.data["next_mob_spawns_at"]
                    .as_i64()
                    .unwrap_or(0)
                    .wrapping_sub(game_time)
                    .max(0) as f64;
                source.previous_spawn_spin = source.spawn_spin;
                source.spawn_spin = (source.spawn_spin + speed / (delay + 200.)) % 360.;
            }
        }
        "minecraft:vault" => {
            source.previous_spin = source.spin;
            source.spin = (source.spin + 190.).rem_euclid(360.) - 180.;
        }
        "minecraft:conduit" => {
            if game_time % 40 == 0 {
                source.pending_world = false;
            }
            if game_time % 40 == 0 {
                let mut water = true;
                let mut count = 0;
                for x in -1..=1 {
                    for y in -1..=1 {
                        for z in -1..=1 {
                            let Some(s) = world.block([
                                block.position[0] + x,
                                block.position[1] + y,
                                block.position[2] + z,
                            ]) else {
                                source.pending_world = true;
                                water = false;
                                continue;
                            };
                            water &= content
                                .info(s)?
                                .fluid
                                .is_some_and(|f| f.kind == FluidKind::Water);
                        }
                    }
                }
                if water {
                    for x in -2i32..=2 {
                        for y in -2i32..=2 {
                            for z in -2i32..=2 {
                                let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
                                if ax <= 1 && ay <= 1 && az <= 1
                                    || (x != 0 || ay != 2 && az != 2)
                                        && (y != 0 || ax != 2 && az != 2)
                                        && (z != 0 || ax != 2 && ay != 2)
                                {
                                    continue;
                                }
                                let Some(s) = world.block([
                                    block.position[0] + x,
                                    block.position[1] + y,
                                    block.position[2] + z,
                                ]) else {
                                    source.pending_world = true;
                                    continue;
                                };
                                if matches!(
                                    content.name(s),
                                    Some(
                                        "minecraft:prismarine"
                                            | "minecraft:prismarine_bricks"
                                            | "minecraft:dark_prismarine"
                                            | "minecraft:sea_lantern"
                                    )
                                ) {
                                    count += 1;
                                }
                            }
                        }
                    }
                }
                source.conduit_active = water && count >= 16;
                source.conduit_hunting = water && count >= 42;
            }
            if source.conduit_active {
                source.conduit_rotation += 1.;
            }
        }
        "minecraft:beacon" => tick_beacon(block.position, source, world, content, game_time)?,
        _ => {}
    }
    Ok(())
}
fn tick_beacon(
    position: [i32; 3],
    source: &mut SourceAnimation,
    world: &World,
    content: &Content,
    game_time: i64,
) -> Result<()> {
    source.pending_world = false;
    if game_time % 80 == 0 && !source.beam_sections.is_empty() {
        source.beacon_levels_pending = true;
    }
    let Some(last_set) = world.surface_height([position[0], position[2]]) else {
        source.pending_world = true;
        return Ok(());
    };
    let mut y = source.last_check_y.unwrap_or(world.min_y - 1);
    if y < position[1] {
        source.checking_beam.clear();
        y = position[1] - 1;
    }
    for _ in 0..10 {
        if y + 1 > last_set {
            break;
        }
        let Some(state) = world.block([position[0], y + 1, position[2]]) else {
            source.pending_world = true;
            source.last_check_y = Some(y);
            return Ok(());
        };
        let name = content
            .name(state)
            .ok_or("unknown beam block")?
            .trim_start_matches("minecraft:");
        let color = if name == "beacon" {
            Some(dye("white").0)
        } else {
            name.strip_suffix("_stained_glass")
                .or_else(|| name.strip_suffix("_stained_glass_pane"))
                .map(|v| dye(v).0)
        };
        if let Some(color) = color {
            if source.checking_beam.len() <= 1 {
                source.checking_beam.push(BeamSection { color, height: 1 });
            } else if let Some(last) = source.checking_beam.last_mut() {
                if last.color == color {
                    last.height += 1;
                } else {
                    let mixed = ((last.color & 0xfefefe) >> 1)
                        + ((color & 0xfefefe) >> 1)
                        + ((last.color & color) & 0x010101);
                    source.checking_beam.push(BeamSection {
                        color: mixed,
                        height: 1,
                    });
                }
            }
        } else if content.info(state)?.light_dampening < 15 || name == "bedrock" {
            if let Some(last) = source.checking_beam.last_mut() {
                last.height += 1;
            } else {
                source.checking_beam.clear();
                y = last_set;
                break;
            }
        } else {
            source.checking_beam.clear();
            y = last_set;
            break;
        }
        y += 1;
    }
    source.last_check_y = Some(y);
    if source.beacon_levels_pending {
        let mut levels = 0;
        'layers: for layer in 1..=4 {
            let y = position[1] - layer;
            if y < world.min_y {
                break;
            }
            for x in position[0] - layer..=position[0] + layer {
                for z in position[2] - layer..=position[2] + layer {
                    let Some(state) = world.block([x, y, z]) else {
                        source.pending_world = true;
                        return Ok(());
                    };
                    if !content.block_has_tag(state, "minecraft:beacon_base_blocks")? {
                        break 'layers;
                    }
                }
            }
            levels = layer as u8;
        }
        source.beacon_levels = levels;
        source.beacon_levels_pending = false;
    }
    if y >= last_set {
        source.beam_sections.clone_from(&source.checking_beam);
        y = world.min_y - 1;
    }
    source.last_check_y = Some(y);
    Ok(())
}
