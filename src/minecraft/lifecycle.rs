use super::Outcome;
use crate::{
    api::InputIntent,
    player::{collision, AttackBlocker, Bounds, Hit, InteractionIntent},
    protocol::{content::Content, Result},
    session::Session,
};
use serde_json::{json, Map, Value};
use std::time::{Duration, Instant};

pub(super) struct Task {
    deadline: Instant,
    kind: Kind,
    held: Vec<String>,
    colliders: Vec<Bounds>,
}
enum Kind {
    Play,
    State {
        observed: u64,
    },
    Ticks {
        start: u64,
        ticks: u64,
    },
    Block {
        position: [i32; 3],
        expected: Option<String>,
        properties: Map<String, Value>,
        version: u64,
        events: u64,
    },
    Respawn {
        health_version: u64,
        ticks: Option<u64>,
        target: Option<u64>,
    },
    MineCrack {
        position: [i32; 3],
        face: i32,
        start: u64,
        ticks: u64,
        last_tick: u64,
    },
    Break {
        position: [i32; 3],
        face: i32,
        initial_revision: u64,
        item: String,
        count: i64,
        initial_count: i64,
        started: bool,
        air: bool,
        saw_drop: bool,
        collecting: bool,
        last_tick: u64,
        detour: i32,
        preferred: i32,
        detail: String,
    },
}
fn integer(args: &Value, key: &str, min: i64, max: i64) -> Result<i64> {
    args.get(key)
        .and_then(Value::as_i64)
        .filter(|value| (min..=max).contains(value))
        .ok_or_else(|| format!("{key} must be an integer between {min} and {max}"))
}
fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{key} must be a string"))
}
/// The Java chat box treats a leading `/` as a command, and the wire command packet
/// carries the text WITHOUT it. Sending "/help" as plain chat only echoes it to other
/// players (the swarm's "no server reply" reports), so an omitted `command` is inferred
/// from the slash; an explicit `command:false` stays literal chat.
fn chat_kind(message: &str, command: Option<bool>) -> Result<(&str, bool)> {
    let (message, command) = match command {
        Some(false) => (message, false),
        Some(true) => (message.strip_prefix('/').unwrap_or(message), true),
        None => match message.strip_prefix('/') {
            Some(rest) => (rest, true),
            None => (message, false),
        },
    };
    if message.is_empty() {
        return Err("chat command is empty after the leading slash".into());
    }
    Ok((message, command))
}
fn position(args: &Value) -> Result<[i32; 3]> {
    Ok([
        integer(args, "x", i32::MIN as i64, i32::MAX as i64)? as i32,
        integer(args, "y", i32::MIN as i64, i32::MAX as i64)? as i32,
        integer(args, "z", i32::MIN as i64, i32::MAX as i64)? as i32,
    ])
}
fn timeout(args: &Value, default: f64, maximum: f64) -> Result<Instant> {
    let seconds = match args.get("timeout_seconds") {
        None => default,
        Some(value) => value
            .as_f64()
            .filter(|value| value.is_finite() && *value >= 0.1 && *value <= maximum)
            .ok_or("timeout_seconds outside bounds")?,
    };
    Ok(Instant::now() + Duration::from_secs_f64(seconds))
}
fn face(args: &Value) -> Result<i32> {
    match text(args, "face")? {
        "down" => Ok(0),
        "up" => Ok(1),
        "north" => Ok(2),
        "south" => Ok(3),
        "west" => Ok(4),
        "east" => Ok(5),
        _ => Err("unknown block face".into()),
    }
}
fn require_play(session: &Session) -> Result<&Content> {
    if session.phase != "play" || !session.player.pose_known {
        return Err("client is not in play".into());
    }
    session
        .content
        .as_ref()
        .ok_or_else(|| "client resources unavailable".into())
}
fn key(session: &mut Session, name: &str, down: bool) -> Result<()> {
    session.input(InputIntent::Key {
        key: name.into(),
        pressed: down,
    })?;
    Ok(())
}
fn input_keys(args: &Value) -> Result<Vec<String>> {
    let values = args
        .get("keys")
        .and_then(Value::as_array)
        .filter(|values| (1..=8).contains(&values.len()))
        .ok_or("keys must contain 1..8 unique inputs")?;
    let mut keys = Vec::with_capacity(values.len());
    for value in values {
        let input = value.as_str().ok_or("each input key must be a string")?;
        let named = input.strip_prefix("key.keyboard.").is_some_and(|suffix| {
            !suffix.is_empty()
                && suffix.len() <= 32
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'.')
        });
        if !named
            && !matches!(
                input,
                "forward"
                    | "back"
                    | "left"
                    | "right"
                    | "jump"
                    | "sneak"
                    | "sprint"
                    | "attack"
                    | "use"
                    | "swap_offhand"
            )
        {
            return Err(format!("unsupported input key: {input}"));
        }
        if keys.iter().any(|key| key == input) {
            return Err(format!("duplicate input key: {input}"));
        }
        keys.push(input.to_owned());
    }
    Ok(keys)
}
fn snapshot(session: &Session) -> Value {
    let observation = session.minecraft_observation();
    let in_play = observation["in_play"].as_bool().unwrap_or(false);
    json!({"state_version":session.state_version(), "in_play":in_play,
        "dimension":observation["dimension"].as_str().unwrap_or(""),
        "x":observation["player"]["x"].as_f64().unwrap_or(0.),
        "y":observation["player"]["y"].as_f64().unwrap_or(0.),
        "z":observation["player"]["z"].as_f64().unwrap_or(0.),
        "selected_hotbar_slot":if in_play { i32::from(session.inventory.selected_slot) } else { -1 },
        "current_screen":observation["screen"]["class"].as_str().unwrap_or("none"), "disconnect_reason":observation["disconnect_reason"].as_str().unwrap_or("")})
}
fn aim(session: &mut Session, target: [f64; 3]) -> Result<()> {
    let eye = session.player.eye_position();
    let delta: [f64; 3] = std::array::from_fn(|axis| target[axis] - eye[axis]);
    let yaw = (delta[2].atan2(delta[0]).to_degrees() - 90.) as f32;
    let pitch = -delta[1]
        .atan2((delta[0] * delta[0] + delta[2] * delta[2]).sqrt())
        .to_degrees() as f32;
    session.input(InputIntent::Look { yaw, pitch })?;
    Ok(())
}
fn aim_block(session: &mut Session, position: [i32; 3], face: i32) -> Result<()> {
    read_state(session, position)?;
    let mut point = position.map(|value| f64::from(value) + 0.5);
    let axis = match face {
        0 | 1 => 1,
        2 | 3 => 2,
        _ => 0,
    };
    point[axis] = f64::from(position[axis]) + if face & 1 == 0 { 0. } else { 1. };
    aim(session, point)
}
/// The ordinary raycast under the current look must resolve to exactly `position`;
/// otherwise an Attack would mine another block (or nothing) and the caller would
/// wait out its deadline on a misleading `started:false`.
fn require_targeted(session: &Session, position: [i32; 3]) -> Result<()> {
    let content = require_play(session)?;
    let at = format!("{},{},{}", position[0], position[1], position[2]);
    if content.block_is_air(read_state(session, position)?)? {
        return Err(format!(
            "block {at} is already air; there is nothing to mine"
        ));
    }
    // Eye-to-centre distance beyond reach plus half a block diagonal can never
    // be hit; say so instead of blaming whatever the ray meets first.
    let eye = session.player.eye_position();
    let distance = (0..3)
        .map(|axis| (f64::from(position[axis]) + 0.5 - eye[axis]).powi(2))
        .sum::<f64>()
        .sqrt();
    let reach = session
        .player
        .attribute("minecraft:block_interaction_range");
    let out_of_reach = || {
        format!(
            "block {at} is out of reach: {distance:.1} blocks from the eye to its centre, ordinary block reach is {reach:.1}; navigate closer first"
        )
    };
    if distance > reach + 0.87 {
        return Err(out_of_reach());
    }
    match session.player.raycast(
        &session.world,
        content,
        &session.entities,
        session.client_tick,
    )? {
        Some(Hit::Block(hit)) if hit.position == position => Ok(()),
        Some(Hit::Block(hit)) => Err(format!(
            "block {at} is not targetable: {} at {},{},{} blocks the look ray",
            content.name(hit.state).unwrap_or("unknown block"),
            hit.position[0],
            hit.position[1],
            hit.position[2]
        )),
        Some(Hit::Entity { handle, .. }) => Err(format!(
            "block {at} is not targetable: entity {} is in front of it",
            handle.network_id
        )),
        None => Err(out_of_reach()),
    }
}
/// Reason the ordinary Attack did not start mining, from the same gates the
/// attack executor applies. `None` means mining is already progressing there.
fn attack_blocker(session: &Session) -> Result<Option<AttackBlocker>> {
    let content = require_play(session)?;
    let target = session.player.raycast(
        &session.world,
        content,
        &session.entities,
        session.client_tick,
    )?;
    session.player.attack_blocker(
        target.as_ref(),
        &session.world,
        content,
        &session.inventory,
        &session.entities,
    )
}
fn read_state(session: &Session, position: [i32; 3]) -> Result<u32> {
    let content = require_play(session)?;
    if session
        .world
        .chunk([position[0].div_euclid(16), position[2].div_euclid(16)])
        .is_none()
    {
        return Err(format!(
            "client chunk is not loaded at {}, {}, {}",
            position[0], position[1], position[2]
        ));
    }
    if position[1] < session.world.min_y
        || i64::from(position[1])
            >= i64::from(session.world.min_y) + i64::from(session.world.height)
    {
        return content
            .resolve_block_state("minecraft:void_air", &std::collections::BTreeMap::new());
    }
    session
        .world
        .block(position)
        .ok_or_else(|| "client block is not loaded".into())
}
fn read_block(session: &Session, position: [i32; 3], scratch: &mut Vec<Bounds>) -> Result<Value> {
    let state = read_state(session, position)?;
    let content = require_play(session)?;
    scratch.clear();
    let collision_empty = if session.entities.append_block_collision(
        position,
        &session.world,
        content,
        session.client_tick,
        scratch,
    )? {
        scratch.is_empty()
    } else {
        content
            .collision_boxes(state)
            .ok_or("unknown client collision shape")?
            .is_empty()
    };
    let [sky, block] = session.world.source_light_levels(position)?;
    let holder = content.block_fluid_holder(state)?;
    let fluid_id = content.registry_name("minecraft:fluid", holder as i32)?;
    let fluid = if let Some(fluid) = content.info(state)?.fluid {
        let above = [
            position[0],
            position[1]
                .checked_add(1)
                .ok_or("fluid position overflow")?,
            position[2],
        ];
        let above_state = read_state(session, above)?;
        let height = if content
            .info(above_state)?
            .fluid
            .is_some_and(|above| above.kind == fluid.kind)
        {
            1.
        } else {
            fluid.height()
        };
        json!({"fluid_id":fluid_id,"empty":false,"source":fluid.amount==8 && !fluid.falling,
            "amount":fluid.amount,"height":height,"own_height":fluid.height(),
            "in_water_tag":content.registry_tag_contains("minecraft:fluid","minecraft:water",holder as i32)?})
    } else {
        json!({"fluid_id":fluid_id,"empty":true,"source":false,"amount":0,"height":0.,"own_height":0.,"in_water_tag":false})
    };
    Ok(
        json!({"x":position[0],"y":position[1],"z":position[2],"block_id":content.name(state).ok_or("unknown client block")?,
        "is_air":content.block_is_air(state)?,"collision_empty":collision_empty,"sky_light":sky,"block_light":block,
        "properties":content.state_properties(state).ok_or("unknown client properties")?,"fluid":fluid}),
    )
}
/// Count the row in both MCP representations (structured JSON and escaped text).
#[derive(Default)]
struct ScanResponseBytes(usize);
impl std::io::Write for ScanResponseBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 += bytes.len() * 2
            + bytes
                .iter()
                .filter(|byte| matches!(byte, b'"' | b'\\'))
                .count();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn inventory_count(session: &Session, item: &str) -> Result<i64> {
    let content = require_play(session)?;
    let mut count = 0;
    // The inventory menu's crafting/output cells are not Player Inventory entries.
    for stack in session
        .inventory
        .player
        .slots
        .iter()
        .skip(5)
        .take(41)
        .flatten()
    {
        if !stack.empty()
            && content
                .item_names
                .get(&stack.id)
                .is_some_and(|name| name == item)
        {
            count += i64::from(stack.count);
        }
    }
    Ok(count)
}
fn break_result(
    session: &Session,
    started: bool,
    air: bool,
    saw_drop: bool,
    pickup: bool,
    detail: &str,
) -> Value {
    let observation = session.minecraft_observation();
    json!({"started":started,"became_air":air,"saw_drop":saw_drop,"pickup_confirmed":pickup,
        "selected_item_id":observation["selected_item"]["item_id"].as_str().unwrap_or("minecraft:air"),
        "selected_item_count":observation["selected_item"]["count"].as_i64().unwrap_or(0),"pickup_detail":detail})
}
pub(super) fn clear_pickup(session: &mut Session) -> Result<()> {
    for name in ["forward", "sprint", "jump", "left", "right"] {
        key(session, name, false)?;
    }
    Ok(())
}
fn probe_clear(
    session: &Session,
    direction: [f64; 3],
    distance: f64,
    raised: bool,
    scratch: &mut Vec<Bounds>,
) -> Result<bool> {
    let content = require_play(session)?;
    let mut area = session.player.bounds();
    if raised {
        area = area.moved([0., 1., 0.]);
    }
    for axis in 0..3 {
        let step = direction[axis] * distance;
        if step < 0. {
            area.min[axis] += step;
        } else {
            area.max[axis] += step;
        }
        area.min[axis] += 0.01;
        area.max[axis] -= 0.01;
    }
    collision::gather(
        &session.world,
        content,
        &session.entities,
        session.client_tick,
        area,
        scratch,
    )?;
    if scratch.iter().any(|shape| shape.intersects(area)) {
        return Ok(false);
    }
    session.entities.collision_bounds(
        area,
        session.player.entity_id,
        None,
        session.client_tick,
        session.player.position,
        &session.world,
        content,
        scratch,
    )?;
    Ok(!scratch.iter().any(|shape| shape.intersects(area))
        && !session
            .world
            .border_collides(session.player.position, area)?)
}
fn pickup_reached(player: Bounds, target: [f64; 3]) -> bool {
    // 26.1.2 Player.aiStep: player's box inflated by (1, .5, 1).
    // EntityType.ITEM: .25-wide/.25-high, with position at its bottom.
    let inflation = [1., 0.5, 1.];
    let reach = Bounds {
        min: std::array::from_fn(|axis| player.min[axis] - inflation[axis]),
        max: std::array::from_fn(|axis| player.max[axis] + inflation[axis]),
    };
    let item = Bounds {
        min: [target[0] - 0.125, target[1], target[2] - 0.125],
        max: [target[0] + 0.125, target[1] + 0.25, target[2] + 0.125],
    };
    reach.intersects(item)
}

pub(super) fn pickup_step(
    session: &mut Session,
    target: [f64; 3],
    current: i32,
    preferred: i32,
    scratch: &mut Vec<Bounds>,
) -> Result<i32> {
    movement_step(session, target, current, preferred, None, scratch)
}

fn movement_step(
    session: &mut Session,
    target: [f64; 3],
    current: i32,
    preferred: i32,
    approaching: Option<bool>,
    scratch: &mut Vec<Bounds>,
) -> Result<i32> {
    let offset: [f64; 3] = std::array::from_fn(|axis| target[axis] - session.player.position[axis]);
    let distance = (offset[0] * offset[0] + offset[2] * offset[2]).sqrt();
    let (direct, left, right, raised) = if distance < 1e-6 {
        (true, true, true, false)
    } else {
        let forward = [offset[0] / distance, 0., offset[2] / distance];
        let left = [forward[2], 0., -forward[0]];
        let direct = probe_clear(session, forward, distance.min(0.8), false, scratch)?;
        (
            direct,
            probe_clear(session, left, 0.8, false, scratch)?,
            probe_clear(session, left.map(|value| -value), 0.8, false, scratch)?,
            !direct && probe_clear(session, forward, distance.min(0.8), true, scratch)?,
        )
    };
    let step_up = session.player.horizontal_collision && raised;
    let blocked = session.player.horizontal_collision && !step_up;
    let next = if current != 0 {
        if direct {
            0
        } else if current < 0 && left {
            -1
        } else if current > 0 && right {
            1
        } else if current < 0 && right {
            1
        } else if current > 0 && left {
            -1
        } else {
            current
        }
    } else if !blocked {
        0
    } else if (preferred < 0 && left) || (preferred > 0 && right) {
        preferred
    } else if (preferred < 0 && right) || (preferred > 0 && left) {
        -preferred
    } else {
        preferred
    };
    aim(session, target)?;
    let approaching =
        approaching.unwrap_or_else(|| offset.iter().map(|value| value * value).sum::<f64>() > 4.);
    for (name, down) in [
        ("sprint", approaching),
        ("forward", approaching && next == 0),
        ("jump", approaching && step_up),
        ("left", approaching && next < 0),
        ("right", approaching && next > 0),
    ] {
        key(session, name, down)?;
    }
    Ok(next)
}

pub(super) fn start(
    session: &mut Session,
    name: &str,
    args: &Value,
) -> Result<Option<Outcome<Task>>> {
    let complete = |value| Ok(Some(Outcome::Complete(value)));
    let kind;
    let deadline;
    let mut held = Vec::new();
    match name {
        "minecraft_connect" => {
            let address = if let Some(value) = args.get("server_addr") {
                let address = value.as_str().ok_or("server_addr must be a string")?;
                let (host, port) = address
                    .rsplit_once(':')
                    .ok_or("server_addr must be host:port")?;
                if host.is_empty() || port.parse::<u16>().ok().is_none_or(|port| port == 0) {
                    return Err("server_addr must be host:port".into());
                }
                address.to_owned()
            } else {
                let host = args
                    .get("host")
                    .map(|value| value.as_str().ok_or("host must be a string"))
                    .transpose()?
                    .unwrap_or("127.0.0.1");
                let port = args
                    .get("port")
                    .map(|_| integer(args, "port", 1, 65535))
                    .transpose()?
                    .unwrap_or(25565);
                format!("{host}:{port}")
            };
            if let Some(active) = session.address() {
                if active == address {
                    return complete(json!({"status":"ok"}));
                }
                return Err(format!(
                    "client is already connected to {active}; disconnect before connecting to {address}"
                ));
            }
            let username = session
                .username()
                .ok_or("offline username is not configured")?
                .to_owned();
            session.connect(&address, &username)?;
            return complete(json!({"status":"ok"}));
        }
        "minecraft_disconnect" => {
            session.disconnect();
            return complete(json!({"status":"ok"}));
        }
        "minecraft_look" => {
            require_play(session)?;
            session.input(InputIntent::Look {
                yaw: integer(args, "yaw_deg", -180, 180)? as f32,
                pitch: integer(args, "pitch_deg", -90, 90)? as f32,
            })?;
            return complete(json!({"status":"ok"}));
        }
        "minecraft_look_at_block" => {
            aim_block(session, position(args)?, face(args)?)?;
            return complete(json!({"status":"ok"}));
        }
        "minecraft_use_item_on" => {
            require_play(session)?;
            let hand = match args
                .get("hand")
                .and_then(Value::as_str)
                .unwrap_or("main_hand")
            {
                "main_hand" => 0,
                "off_hand" => 1,
                _ => return Err("unknown interaction hand".into()),
            };
            return complete(session.use_item_on(position(args)?, face(args)?, hand)?);
        }
        "minecraft_send_chat" => {
            require_play(session)?;
            let (message, command) = chat_kind(
                text(args, "message")?,
                match args.get("command") {
                    None => None,
                    Some(value) => Some(value.as_bool().ok_or("command must be boolean")?),
                },
            )?;
            session.send_chat(message, command)?;
            return complete(json!({"status":"ok","sent_as":if command {"command"} else {"chat"}}));
        }
        "minecraft_read_block" => {
            return complete(read_block(session, position(args)?, &mut Vec::new())?);
        }
        "minecraft_scan_blocks" => {
            require_play(session)?;
            let minimum = [
                integer(args, "min_x", i32::MIN as i64, i32::MAX as i64)? as i32,
                integer(args, "min_y", i32::MIN as i64, i32::MAX as i64)? as i32,
                integer(args, "min_z", i32::MIN as i64, i32::MAX as i64)? as i32,
            ];
            let maximum = [
                integer(args, "max_x", i32::MIN as i64, i32::MAX as i64)? as i32,
                integer(args, "max_y", i32::MIN as i64, i32::MAX as i64)? as i32,
                integer(args, "max_z", i32::MIN as i64, i32::MAX as i64)? as i32,
            ];
            let limit = args
                .get("max_blocks")
                .map(|_| integer(args, "max_blocks", 1, 4096))
                .transpose()?
                .unwrap_or(4096);
            let mut volume = 1i64;
            for axis in 0..3 {
                if minimum[axis] > maximum[axis] {
                    return Err("scan bounds must satisfy min <= max on every axis".into());
                }
                volume = volume
                    .checked_mul(i64::from(maximum[axis]) - i64::from(minimum[axis]) + 1)
                    .filter(|volume| *volume <= limit)
                    .ok_or("scan volume exceeds max_blocks")?;
            }
            let offset = args
                .get("offset")
                .map(|_| integer(args, "offset", 0, volume))
                .transpose()?
                .unwrap_or(0);
            let mut response_bytes = ScanResponseBytes::default();
            let mut index = 0;
            let mut truncated = false;
            let mut blocks = Vec::with_capacity(volume as usize);
            let mut scratch = Vec::new();
            'scan: for y in i64::from(minimum[1])..=i64::from(maximum[1]) {
                for z in i64::from(minimum[2])..=i64::from(maximum[2]) {
                    for x in i64::from(minimum[0])..=i64::from(maximum[0]) {
                        if index < offset {
                            index += 1;
                            continue;
                        }
                        let row =
                            read_block(session, [x as i32, y as i32, z as i32], &mut scratch)?;
                        serde_json::to_writer(&mut response_bytes, &row)
                            .map_err(|error| error.to_string())?;
                        // Leave room for the MCP envelope and the bounds/page fields.
                        if response_bytes.0 > 240_000 {
                            if blocks.is_empty() {
                                return Err("single block exceeds bounded scan response".into());
                            }
                            truncated = true;
                            break 'scan;
                        }
                        blocks.push(row);
                        index += 1;
                    }
                }
            }
            return complete(
                json!({"count":blocks.len(),"total_count":volume,"offset":offset,"truncated":truncated,
                    "next_offset":truncated.then_some(index),
                    "bounds":{"min_x":minimum[0],"min_y":minimum[1],"min_z":minimum[2],"max_x":maximum[0],"max_y":maximum[1],"max_z":maximum[2]},"blocks":blocks}),
            );
        }
        "minecraft_wait_for_play" => {
            deadline = timeout(args, 30., 3600.)?;
            kind = Kind::Play;
        }
        "minecraft_wait_for_state_change" => {
            deadline = timeout(args, 30., 120.)?;
            kind = Kind::State {
                observed: integer(args, "observed_version", 0, i32::MAX as i64)? as u64,
            };
        }
        "minecraft_wait_for_loaded_block" | "minecraft_wait_for_block_state" => {
            require_play(session)?;
            let expected = if name == "minecraft_wait_for_block_state" {
                Some(text(args, "block_id")?.to_owned())
            } else {
                None
            };
            let properties = args
                .get("properties")
                .map(|value| {
                    value
                        .as_object()
                        .cloned()
                        .ok_or("properties must be an object")
                })
                .transpose()?
                .unwrap_or_default();
            if properties.values().any(|value| !value.is_string()) {
                return Err("block properties must be strings".into());
            }
            deadline = timeout(args, if expected.is_some() { 8. } else { 30. }, 120.)?;
            kind = Kind::Block {
                position: position(args)?,
                expected,
                properties,
                version: session.state_version(),
                events: 0,
            };
        }
        "minecraft_wait_ticks" | "minecraft_press_inputs" => {
            require_play(session)?;
            let ticks = integer(args, "ticks", 1, 255)? as u64;
            if name == "minecraft_press_inputs" && args.get("hold").is_some() {
                let down = args["hold"].as_bool().ok_or("hold must be a boolean")?;
                let keys = input_keys(args)?;
                let mut applied = Vec::new();
                for name in &keys {
                    if let Err(error) = key(session, name, down) {
                        if down {
                            for prior in applied {
                                let _ = key(session, prior, false);
                            }
                        }
                        return Err(error);
                    }
                    applied.push(name.as_str());
                }
                return complete(json!({"status":"ok", "held":down}));
            }
            if name == "minecraft_press_inputs" {
                held = input_keys(args)?;
            }
            deadline = Instant::now() + Duration::from_secs(30);
            kind = Kind::Ticks {
                start: session.client_tick,
                ticks,
            };
        }
        "minecraft_toggle_debug_overlay" => {
            // Owned by the App (lib.rs drain_requests intercept): never
            // reaches session dispatch. Kept visible here so a direct call
            // fails loudly instead of falling to "unknown tool".
            return Err("debug overlay belongs to application owner".into());
        }
        "minecraft_respawn" => {
            require_play(session)?;
            if session.player.health.is_some_and(|health| health > 0.) {
                return Err("client player is not dead".into());
            }
            if args.get("keys").is_some() != args.get("ticks").is_some() {
                return Err("respawn keys and ticks must be provided together".into());
            }
            let ticks = if args.get("keys").is_some() {
                held = input_keys(args)?;
                Some(integer(args, "ticks", 1, 255)? as u64)
            } else {
                None
            };
            deadline = timeout(args, 10., 120.)?;
            kind = Kind::Respawn {
                health_version: session.health_version(),
                ticks,
                target: None,
            };
        }
        "minecraft_mine_until_crack" => {
            // Windowed-proof seam: swing at a block WITHOUT breaking it, so the
            // playthrough can screenshot the crack overlay mid-break. Holds
            // attack for `ticks` client ticks (capped), then releases and
            // reports the observed crack stage (same formula as the overlay).
            require_play(session)?;
            let position = position(args)?;
            let face = face(args)?;
            let ticks = integer(args, "ticks", 1, 100)? as u64;
            deadline = timeout(args, 8., 120.)?;
            key(session, "attack", false)?;
            aim_block(session, position, face)?;
            require_targeted(session, position)?;
            if let Some(blocker) = attack_blocker(session)? {
                return Err(format!("mining cannot start: {}", blocker.reason));
            }
            let result = session.interact(InteractionIntent::Attack)?;
            // Without a dig-start packet mining may still be progressing on the
            // target; otherwise the caller must see that nothing started.
            if !result["started"].as_bool().unwrap_or(false)
                && session.player.breaking_crack().map(|(at, _)| at) != Some(position)
            {
                return Err(
                    "mining did not start: the attack executor sent no dig start and nothing is cracking on the target"
                        .into(),
                );
            }
            held.push("attack".into());
            kind = Kind::MineCrack {
                position,
                face,
                start: session.client_tick,
                ticks,
                last_tick: session.client_tick,
            };
        }
        "minecraft_break_block" => {
            require_play(session)?;
            let position = position(args)?;
            let face = face(args)?;
            let item = text(args, "expected_drop_item_id")?.to_owned();
            let count = integer(args, "expected_drop_count", 0, 64)?;
            let initial_count = inventory_count(session, &item)?;
            let initial_revision = session.received_block_revision(position);
            deadline = timeout(args, 8., 120.)?;
            key(session, "attack", false)?;
            aim_block(session, position, face)?;
            require_targeted(session, position)?;
            // A transient gate (destroy delay) is retried by the task each tick;
            // any other refusal is final and reported with its cause.
            let started = match attack_blocker(session)? {
                Some(blocker) if !blocker.transient => {
                    return Err(format!("mining cannot start: {}", blocker.reason))
                }
                Some(_) => false,
                None => session.interact(InteractionIntent::Attack)?["started"]
                    .as_bool()
                    .ok_or("ordinary attack executor omitted actual started fact")?,
            };
            held.push("attack".into());
            kind = Kind::Break {
                position,
                face,
                initial_revision,
                item,
                count,
                initial_count,
                started,
                air: false,
                saw_drop: false,
                collecting: false,
                last_tick: session.client_tick,
                detour: 0,
                preferred: 1,
                detail: String::new(),
            };
        }
        _ => return Ok(None),
    }
    let mut task = Task {
        deadline,
        kind,
        held,
        colliders: Vec::new(),
    };
    for index in 0..task.held.len() {
        if matches!(task.kind, Kind::Break { started: false, .. }) && task.held[index] == "attack" {
            continue;
        }
        if let Err(error) = key(session, &task.held[index], true) {
            task.cancel(session)?;
            return Err(error);
        }
    }
    if matches!(task.kind, Kind::Respawn { .. }) {
        if let Err(error) = session.respawn() {
            task.cancel(session)?;
            return Err(error);
        }
    }
    match task.on_event(session) {
        Ok(Some(result)) => complete(result),
        Ok(None) => Ok(Some(Outcome::Pending(task))),
        Err(error) => {
            let _ = task.cancel(session);
            Err(error)
        }
    }
}
impl Task {
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(super) fn cancel(&mut self, session: &mut Session) -> Result<()> {
        let mut first = None;
        for held in self.held.drain(..) {
            if let Err(error) = key(session, &held, false) {
                first.get_or_insert(error);
            }
        }
        if matches!(self.kind, Kind::Break { .. }) {
            if let Err(error) = clear_pickup(session) {
                first.get_or_insert(error);
            }
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
    pub(super) fn on_event(&mut self, session: &mut Session) -> Result<Option<Value>> {
        let expired = Instant::now() >= self.deadline;
        let result = match &mut self.kind {
            Kind::Play => {
                if (session.phase == "play"
                    && session.play_world_presentation_ready()
                    && session.spawn_meshes_presented())
                    || session.phase == "error"
                    || expired
                {
                    Some(snapshot(session))
                } else {
                    None
                }
            }
            Kind::State { observed } => {
                if session.state_version() != *observed {
                    Some(snapshot(session))
                } else if expired {
                    return Err("client state event did not arrive".into());
                } else {
                    None
                }
            }
            Kind::Ticks { start, ticks } => {
                require_play(session)?;
                if session.player.health.is_some_and(|health| health <= 0.) {
                    return Err("input interrupted because the player died".into());
                }
                if session.client_tick.wrapping_sub(*start) >= *ticks {
                    Some(json!({"status":"ok"}))
                } else if expired {
                    return Err("client did not advance requested ticks within 30 seconds".into());
                } else {
                    None
                }
            }
            Kind::Block {
                position,
                expected,
                properties,
                version,
                events,
            } => {
                require_play(session)?;
                if session.state_version() != *version {
                    *version = session.state_version();
                    *events += 1;
                }
                if expired {
                    return Err(format!(
                        "client block did not reach expected state at {}, {}, {}",
                        position[0], position[1], position[2]
                    ));
                }
                if session
                    .world
                    .chunk([position[0].div_euclid(16), position[2].div_euclid(16)])
                    .is_none()
                    || session
                        .world
                        .light_levels(*position)
                        .iter()
                        .any(Option::is_none)
                {
                    None
                } else {
                    let block = read_block(session, *position, &mut self.colliders)?;
                    if expected
                        .as_ref()
                        .is_none_or(|expected| block["block_id"] == *expected)
                        && properties
                            .iter()
                            .all(|(key, value)| block["properties"].get(key) == Some(value))
                    {
                        Some(if expected.is_some() {
                            json!({"status":"matched","state_events":*events,"block":block})
                        } else {
                            block
                        })
                    } else {
                        None
                    }
                }
            }
            Kind::Respawn {
                health_version,
                ticks,
                target,
            } => {
                if target.is_none()
                    && session.health_version() != *health_version
                    && session.phase == "play"
                    && session.player.health.is_some_and(|health| health > 0.)
                    && !session.minecraft_observation()["screen"]["open"]
                        .as_bool()
                        .unwrap_or(false)
                {
                    if ticks.is_some() {
                        *target = Some(session.client_tick);
                        self.deadline = Instant::now() + Duration::from_secs(30);
                    } else {
                        *target = Some(session.client_tick);
                    }
                }
                if target.is_some() && session.player.health.is_some_and(|health| health <= 0.) {
                    return Err("input interrupted because the player died".into());
                }
                if target.is_some_and(|start| {
                    session.client_tick.wrapping_sub(start) >= ticks.unwrap_or(0)
                }) {
                    Some(json!({"status":"respawned"}))
                } else if Instant::now() >= self.deadline {
                    return Err("client respawn was not confirmed".into());
                } else {
                    None
                }
            }
            Kind::MineCrack {
                position,
                face,
                start,
                ticks,
                last_tick,
            } => {
                require_play(session)?;
                if session.player.health.is_some_and(|health| health <= 0.) {
                    return Err("input interrupted because the player died".into());
                }
                if session.client_tick != *last_tick {
                    aim_block(session, *position, *face)?;
                    *last_tick = session.client_tick;
                }
                if session.client_tick.wrapping_sub(*start) >= *ticks {
                    // Report actual overlay-stage state, never a bare ack: the
                    // playthrough gates its crack screenshot on stage >= 0.
                    let crack = session.player.breaking_crack();
                    let stage = crack
                        .filter(|(pos, _)| pos == position)
                        .map(|(_, stage)| stage);
                    match stage {
                        Some(stage) if (0..=9).contains(&stage) => Some(json!({
                            "status": "cracking",
                            "x": position[0], "y": position[1], "z": position[2],
                            "stage": stage,
                        })),
                        // Block broke early (soft target) or mining never held:
                        // fail loudly instead of idling past the deadline.
                        _ => {
                            return Err(
                                "no crack stage observed on the target after the requested ticks (block broke early or mining was not held)"
                                    .into(),
                            )
                        }
                    }
                } else if expired {
                    return Err("client did not hold mining input within 30 seconds".into());
                } else {
                    None
                }
            }
            Kind::Break {
                position,
                face,
                initial_revision,
                item,
                count,
                initial_count,
                started,
                air,
                saw_drop,
                collecting,
                last_tick,
                detour,
                preferred,
                detail,
            } => {
                require_play(session)?;
                let visible_air =
                    require_play(session)?.block_is_air(read_state(session, *position)?)?;
                if visible_air {
                    // Stop this target's held mining at the local transition, but
                    // never count that prediction as independently received air.
                    key(session, "attack", false)?;
                } else if !*air && session.client_tick != *last_tick {
                    aim_block(session, *position, *face)?;
                    if !*started && attack_blocker(session)?.is_none() {
                        // Retry the ordinary action after client ticks consume a
                        // previous block's destroy delay. Hold only once the
                        // executor reports an actual dispatch. A refused attack is
                        // not swung (a miss would spam arm swings to the server).
                        *started = session.interact(InteractionIntent::Attack)?["started"]
                            .as_bool()
                            .ok_or("ordinary attack executor omitted actual started fact")?;
                        if *started {
                            key(session, "attack", true)?;
                        }
                    }
                }
                let drop = session.entities.nearest_visible_item(
                    item,
                    *position,
                    require_play(session)?,
                    session.client_tick,
                )?;
                *saw_drop |= drop.is_some();
                let received_air = session
                    .received_block(*position)
                    .map(|state| require_play(session)?.block_is_air(state))
                    .transpose()?
                    .unwrap_or(false);
                if session.received_block_revision(*position) > *initial_revision && received_air {
                    *air = true;
                    key(session, "attack", false)?;
                }
                let actual_count = inventory_count(session, item)?;
                let confirmed = actual_count >= *initial_count + *count;
                if *air && *count == 0 {
                    Some(break_result(
                        session, *started, true, *saw_drop, confirmed, detail,
                    ))
                } else {
                    if *air && *saw_drop {
                        *collecting = true;
                    }
                    if *collecting && drop.is_none() && confirmed {
                        Some(break_result(
                            session, *started, true, *saw_drop, true, detail,
                        ))
                    } else if expired {
                        if !*started && !*air {
                            require_targeted(session, *position)?;
                            return Err(match attack_blocker(session)? {
                                Some(blocker) => format!(
                                    "mining never started before the deadline: {}",
                                    blocker.reason
                                ),
                                None => "mining never started before the deadline".into(),
                            });
                        }
                        Some(break_result(
                            session,
                            *started,
                            *air,
                            *saw_drop,
                            *collecting && confirmed,
                            detail,
                        ))
                    } else {
                        if *collecting && session.client_tick != *last_tick {
                            let target = drop
                                .unwrap_or_else(|| position.map(|value| f64::from(value) + 0.5));
                            let approaching = !pickup_reached(session.player.bounds(), target);
                            let next = movement_step(
                                session,
                                target,
                                *detour,
                                *preferred,
                                Some(approaching),
                                &mut self.colliders,
                            )?;
                            if *detour == 0 && next != 0 {
                                *preferred = -next;
                            }
                            *detour = next;
                            *detail = format!(
                                "visible_drop={} inventory_count={} initial_count={} expected_count={} detour={} client_tick={}",
                                drop.is_some(),
                                actual_count,
                                initial_count,
                                count,
                                next,
                                session.client_tick
                            );
                        }
                        *last_tick = session.client_tick;
                        None
                    }
                }
            }
        };
        if result.is_some() {
            self.cancel(session)?;
        }
        Ok(result)
    }
}

#[cfg(test)]
mod chat_kind_tests {
    use super::chat_kind;
    #[test]
    fn leading_slash_is_a_command_without_the_slash() {
        assert_eq!(chat_kind("/help", None).unwrap(), ("help", true));
        assert_eq!(
            chat_kind("/settlement info", None).unwrap(),
            ("settlement info", true)
        );
        assert_eq!(chat_kind("/help", Some(true)).unwrap(), ("help", true));
        assert_eq!(chat_kind("help", Some(true)).unwrap(), ("help", true));
    }
    #[test]
    fn plain_text_and_explicit_false_stay_chat() {
        assert_eq!(chat_kind("hello", None).unwrap(), ("hello", false));
        assert_eq!(
            chat_kind("/not a command", Some(false)).unwrap(),
            ("/not a command", false)
        );
    }
    #[test]
    fn bare_slash_is_rejected() {
        assert!(chat_kind("/", None).is_err());
        assert!(chat_kind("/", Some(true)).is_err());
    }
}

#[cfg(test)]
mod p1_pickup_tests {
    use super::*;
    #[test]
    fn scan_byte_budget_counts_both_mcp_representations() {
        let row = json!({"x":-30_000_000,"y":319,"z":30_000_000,
            "block_id":"minecraft:oak_sign","properties":{"text":"quoted \"value\"\\line"}});
        let mut bytes = ScanResponseBytes::default();
        serde_json::to_writer(&mut bytes, &row).unwrap();
        let plain = row.to_string();
        let escaped = serde_json::to_string(&plain).unwrap();
        assert_eq!(bytes.0, plain.len() + escaped.len() - 2);
        let mut rows = Vec::new();
        while bytes.0 <= 240_000 {
            rows.push(row.clone());
            serde_json::to_writer(&mut bytes, &row).unwrap();
        }
        let value = json!({"blocks":rows});
        let envelope =
            json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value});
        assert!(serde_json::to_vec(&envelope).unwrap().len() < 262144);
    }

    fn standing() -> Bounds {
        Bounds::new([-0.3, 0., -0.3, 0.3, 1.8, 0.3])
    }

    #[test]
    fn p1_pickup_moves_inside_old_two_block_radius_until_item_box_overlaps() {
        for target in [[0., 0., 1.9], [1.9, 0., 0.], [-1.9, 0., 0.]] {
            assert!(target.iter().map(|value| value * value).sum::<f64>() < 4.);
            assert!(!pickup_reached(standing(), target));
        }
        assert!(pickup_reached(standing(), [0., 0., 1.4]));
    }

    #[test]
    fn p1_pickup_requires_strict_horizontal_and_vertical_box_overlap() {
        let body = standing();
        let horizontal_edge = body.max[0] + 1. + 0.125;
        let below_edge = body.min[1] - 0.5 - 0.25;
        let above_edge = body.max[1] + 0.5;
        assert!(!pickup_reached(body, [horizontal_edge, 0., 0.]));
        assert!(pickup_reached(body, [horizontal_edge - 0.001, 0., 0.]));
        assert!(!pickup_reached(body, [0., below_edge, 0.]));
        assert!(pickup_reached(body, [0., below_edge + 0.001, 0.]));
        assert!(!pickup_reached(body, [0., above_edge, 0.]));
        assert!(pickup_reached(body, [0., above_edge - 0.001, 0.]));
    }

    #[test]
    fn p1_pickup_uses_player_pose_box_not_spherical_distance() {
        let target = [1.4, 2.2, 1.4];
        assert!(target.iter().map(|value| value * value).sum::<f64>() > 4.);
        assert!(pickup_reached(standing(), target));
        let crouching = Bounds::new([-0.3, 0., -0.3, 0.3, 1.5, 0.3]);
        assert!(!pickup_reached(crouching, target));
    }
}
