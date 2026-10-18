//! Java-compatible inventory tools over the ordinary input/UI owners and received facts.
use super::Outcome;
use crate::{
    api::InputIntent,
    inventory::{Stack, UiContext},
    protocol::{content::Content, Result},
    session::Session,
    ui::{ClickMode, UiIntent},
};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
struct Checkpoint {
    container: i32,
    state: i32,
    packets: u64,
}
impl Checkpoint {
    fn capture(session: &Session) -> Self {
        let menu = session.inventory.received_menu();
        Self {
            container: menu.id,
            state: menu.state_id,
            packets: session.container_packet_version(),
        }
    }
    fn advanced(self, session: &Session, click: bool) -> bool {
        let menu = session.inventory.received_menu();
        if click {
            session.container_packet_version() != self.packets
                && (menu.id != self.container || menu.state_id != self.state)
        } else {
            menu.id == self.container && menu.state_id != self.state
        }
    }
}

enum Wait {
    ItemVisibility {
        item: String,
        origin: [i32; 3],
        visible: bool,
    },
    Inventory {
        item: String,
        count: i32,
    },
    Slot {
        item: String,
        count: i32,
        slot: usize,
    },
    Selection {
        item: String,
        count: i32,
        swap: Option<(Checkpoint, u8)>,
    },
    Update {
        checkpoint: Checkpoint,
        click: bool,
        result: Value,
    },
    Drop {
        item: String,
        count: i32,
        before: i32,
        origin: [i32; 3],
        debited: Option<i32>,
    },
}
pub(super) struct Task {
    deadline: Instant,
    timeout: Duration,
    wait: Wait,
}
impl Task {
    pub(super) fn deadline(&self) -> Instant {
        self.deadline
    }
    pub(super) fn cancel(&mut self, _session: &mut Session) -> Result<()> {
        // All ordinary key edges are released synchronously. There is no held input to cancel.
        Ok(())
    }
    pub(super) fn on_event(&mut self, session: &mut Session) -> Result<Option<Value>> {
        require_play(session)?;
        match &mut self.wait {
            Wait::ItemVisibility {
                item,
                origin,
                visible,
            } => {
                if super::entities::visible_item(session, item, *origin)? == *visible {
                    return Ok(Some(json!({
                        "matched":true,"item_id":item,"visible":visible,
                        "x":origin[0],"y":origin[1],"z":origin[2]
                    })));
                }
            }
            Wait::Inventory { item, count } => {
                if inventory_count(session, item)? == Some(*count) {
                    return Ok(Some(json!({"matched":true,"item_id":item,"count":count})));
                }
            }
            Wait::Slot { item, count, slot } => {
                if matches(
                    session
                        .inventory
                        .received_menu()
                        .slots
                        .get(*slot)
                        .and_then(Option::as_ref),
                    content(session)?,
                    item,
                    *count,
                )? {
                    return Ok(Some(
                        json!({"matched":true,"slot":slot,"item_id":item,"count":count}),
                    ));
                }
            }
            Wait::Selection { item, count, swap } => {
                if let Some((checkpoint, target)) = *swap {
                    if !checkpoint.advanced(session, false) {
                        return Ok(None);
                    }
                    *swap = None;
                    if session.inventory.selected_slot == target
                        && matches(selected(session)?, content(session)?, item, *count)?
                    {
                        return Ok(Some(selected_result(session)?));
                    }
                }
                for slot in 0..9 {
                    if matches(
                        session.inventory.received_player_slot(36 + slot)?,
                        content(session)?,
                        item,
                        *count,
                    )? {
                        session.input(InputIntent::Hotbar { slot: slot as u8 })?;
                        return Ok(Some(selected_result(session)?));
                    }
                }
                let mut source = None;
                for (slot, stack) in session
                    .inventory
                    .received_menu()
                    .slots
                    .iter()
                    .enumerate()
                    .rev()
                {
                    if matches(stack.as_ref(), content(session)?, item, *count)? {
                        source = Some(slot);
                        break;
                    }
                }
                if let Some(source) = source {
                    let selected = session.inventory.selected_slot;
                    let mut target = selected;
                    if !session
                        .inventory
                        .received_player_slot(36 + u16::from(selected))?
                        .is_some_and(Stack::empty)
                    {
                        for slot in 0..9 {
                            if session
                                .inventory
                                .received_player_slot(36 + slot)?
                                .is_some_and(Stack::empty)
                            {
                                target = slot as u8;
                                break;
                            }
                        }
                    }
                    let checkpoint = Checkpoint::capture(session);
                    let context = context(session)?;
                    session.execute_ui(UiIntent::ClickSlot {
                        context,
                        slot: source as i16,
                        button: target,
                        mode: ClickMode::Swap,
                    })?;
                    session.input(InputIntent::Hotbar { slot: target })?;
                    *swap = Some((checkpoint, target));
                }
            }
            Wait::Update {
                checkpoint,
                click,
                result,
            } => {
                if checkpoint.advanced(session, *click) {
                    return Ok(Some(result.take()));
                }
            }
            Wait::Drop {
                item,
                count,
                before,
                origin,
                debited,
            } => {
                if debited.is_none() {
                    let Some(after) = inventory_count(session, item)? else {
                        return Ok(None);
                    };
                    if after > (*before - *count).max(0) {
                        return Ok(None);
                    }
                    *debited = Some(after);
                    // The Java facade gives the visibility phase its own timeout after debit.
                    self.deadline = Instant::now() + self.timeout;
                }
                if super::entities::visible_item(session, item, *origin)? {
                    return Ok(Some(
                        json!({"status":"confirmed","item_id":item,"count":count,"inventory_before":before,"inventory_after":debited.unwrap(),"visible":true,"x":origin[0],"y":origin[1],"z":origin[2]}),
                    ));
                }
            }
        }
        // Every other Minecraft task fails at its deadline; without this a click the server
        // never echoes would hold the single writer slot forever.
        if Instant::now() >= self.deadline {
            return Err("container wait timed out without its received-state predicate".into());
        }
        Ok(None)
    }
}

pub(super) fn start(
    session: &mut Session,
    name: &str,
    args: &Value,
) -> Result<Option<Outcome<Task>>> {
    let wait = match name {
        "minecraft_wait_for_visible_item" | "minecraft_wait_for_no_visible_item" => {
            Wait::ItemVisibility {
                item: text(args, "item_id")?.to_owned(),
                origin: [
                    integer(args, "x", i32::MIN, i32::MAX)?,
                    integer(args, "y", i32::MIN, i32::MAX)?,
                    integer(args, "z", i32::MIN, i32::MAX)?,
                ],
                visible: name == "minecraft_wait_for_visible_item",
            }
        }
        "minecraft_wait_for_inventory" => Wait::Inventory {
            item: text(args, "item_id")?.to_owned(),
            count: integer(args, "count", 0, 4096)?,
        },
        "minecraft_wait_for_container_slot" => Wait::Slot {
            item: text(args, "item_id")?.to_owned(),
            count: integer(args, "count", 1, 4096)?,
            slot: integer(args, "slot", 0, i16::MAX as i32)? as usize,
        },
        "minecraft_select_hotbar_item" => Wait::Selection {
            item: text(args, "item_id")?.to_owned(),
            count: integer(args, "count", 1, 64)?,
            swap: None,
        },
        "minecraft_set_hotbar_slot" => {
            require_play(session)?;
            session.input(InputIntent::Hotbar {
                slot: integer(args, "slot", 0, 8)? as u8,
            })?;
            return Ok(Some(Outcome::Complete(json!({"status":"ok"}))));
        }
        "minecraft_open_inventory" => {
            require_play(session)?;
            session.execute_ui(UiIntent::OpenInventory)?;
            return Ok(Some(Outcome::Complete(json!({"status":"ok"}))));
        }
        "minecraft_open_pause_menu" => {
            // Ordinary Game -> Pause path (same transition the physical
            // Escape key drives via UiState::key). Fail-closed unless the
            // Game screen is open: UiIntent::OpenPause refuses on dialogs.
            require_play(session)?;
            session.execute_ui(UiIntent::OpenPause)?;
            return Ok(Some(Outcome::Complete(json!({"status":"ok"}))));
        }
        "minecraft_close_screen" => {
            require_play(session)?;
            let context = context(session)?;
            session.execute_ui(UiIntent::Close { context })?;
            return Ok(Some(Outcome::Complete(json!({"status":"ok"}))));
        }
        "minecraft_set_screen_input" => {
            // Title screen runs pre-play (phase != play): allow the input but
            // route through the title UI executor (fail-closed elsewhere).
            let value = text(args, "value")?;
            let content = session.content.as_ref().ok_or("content_not_ready")?;
            let (focus, length) = session.ui.as_mut().ok_or("ui_not_ready")?.exact_input(
                &session.inventory,
                content,
                text(args, "expected_screen_class")?,
                text(args, "expected_title")?,
                text(args, "input_label")?,
                value,
            )?;
            if session.phase != "play" {
                session.execute_title_ui_for_mcp(focus)?;
                session.execute_title_ui_for_mcp(UiIntent::SelectText {
                    start: 0,
                    end: length,
                })?;
                session.execute_title_ui_for_mcp(UiIntent::Text { text: value.into() })?;
            } else {
                session.execute_ui(focus)?;
                session.execute_ui(UiIntent::SelectText {
                    start: 0,
                    end: length,
                })?;
                session.execute_ui(UiIntent::Text { text: value.into() })?;
            }
            return Ok(Some(Outcome::Complete(json!({"status":"ok"}))));
        }
        "minecraft_click_screen_button" | "minecraft_click_confirmation_button" => {
            // Title Connect/Quit run pre-play: same executor split.
            let confirmation = name == "minecraft_click_confirmation_button";
            let expected_class = if confirmation {
                None
            } else {
                Some(text(args, "expected_screen_class")?)
            };
            let content = session.content.as_ref().ok_or("content_not_ready")?;
            let intent = session.ui.as_mut().ok_or("ui_not_ready")?.exact_button(
                &session.inventory,
                content,
                expected_class,
                text(args, "expected_title")?,
                text(args, "button_label")?,
                confirmation,
            )?;
            if session.phase != "play" {
                session.execute_title_ui_for_mcp(intent)?;
            } else {
                session.execute_ui(intent)?;
            }
            return Ok(Some(Outcome::Complete(json!({"status":"ok"}))));
        }
        "minecraft_read_item_components" => {
            require_play(session)?;
            return Ok(Some(Outcome::Complete(read_components(session, args)?)));
        }
        "minecraft_read_recipe_book" => {
            require_play(session)?;
            let limit = if args.get("limit").is_some() {
                integer(args, "limit", 1, 8192)? as usize
            } else {
                2048
            };
            return Ok(Some(Outcome::Complete(recipe_book(session, limit))));
        }
        "minecraft_quick_move_container_slot"
        | "minecraft_click_container_slot"
        | "minecraft_click_container_button" => {
            require_play(session)?;
            let checkpoint = Checkpoint::capture(session);
            let click = name == "minecraft_click_container_slot";
            let result;
            let intent;
            if name == "minecraft_click_container_button" {
                let button = integer(args, "button_id", 0, i32::MAX)?;
                intent = UiIntent::MenuButton {
                    context: context(session)?,
                    button,
                };
                result = json!({"confirmed":true,"button_id":button});
            } else {
                let slot = integer(args, "slot", 0, i16::MAX as i32)? as i16;
                if slot as usize >= session.inventory.received_menu().slots.len() {
                    return Err("container slot is outside menu size".into());
                }
                if click && !session.ui.as_ref().is_some_and(|ui| ui.visible_container()) {
                    return Err("no visible container menu is open".into());
                }
                let button = if click {
                    match text(args, "button")? {
                        "primary" => 0,
                        "secondary" => 1,
                        _ => return Err("button must be primary or secondary".into()),
                    }
                } else {
                    0
                };
                intent = UiIntent::ClickSlot {
                    context: context(session)?,
                    slot,
                    button,
                    mode: if click {
                        ClickMode::Pickup
                    } else {
                        ClickMode::QuickMove
                    },
                };
                result = if click {
                    json!({"confirmed":true,"slot":slot,"button":args["button"]})
                } else {
                    json!({"confirmed":true,"slot":slot})
                };
            }
            let before =
                (name != "minecraft_click_container_button").then(|| menu_snapshot(session));
            session.execute_ui(intent)?;
            // A click the ordinary container executor predicts to change nothing (result
            // slot while carrying something else, empty slot with empty cursor, ...) sends
            // a ClickContainer the server answers with no state change, so waiting for a
            // state-id advance would only time out. Report it as the local no-op it is.
            if before.is_some_and(|before| before == menu_snapshot(session)) {
                let mut result = result;
                result["confirmed"] = json!(false);
                result["no_op"] = json!(true);
                result["origin"] = json!("local_prediction");
                return Ok(Some(Outcome::Complete(result)));
            }
            Wait::Update {
                checkpoint,
                click,
                result,
            }
        }
        "minecraft_drop_selected_item" => {
            require_play(session)?;
            let item = text(args, "item_id")?.to_owned();
            let count = integer(args, "count", 1, 64)?;
            if !matches(selected(session)?, content(session)?, &item, count)? {
                return Err("selected item does not match drop request".into());
            }
            let before =
                inventory_count(session, &item)?.ok_or("inventory baseline not observed")?;
            let origin = session.player.position.map(|value| value.floor() as i32);
            // Source player.drop(false) drops one item, even when the requested debit is larger.
            let pressed = session.input(InputIntent::Key {
                key: "drop".into(),
                pressed: true,
            });
            let released = session.input(InputIntent::Key {
                key: "drop".into(),
                pressed: false,
            });
            pressed?;
            released?;
            Wait::Drop {
                item,
                count,
                before,
                origin,
                debited: None,
            }
        }
        _ => return Ok(None),
    };
    require_play(session)?;
    let timeout = timeout(args)?;
    let mut task = Task {
        deadline: Instant::now() + timeout,
        timeout,
        wait,
    };
    Ok(Some(match task.on_event(session)? {
        Some(value) => Outcome::Complete(value),
        None => Outcome::Pending(task),
    }))
}
/// Predicted menu slots plus carried stack, for no-op click detection.
fn menu_snapshot(session: &Session) -> (Vec<Option<Stack>>, Option<Stack>) {
    (
        session.inventory.menu().slots.clone(),
        session.inventory.cursor.clone(),
    )
}
fn require_play(session: &Session) -> Result<()> {
    if session.phase != "play" {
        return Err("client is not in play".into());
    }
    Ok(())
}
fn content(session: &Session) -> Result<&Content> {
    session
        .content
        .as_ref()
        .ok_or_else(|| "content_not_ready".into())
}
fn context(session: &mut Session) -> Result<UiContext> {
    let content = session.content.as_ref().ok_or("content unavailable")?;
    let ui = session.ui.as_mut().ok_or("ui_not_ready")?;
    // Ordinary UI execution synchronizes first. Capture its revision after the
    // same synchronization, not from the previous received inventory generation.
    ui.synchronize(&session.inventory, content)?;
    Ok(ui.context(&session.inventory))
}
fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args[key]
        .as_str()
        .ok_or_else(|| format!("{key} must be a string"))
}
fn integer(args: &Value, key: &str, min: i32, max: i32) -> Result<i32> {
    let value = args[key]
        .as_i64()
        .ok_or_else(|| format!("{key} must be an integer"))?;
    if value < i64::from(min) || value > i64::from(max) {
        return Err(format!("{key} outside {min}..{max}"));
    }
    Ok(value as i32)
}
fn timeout(args: &Value) -> Result<Duration> {
    let seconds = args
        .get("timeout_seconds")
        .map(|v| v.as_f64().ok_or("timeout_seconds must be a number"))
        .transpose()?
        .unwrap_or(8.);
    if !seconds.is_finite() || !(0.1..=120.).contains(&seconds) {
        return Err("timeout_seconds outside 0.1..120".into());
    }
    Ok(Duration::from_secs_f64(seconds))
}
fn player_slot(slot: u16) -> Result<u16> {
    Ok(match slot {
        0..=8 => 36 + slot,
        9..=35 => slot,
        36..=39 => 44 - slot,
        40 => 45,
        41..=42 => slot + 5,
        _ => return Err("player slot is outside inventory size 43".into()),
    })
}
fn selected(session: &Session) -> Result<Option<&Stack>> {
    session
        .inventory
        .received_player_slot(36 + u16::from(session.inventory.selected_slot))
}
fn item_name<'a>(stack: &Stack, content: &'a Content) -> Result<&'a str> {
    if stack.empty() {
        Ok("minecraft:air")
    } else {
        content
            .item_names
            .get(&stack.id)
            .map(String::as_str)
            .ok_or_else(|| "unknown received item ID".into())
    }
}
fn matches(stack: Option<&Stack>, content: &Content, item: &str, count: i32) -> Result<bool> {
    match stack {
        Some(stack) if !stack.empty() && stack.count >= count => {
            Ok(item_name(stack, content)? == item)
        }
        _ => Ok(false),
    }
}
fn selected_result(session: &Session) -> Result<Value> {
    let stack = selected(session)?.ok_or("selected stack not observed")?;
    Ok(json!({"selected":true,"item_id":item_name(stack, content(session)?)?,"count":stack.count}))
}
fn inventory_count(session: &Session, item: &str) -> Result<Option<i32>> {
    let content = content(session)?;
    for slot in 5..46 {
        if session.inventory.received_player_slot(slot)?.is_none() {
            return Ok(None);
        }
    }
    let mut count = 0;
    for slot in 0..43 {
        let menu_slot = player_slot(slot)?;
        // The two auxiliary equipment slots start EMPTY in Source. Until their
        // first receipt they contribute nothing; never consult mutable prediction
        // to complete an authoritative inventory wait.
        let stack = session.inventory.received_player_slot(menu_slot)?;
        if let Some(stack) = stack {
            if !stack.empty() && item_name(stack, content)? == item {
                count += stack.count;
            }
        }
    }
    Ok(Some(count))
}
fn read_components(session: &Session, args: &Value) -> Result<Value> {
    let source = text(args, "source")?;
    let slot;
    let stack;
    let mut result = json!({"source":source});
    match source {
        "selected" => {
            if args.get("slot").is_some() {
                return Err("selected source does not accept a slot".into());
            }
            slot = u16::from(session.inventory.selected_slot);
            stack = selected(session)?;
        }
        "player" => {
            slot = integer(args, "slot", 0, i16::MAX as i32)? as u16;
            stack = session.inventory.received_player_slot(player_slot(slot)?)?;
        }
        "container" => {
            if !session.ui.as_ref().is_some_and(|ui| ui.visible_container()) {
                return Err("no visible container menu is open".into());
            }
            slot = integer(args, "slot", 0, i16::MAX as i32)? as u16;
            let menu = session.inventory.received_menu();
            stack = menu
                .slots
                .get(usize::from(slot))
                .ok_or("container slot is outside menu size")?
                .as_ref();
            result["container_id"] = json!(menu.id);
            result["container_state_id"] = json!(menu.state_id);
        }
        _ => return Err("source must be selected, player, or container".into()),
    }
    let stack = stack.ok_or("stack not observed")?;
    result["slot"] = json!(slot);
    result["item_id"] = json!(item_name(stack, content(session)?)?);
    result["count"] = json!(if stack.empty() { 0 } else { stack.count });
    let mut components = serde_json::Map::new();
    if !stack.empty() {
        for (name, value) in stack.components.iter() {
            let value = if name == "minecraft:custom_data" {
                json!({"snbt":crate::inventory::snbt::encode(stack.typed_component(name).ok_or("custom data typed value not observed")?)?})
            } else {
                value.clone()
            };
            components.insert(name.clone(), value);
        }
    }
    result["components"] = Value::Object(components);
    Ok(result)
}
fn recipe_book(session: &Session, limit: usize) -> Value {
    let recipes = &session.inventory.recipes;
    let mut groups = BTreeSet::new();
    let mut collection_count = 0;
    for recipe in recipes.values() {
        if recipe.group.is_none()
            || groups.insert((recipe.category.as_str(), recipe.group.unwrap()))
        {
            collection_count += 1;
        }
    }
    let ids: Vec<_> = recipes.keys().copied().take(limit).collect();
    let prefix = recipes
        .keys()
        .copied()
        .zip(0..)
        .take_while(|(id, expected)| id == expected)
        .count();
    json!({"collection_count":collection_count,"entry_count":recipes.len(),"listed_count":ids.len(),"truncated":ids.len()<recipes.len(),"contiguous_prefix_count":prefix,"min_display_id":recipes.keys().next().copied().unwrap_or(-1),"max_display_id":recipes.keys().next_back().copied().unwrap_or(-1),"display_ids":ids})
}
