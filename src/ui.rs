//! Native UI uses the same semantic executor for physical input and typed automation.
mod advancements;
mod anim;
mod catalogue;
mod chat_tracking;
mod clipping;
mod display;
mod edit;
pub mod font;
mod ghost;
pub mod hash;
mod hud;
mod hud_case;
mod hud_locator;
mod hud_math;
mod hud_view;
mod interaction;
pub mod items;
pub mod layout;
mod links;
mod loader;
mod mcp;
mod menus;
mod operator;
mod options;
mod packets;
mod palette;
mod recipe_grid;
pub mod render;
mod sample;
mod screens;
pub mod shaping;
mod state;
mod statistics;
pub mod text;
mod theme;
mod tint;
mod ttf;
mod unihex;
mod widgets;
mod worlds;
use crate::inventory::UiContext;
pub use chat_tracking::LastSeenUpdate;
pub use hud::{
    LocatorCamera, NumberFormat, Objective, ScoreEntry, ScoreRenderType, ScoreTeam,
    ScoreboardState, TeamCollision, TeamParameters, TeamVisibility, TrackedWaypoint, WaypointIcon,
    WaypointId, WaypointPosition, WaypointState,
};
pub use packets::{
    AdvancementState, BossBar, ChatLine, ChatReceipt, ChatVerification, MapDecoration, MapState,
};
use serde::{Deserialize, Serialize};
pub use state::*;

/// JsonOps Codec.INT narrows Gson's exact decimal Number through intValue().
fn codec_int(value: &serde_json::Value) -> crate::protocol::Result<i32> {
    let number = value
        .as_number()
        .ok_or("Source Codec.INT expects a JSON number")?;
    codec_int_token(number.as_str())
}
fn codec_int_token(token: &str) -> crate::protocol::Result<i32> {
    let (negative, token) = token
        .strip_prefix('-')
        .map_or((false, token), |rest| (true, rest));
    let (mantissa, exponent) = match token.split_once(['e', 'E']) {
        Some((mantissa, exponent)) => (
            mantissa,
            exponent
                .parse::<i32>()
                .map_err(|_| "Source Codec.INT decimal exponent")?,
        ),
        None => (token, 0),
    };
    let decimal = mantissa.find('.');
    let point = decimal.unwrap_or(mantissa.len());
    let digits = mantissa.len() - usize::from(decimal.is_some());
    let fractional = i32::try_from(digits - point).map_err(|_| "Source Codec.INT decimal scale")?;
    let scale = fractional
        .checked_sub(exponent)
        .ok_or("Source Codec.INT decimal scale")?;
    if scale <= -32 {
        return Ok(0);
    }
    let keep = if scale > 0 {
        digits.saturating_sub(scale as usize)
    } else {
        digits
    };
    let end = keep + usize::from(decimal.is_some() && keep > point);
    // Higher decimal places vanish modulo 2^32; retain at most 32 digits and one dot.
    let mut integer = 0u32;
    for &byte in &mantissa.as_bytes()[end.saturating_sub(33)..end] {
        if byte != b'.' {
            integer = integer
                .wrapping_mul(10)
                .wrapping_add(u32::from(byte - b'0'));
        }
    }
    if scale < 0 {
        integer = integer.wrapping_mul(10u32.wrapping_pow((-scale) as u32));
    }
    Ok(if negative {
        integer.wrapping_neg()
    } else {
        integer
    } as i32)
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub logo: bool,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointerPhase {
    Move,
    Down,
    Up,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClickMode {
    Pickup,
    QuickMove,
    Swap,
    Clone,
    Throw,
    QuickCraft,
    PickupAll,
}
impl ClickMode {
    pub fn wire(self) -> i32 {
        match self {
            Self::Pickup => 0,
            Self::QuickMove => 1,
            Self::Swap => 2,
            Self::Clone => 3,
            Self::Throw => 4,
            Self::QuickCraft => 5,
            Self::PickupAll => 6,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiIntent {
    Activate {
        context: UiContext,
        element_id: String,
    },
    Focus {
        context: UiContext,
        element_id: String,
    },
    ScrollTarget {
        context: UiContext,
        element_id: String,
        delta_y: f32,
        delta_x: f32,
    },
    OpenInventory,
    /// Ordinary Game -> Pause (same transition the physical Escape key
    /// drives). Fail-closed unless the Game screen is open.
    OpenPause,
    Close {
        context: UiContext,
    },
    ClickSlot {
        context: UiContext,
        slot: i16,
        button: u8,
        mode: ClickMode,
    },
    MenuButton {
        context: UiContext,
        button: i32,
    },
    Beacon {
        context: UiContext,
        primary: Option<i32>,
        secondary: Option<i32>,
    },
    SlotToggle {
        context: UiContext,
        slot: i16,
        enabled: bool,
    },
    Rename {
        context: UiContext,
        text: String,
    },
    PlaceRecipe {
        context: UiContext,
        display_id: i32,
        craft_all: bool,
    },
    RecipeBook {
        context: UiContext,
        open: bool,
        filter: bool,
    },
    BundleSelect {
        context: UiContext,
        slot: i16,
        index: i32,
    },
    CreativeSlot {
        context: UiContext,
        slot: i16,
        item_id: u32,
        count: i32,
    },
    OpenChat {
        command: bool,
    },
    SendChat {
        text: String,
    },
    EditSignLine {
        context: UiContext,
        line: u8,
        text: String,
    },
    SubmitSign {
        context: UiContext,
    },
    EditBookPage {
        context: UiContext,
        page: u16,
        text: String,
    },
    SubmitBook {
        context: UiContext,
        title: Option<String>,
    },
    Pointer {
        phase: PointerPhase,
        x: f64,
        y: f64,
        button: Option<PointerButton>,
        modifiers: Modifiers,
    },
    Scroll {
        x: f64,
        y: f64,
        delta_x: f32,
        delta_y: f32,
        modifiers: Modifiers,
    },
    Key {
        key: String,
        pressed: bool,
        modifiers: Modifiers,
    },
    Text {
        text: String,
    },
    Composition {
        text: String,
        cursor: Option<[usize; 2]>,
    },
    SelectText {
        start: usize,
        end: usize,
    },
}

/// Closed shape shared by the MCP catalog and the ordinary typed input parser.
/// Availability and context fences belong to the UI executor, not this schema.
pub fn intent_schema() -> serde_json::Value {
    use serde_json::{json, Value};
    use std::sync::LazyLock;
    static SCHEMA: LazyLock<Value> = LazyLock::new(|| {
        fn object(fields: Value, required: &[&str]) -> Value {
            json!({"type":"object","properties":fields,"required":required,"additionalProperties":false})
        }
        fn action(name: &str, fields: Value) -> Value {
            let mut properties = fields
                .as_object()
                .expect("schema fields are objects")
                .clone();
            properties.insert("action".into(), json!({"const":name}));
            let required: Vec<_> = properties.keys().map(String::as_str).collect();
            object(json!(properties), &required)
        }
        let context = object(
            json!({
                "screen_instance":{"type":"string","pattern":"^(0|[1-9][0-9]{0,19})$"},
                "revision":{"type":"string","pattern":"^(0|[1-9][0-9]{0,19})$"},
                "ui_revision":{"type":"string","pattern":"^(0|[1-9][0-9]{0,19})$"},
                "container_id":{"type":"integer","minimum":-2147483648_i64,"maximum":2147483647},
                "state_id":{"type":"integer","minimum":-2147483648_i64,"maximum":2147483647}
            }),
            &[
                "screen_instance",
                "revision",
                "ui_revision",
                "container_id",
                "state_id",
            ],
        );
        let modifiers = object(
            json!({"shift":{"type":"boolean"},"control":{"type":"boolean"},"alt":{"type":"boolean"},"logo":{"type":"boolean"}}),
            &["shift", "control", "alt", "logo"],
        );
        let slot = json!({"type":"integer","minimum":-32768,"maximum":32767});
        let text = json!({"type":"string"});
        let index = json!({"type":"integer","minimum":0});
        json!({"oneOf":[
            action("activate",json!({"context":context,"element_id":{"type":"string","maxLength":256}})),
            action("focus",json!({"context":context,"element_id":{"type":"string","maxLength":256}})),
            action("scroll_target",json!({"context":context,"element_id":{"type":"string","maxLength":256},"delta_x":{"type":"number"},"delta_y":{"type":"number"}})),
            action("open_inventory",json!({})),
            action("close",json!({"context":context})),
            action("click_slot",json!({"context":context,"slot":slot,"button":{"type":"integer","minimum":0,"maximum":255},"mode":{"enum":["pickup","quick_move","swap","clone","throw","quick_craft","pickup_all"]}})),
            action("menu_button",json!({"context":context,"button":{"type":"integer","minimum":-2147483648_i64,"maximum":2147483647}})),
            action("beacon",json!({"context":context,"primary":{"type":["integer","null"],"minimum":0,"maximum":2147483647},"secondary":{"type":["integer","null"],"minimum":0,"maximum":2147483647}})),
            action("slot_toggle",json!({"context":context,"slot":slot,"enabled":{"type":"boolean"}})),
            action("rename",json!({"context":context,"text":text})),
            action("place_recipe",json!({"context":context,"display_id":{"type":"integer","minimum":0,"maximum":2147483647},"craft_all":{"type":"boolean"}})),
            action("recipe_book",json!({"context":context,"open":{"type":"boolean"},"filter":{"type":"boolean"}})),
            action("bundle_select",json!({"context":context,"slot":slot,"index":{"type":"integer","minimum":-1,"maximum":2147483647}})),
            action("creative_slot",json!({"context":context,"slot":slot,"item_id":{"type":"integer","minimum":0,"maximum":4294967295_u64},"count":{"type":"integer","minimum":-2147483648_i64,"maximum":2147483647}})),
            action("open_chat",json!({"command":{"type":"boolean"}})),
            action("send_chat",json!({"text":text})),
            action("edit_sign_line",json!({"context":context,"line":{"type":"integer","minimum":0,"maximum":255},"text":text})),
            action("submit_sign",json!({"context":context})),
            action("edit_book_page",json!({"context":context,"page":{"type":"integer","minimum":0,"maximum":65535},"text":text})),
            action("submit_book",json!({"context":context,"title":{"type":["string","null"]}})),
            action("pointer",json!({"phase":{"enum":["move","down","up"]},"x":{"type":"number"},"y":{"type":"number"},"button":{"enum":["left","right","middle",null]},"modifiers":modifiers})),
            action("scroll",json!({"x":{"type":"number"},"y":{"type":"number"},"delta_x":{"type":"number"},"delta_y":{"type":"number"},"modifiers":modifiers})),
            action("key",json!({"key":text,"pressed":{"type":"boolean"},"modifiers":modifiers})),
            action("text",json!({"text":text})),
            action("composition",json!({"text":text,"cursor":{"anyOf":[{"type":"null"},{"type":"array","items":index,"minItems":2,"maxItems":2}]}})),
            action("select_text",json!({"start":index,"end":index}))
        ]})
    });
    SCHEMA.clone()
}
