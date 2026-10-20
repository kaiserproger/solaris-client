//! Server-driven 26.1.2 dialogs (`show_dialog` / `clear_dialog`, answered by `custom_click_action`).
//!
//! Layouts, defaults and click semantics follow the local decompiled client
//! (`net.minecraft.server.dialog.*`, `client.gui.screens.dialog.*`):
//! * the dialog is the inline `Dialog.DIRECT_CODEC` NBT; types `notice`, `confirmation` and
//!   `multi_action` are rendered, inputs `text`, `number_range`, `boolean` and `single_option`,
//!   body `plain_message`; `server_links`/`dialog_list` and `item` bodies are not supported;
//! * buttons run `dynamic/custom` (additions + one typed entry per input), static `custom`,
//!   `dynamic/run_command` / `run_command`; other action kinds only apply `after_action`;
//! * `after_action` close/none/wait_for_response, Escape runs `on_cancel` (notice: its button,
//!   confirmation: `no`, multi_action: `exit_action`) and closes only if `can_close_with_escape`;
//! * a new dialog replaces the shown one and keeps the original screen to return to.
//! Not implemented (vanilla extras): the "custom screen" warning button and button tooltips.
use super::state::{NodeAction, Paint, Screen, UiCommand, UiEffects, UiState};
use crate::protocol::{Reader, Result, nbt::Nbt};
use serde_json::{Value, json};
use std::sync::Arc;

const MAX_INPUTS: usize = 64;
const MAX_BUTTONS: usize = 128;
const MAX_BODY: usize = 64;
const HEADER: f32 = 33.;
const FOOTER: f32 = 33.;
const SPACING: f32 = 10.;
const ROW: f32 = 20.;
const WAIT_BACK_VISIBLE_TICKS: u64 = 20;
const WAIT_BACK_ACTIVE_TICKS: u64 = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AfterAction {
    Close,
    None,
    WaitForResponse,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DialogAction {
    /// `dynamic/custom` (`dynamic`) or static `custom`: send `custom_click_action(id, payload)`.
    Custom {
        id: String,
        payload: Option<Nbt>,
        dynamic: bool,
    },
    /// `dynamic/run_command` (template) or static `run_command`.
    RunCommand { command: String, dynamic: bool },
    /// Valid vanilla action this client does not execute (open_url, show_dialog, ...).
    Unsupported(String),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DialogButton {
    label: Value,
    width: f32,
    action: Option<DialogAction>,
}

#[derive(Clone, Debug, PartialEq)]
enum BodyItem {
    Text { contents: Value, width: f32 },
}

#[derive(Clone, Debug, PartialEq)]
enum InputKind {
    Text {
        label: Value,
        label_visible: bool,
        initial: String,
        max_length: usize,
        multiline: bool,
    },
    Number {
        label: Value,
        label_format: String,
        start: f32,
        end: f32,
        initial: Option<f32>,
        step: Option<f32>,
    },
    Bool {
        label: Value,
        initial: bool,
        on_true: String,
        on_false: String,
    },
    Choice {
        label: Value,
        label_visible: bool,
        /// `(id, display)`.
        options: Vec<(String, Value)>,
        initial: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
struct DialogInput {
    key: String,
    width: f32,
    kind: InputKind,
}

#[derive(Clone, Debug, PartialEq)]
enum Layout {
    /// `notice` / `confirmation`: buttons in the footer.
    Simple,
    /// `multi_action`: button grid in the body, optional exit button in the footer.
    Grid { columns: usize, exit: bool },
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ServerDialog {
    title: Value,
    can_close_with_escape: bool,
    after_action: AfterAction,
    body: Vec<BodyItem>,
    inputs: Vec<DialogInput>,
    /// Flattened buttons: notice `[action]`, confirmation `[yes, no]`, multi_action `[actions.., exit?]`.
    buttons: Vec<DialogButton>,
    layout: Layout,
    /// Index into `buttons` run by Escape / window close.
    on_cancel: Option<usize>,
    /// Dialog parts this client skipped (reported through observation).
    skipped: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum InputValue {
    /// Text lives in `UiState::fields`.
    Text,
    Number(f32),
    Bool(bool),
    Choice(usize),
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum DialogControl {
    Button(usize),
    Check(usize),
    Cycle(usize),
    Slide(usize),
    Back,
}

/// One shown dialog plus its live input values.
#[derive(Clone, Debug)]
pub(crate) struct ShownDialog {
    def: Arc<ServerDialog>,
    values: Vec<InputValue>,
    waiting_since: Option<u64>,
}

/// UI-owned dialog state.
#[derive(Default)]
pub(crate) struct DialogHost {
    pub(crate) shown: Option<ShownDialog>,
    /// A dialog received while the loading/death screen owns the UI; shown once it ends.
    pending: Option<Arc<ServerDialog>>,
    /// Screen to return to when the dialog closes (vanilla `previousScreen`).
    return_to: Option<Screen>,
    /// Inclusive count of dialog packets/actions for observation.
    pub(crate) rejected: Vec<String>,
    pub(crate) max_scroll: usize,
    /// Body viewport `[top, bottom]` in GUI pixels from the latest tree build.
    pub(crate) view: [f32; 2],
}

// ---------------------------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------------------------

fn field<'a>(tag: &'a Nbt, key: &str) -> Option<&'a Nbt> {
    tag.get(key)
}

fn required<'a>(tag: &'a Nbt, key: &str) -> Result<&'a Nbt> {
    field(tag, key).ok_or_else(|| format!("dialog field {key} missing"))
}

fn text(tag: &Nbt, key: &str) -> Result<String> {
    required(tag, key)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("dialog field {key} must be a string"))
}

fn component(tag: &Nbt, key: &str) -> Result<Value> {
    Ok(required(tag, key)?.to_json())
}

fn optional_bool(tag: &Nbt, key: &str, default: bool) -> Result<bool> {
    match field(tag, key) {
        None => Ok(default),
        Some(value) => value
            .as_bool()
            .ok_or_else(|| format!("dialog field {key} must be a boolean")),
    }
}

fn width(tag: &Nbt, key: &str, default: f32) -> Result<f32> {
    match field(tag, key) {
        None => Ok(default),
        Some(value) => match value.as_i64() {
            Some(width) if (1..=1024).contains(&width) => Ok(width as f32),
            _ => Err(format!("dialog field {key} must be in 1..=1024")),
        },
    }
}

fn float(tag: &Nbt, key: &str) -> Result<f32> {
    required(tag, key)?
        .as_f64()
        .filter(|v| v.is_finite())
        .map(|v| v as f32)
        .ok_or_else(|| format!("dialog field {key} must be a finite number"))
}

fn optional_float(tag: &Nbt, key: &str) -> Result<Option<f32>> {
    if field(tag, key).is_none() {
        return Ok(None);
    }
    float(tag, key).map(Some)
}

/// `type` is an identifier; the default namespace is optional.
fn type_name(tag: &Nbt) -> Result<String> {
    let raw = text(tag, "type")?;
    Ok(raw.strip_prefix("minecraft:").unwrap_or(&raw).to_owned())
}

/// Compact list codec: a list, or a single element.
fn compact_list(tag: Option<&Nbt>) -> Vec<&Nbt> {
    match tag {
        None => Vec::new(),
        Some(Nbt::List(_, items)) => items.iter().collect(),
        Some(single) => vec![single],
    }
}

fn valid_variable(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
}

fn identifier(value: &str) -> Result<String> {
    crate::protocol::parse_identifier(value)
}

fn parse_action(tag: &Nbt) -> Result<DialogAction> {
    let kind = type_name(tag)?;
    Ok(match kind.as_str() {
        "dynamic/custom" => DialogAction::Custom {
            id: identifier(&text(tag, "id")?)?,
            payload: match field(tag, "additions") {
                None => None,
                Some(additions @ Nbt::Compound(_)) => Some(additions.clone()),
                Some(_) => return Err("dialog additions must be a compound".into()),
            },
            dynamic: true,
        },
        "custom" => DialogAction::Custom {
            id: identifier(&text(tag, "id")?)?,
            payload: field(tag, "payload").cloned(),
            dynamic: false,
        },
        "dynamic/run_command" => {
            let template = text(tag, "template")?;
            if !template.contains("$(") {
                return Err("dialog template has no variables".into());
            }
            DialogAction::RunCommand {
                command: template,
                dynamic: true,
            }
        }
        "run_command" => DialogAction::RunCommand {
            command: text(tag, "command")?,
            dynamic: false,
        },
        "open_url" | "suggest_command" | "show_dialog" | "change_page" | "copy_to_clipboard" => {
            DialogAction::Unsupported(kind)
        }
        other => return Err(format!("unknown dialog action type {other}")),
    })
}

fn parse_button(tag: &Nbt) -> Result<DialogButton> {
    Ok(DialogButton {
        label: component(tag, "label")?,
        width: width(tag, "width", 150.)?,
        action: match field(tag, "action") {
            None => None,
            Some(action) => Some(parse_action(action)?),
        },
    })
}

fn parse_input(tag: &Nbt) -> Result<DialogInput> {
    let key = text(tag, "key")?;
    if !valid_variable(&key) {
        return Err(format!("{key} is not a valid input name"));
    }
    let w = width(tag, "width", 200.)?;
    let kind = match type_name(tag)?.as_str() {
        "text" => {
            let initial = match field(tag, "initial") {
                None => String::new(),
                Some(value) => value
                    .as_str()
                    .ok_or("dialog text initial must be a string")?
                    .to_owned(),
            };
            let max_length = match field(tag, "max_length") {
                None => 32,
                Some(value) => match value.as_i64() {
                    Some(n) if (1..=1_048_576).contains(&n) => n as usize,
                    _ => return Err("dialog text max_length must be positive".into()),
                },
            };
            if initial.encode_utf16().count() > max_length {
                return Err("Default text length exceeds allowed size".into());
            }
            InputKind::Text {
                label: component(tag, "label")?,
                label_visible: optional_bool(tag, "label_visible", true)?,
                initial,
                max_length,
                multiline: field(tag, "multiline").is_some(),
            }
        }
        "number_range" => {
            let (start, end) = (float(tag, "start")?, float(tag, "end")?);
            let initial = optional_float(tag, "initial")?;
            if let Some(initial) = initial
                && (initial < start.min(end) || initial > start.max(end))
            {
                return Err("Initial value is outside of range".into());
            }
            let step = optional_float(tag, "step")?;
            if step.is_some_and(|step| step <= 0.) {
                return Err("dialog step must be positive".into());
            }
            InputKind::Number {
                label: component(tag, "label")?,
                label_format: match field(tag, "label_format") {
                    None => "options.generic_value".into(),
                    Some(value) => value
                        .as_str()
                        .ok_or("dialog label_format must be a string")?
                        .to_owned(),
                },
                start,
                end,
                initial,
                step,
            }
        }
        "boolean" => InputKind::Bool {
            label: component(tag, "label")?,
            initial: optional_bool(tag, "initial", false)?,
            on_true: match field(tag, "on_true") {
                None => "true".into(),
                Some(value) => value
                    .as_str()
                    .ok_or("dialog on_true must be a string")?
                    .to_owned(),
            },
            on_false: match field(tag, "on_false") {
                None => "false".into(),
                Some(value) => value
                    .as_str()
                    .ok_or("dialog on_false must be a string")?
                    .to_owned(),
            },
        },
        "single_option" => {
            let entries = compact_list(field(tag, "options"));
            if entries.is_empty() {
                return Err("dialog single_option needs options".into());
            }
            let mut options = Vec::with_capacity(entries.len());
            let mut initial = None;
            for (index, entry) in entries.into_iter().enumerate() {
                let (id, display, is_initial) = match entry {
                    Nbt::String(id) => (id.clone(), json!(id), false),
                    full => {
                        let id = text(full, "id")?;
                        let display =
                            field(full, "display").map_or_else(|| json!(id), Nbt::to_json);
                        (id, display, optional_bool(full, "initial", false)?)
                    }
                };
                if is_initial {
                    if initial.is_some() {
                        return Err("Multiple initial values".into());
                    }
                    initial = Some(index);
                }
                options.push((id, display));
            }
            InputKind::Choice {
                label: component(tag, "label")?,
                label_visible: optional_bool(tag, "label_visible", true)?,
                options,
                initial: initial.unwrap_or(0),
            }
        }
        other => return Err(format!("unknown dialog input type {other}")),
    };
    Ok(DialogInput {
        key,
        width: w,
        kind,
    })
}

impl ServerDialog {
    /// `Ok(None)`: a well-formed dialog of a type this client does not support.
    pub(crate) fn parse(tag: &Nbt) -> Result<std::result::Result<Self, String>> {
        if !matches!(tag, Nbt::Compound(_)) {
            return Err("dialog must be a compound".into());
        }
        let kind = type_name(tag)?;
        if !matches!(kind.as_str(), "notice" | "confirmation" | "multi_action") {
            return Ok(Err(format!("unsupported dialog type {kind}")));
        }
        let title = component(tag, "title")?;
        let can_close_with_escape = optional_bool(tag, "can_close_with_escape", true)?;
        let pause = optional_bool(tag, "pause", true)?;
        let after_action = match field(tag, "after_action").map(Nbt::as_str) {
            None => AfterAction::Close,
            Some(Some("close")) => AfterAction::Close,
            Some(Some("none")) => AfterAction::None,
            Some(Some("wait_for_response")) => AfterAction::WaitForResponse,
            Some(_) => return Err("unknown dialog after_action".into()),
        };
        if pause && after_action == AfterAction::None {
            return Err("Dialogs that pause the game must use after_action values that unpause it after user action!".into());
        }
        let mut skipped = Vec::new();
        let mut body = Vec::new();
        for item in compact_list(field(tag, "body")) {
            match type_name(item)?.as_str() {
                "plain_message" => body.push(BodyItem::Text {
                    contents: component(item, "contents")?,
                    width: width(item, "width", 200.)?,
                }),
                other => skipped.push(format!("body {other}")),
            }
        }
        let mut inputs = Vec::new();
        for input in compact_list(field(tag, "inputs")) {
            inputs.push(parse_input(input)?);
        }
        let (buttons, layout, on_cancel) = match kind.as_str() {
            "notice" => {
                let button = match field(tag, "action") {
                    Some(action) => parse_button(action)?,
                    None => DialogButton {
                        label: json!({"translate": "gui.ok"}),
                        width: 150.,
                        action: None,
                    },
                };
                (vec![button], Layout::Simple, Some(0))
            }
            "confirmation" => (
                vec![
                    parse_button(required(tag, "yes")?)?,
                    parse_button(required(tag, "no")?)?,
                ],
                Layout::Simple,
                Some(1),
            ),
            _ => {
                let actions = compact_list(field(tag, "actions"));
                if actions.is_empty() {
                    return Err("dialog actions must not be empty".into());
                }
                let mut buttons = actions
                    .into_iter()
                    .map(parse_button)
                    .collect::<Result<Vec<_>>>()?;
                let exit = field(tag, "exit_action").map(parse_button).transpose()?;
                let on_cancel = exit.as_ref().map(|_| buttons.len());
                let has_exit = exit.is_some();
                buttons.extend(exit);
                let columns = match field(tag, "columns") {
                    None => 2,
                    Some(value) => match value.as_i64() {
                        Some(n) if n >= 1 => n.min(64) as usize,
                        _ => return Err("dialog columns must be positive".into()),
                    },
                };
                (
                    buttons,
                    Layout::Grid {
                        columns,
                        exit: has_exit,
                    },
                    on_cancel,
                )
            }
        };
        if inputs.len() > MAX_INPUTS || buttons.len() > MAX_BUTTONS || body.len() > MAX_BODY {
            return Err("dialog element limit".into());
        }
        let mut keys = std::collections::BTreeSet::new();
        if inputs.iter().any(|input| !keys.insert(input.key.as_str())) {
            return Err("duplicate dialog input key".into());
        }
        Ok(Ok(Self {
            title,
            can_close_with_escape,
            after_action,
            body,
            inputs,
            buttons,
            layout,
            on_cancel,
            skipped,
        }))
    }
}

// ---------------------------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------------------------

/// Java `Math.round(float)`.
fn java_round(value: f32) -> f32 {
    (value + 0.5).floor()
}

fn inverse_lerp(value: f32, start: f32, end: f32) -> f32 {
    (value - start) / (end - start)
}

/// `NumberRangeInput.RangeInfo.initialScaledValue`.
fn initial_scaled(start: f32, end: f32, initial: Option<f32>) -> f32 {
    initial.unwrap_or((start + end) / 2.)
}

fn scaled_to_slider(start: f32, end: f32, value: f32) -> f32 {
    if start == end {
        0.5
    } else {
        inverse_lerp(value, start, end)
    }
}

/// `NumberRangeInput.RangeInfo.computeScaledValue` on f32, like the vanilla client.
pub(crate) fn compute_scaled_value(
    slider: f32,
    start: f32,
    end: f32,
    initial: Option<f32>,
    step: Option<f32>,
) -> f32 {
    let in_range = start + slider * (end - start);
    let Some(step) = step else {
        return in_range;
    };
    let initial = initial_scaled(start, end, initial);
    let steps = java_round((in_range - initial) / step);
    let result = initial + steps * step;
    let slider_pos = scaled_to_slider(start, end, result);
    if !(slider_pos < 0. || slider_pos > 1.) {
        return result;
    }
    let sign = if steps > 0. {
        1.
    } else if steps < 0. {
        -1.
    } else {
        0.
    };
    initial + (steps - sign) * step
}

fn initial_slider(start: f32, end: f32, initial: Option<f32>) -> f32 {
    scaled_to_slider(start, end, initial_scaled(start, end, initial))
}

/// Vanilla `valueToString`: whole numbers print as integers.
fn number_text(value: f32) -> String {
    let int = value as i32;
    if int as f32 == value {
        int.to_string()
    } else {
        format!("{value}")
    }
}

/// `StringTag.escapeWithoutQuotes`.
fn escape_without_quotes(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' | '\'' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c => out.push(c),
        }
    }
    out
}

/// `StringTemplate` substitution; unknown variables become the empty string.
fn instantiate(template: &str, values: &std::collections::BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find("$(") {
        let Some(end) = rest[start..].find(')') else {
            break;
        };
        out.push_str(&rest[..start]);
        let name = &rest[start + 2..start + end];
        out.push_str(values.get(name).map_or("", String::as_str));
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

impl ShownDialog {
    fn new(def: Arc<ServerDialog>) -> Self {
        let values = def
            .inputs
            .iter()
            .map(|input| match &input.kind {
                InputKind::Text { .. } => InputValue::Text,
                InputKind::Number {
                    start,
                    end,
                    initial,
                    ..
                } => InputValue::Number(initial_slider(*start, *end, *initial)),
                InputKind::Bool { initial, .. } => InputValue::Bool(*initial),
                InputKind::Choice { initial, .. } => InputValue::Choice(*initial),
            })
            .collect();
        Self {
            def,
            values,
            waiting_since: None,
        }
    }

    fn scaled(&self, index: usize) -> Option<f32> {
        match (&self.def.inputs[index].kind, &self.values[index]) {
            (
                InputKind::Number {
                    start,
                    end,
                    initial,
                    step,
                    ..
                },
                InputValue::Number(slider),
            ) => Some(compute_scaled_value(*slider, *start, *end, *initial, *step)),
            _ => None,
        }
    }

    /// `(typed NBT value, template substitution)` per input, in dialog order.
    fn collected(
        &self,
        fields: &std::collections::BTreeMap<String, super::edit::EditBuffer>,
    ) -> Vec<(String, Nbt, String)> {
        self.def
            .inputs
            .iter()
            .enumerate()
            .map(|(index, input)| {
                let (tag, substitution) = match (&input.kind, &self.values[index]) {
                    (InputKind::Text { .. }, _) => {
                        let value = fields
                            .get(&field_id(&input.key))
                            .map(|f| f.text.clone())
                            .unwrap_or_default();
                        let escaped = escape_without_quotes(&value);
                        (Nbt::String(value), escaped)
                    }
                    (InputKind::Number { .. }, _) => {
                        let value = self.scaled(index).unwrap_or_default();
                        (Nbt::Float(value), number_text(value))
                    }
                    (
                        InputKind::Bool {
                            on_true, on_false, ..
                        },
                        InputValue::Bool(on),
                    ) => (
                        Nbt::Byte(i8::from(*on)),
                        if *on {
                            on_true.clone()
                        } else {
                            on_false.clone()
                        },
                    ),
                    (InputKind::Choice { options, .. }, InputValue::Choice(selected)) => {
                        let id = options[*selected].0.clone();
                        (Nbt::String(id.clone()), id)
                    }
                    _ => (Nbt::String(String::new()), String::new()),
                };
                (input.key.clone(), tag, substitution)
            })
            .collect()
    }
}

fn field_id(key: &str) -> String {
    format!("dialog/input/{key}")
}

// ---------------------------------------------------------------------------------------------
// UiState integration
// ---------------------------------------------------------------------------------------------

impl UiState {
    /// `show_dialog` (140) / `clear_dialog` (139). Malformed dialogs fail the packet like the
    /// vanilla decoder; unsupported dialog types are ignored and reported through observation.
    pub(crate) fn apply_dialog_packet(&mut self, id: i32, r: &mut Reader<'_>) -> Result<()> {
        match id {
            139 => {
                r.done()?;
                self.clear_server_dialog();
            }
            140 => {
                let holder = r.var()?;
                if holder != 0 {
                    // Registry dialogs need the configuration `dialog` registry; not decoded here.
                    let _ = r.take(r.bytes.len() - r.offset)?;
                    self.dialogs
                        .rejected
                        .push("registry dialog reference".into());
                    return Ok(());
                }
                let tag = r.nbt_typed()?;
                r.done()?;
                match ServerDialog::parse(&tag)? {
                    Ok(dialog) => self.show_server_dialog(Arc::new(dialog))?,
                    Err(reason) => self.dialogs.rejected.push(reason),
                }
            }
            _ => return Err("not a dialog packet".into()),
        }
        Ok(())
    }

    pub(crate) fn server_dialog_open(&self) -> bool {
        self.screen == Screen::ServerDialog
    }

    pub(crate) fn dialog_observation(&self) -> Value {
        let shown = self.dialogs.shown.as_ref();
        json!({
            "open": self.screen == Screen::ServerDialog,
            "waiting": shown.is_some_and(|d| d.waiting_since.is_some()),
            "title": shown.and_then(|d| self.text.language.plain(&d.def.title).ok()),
            "buttons": shown.map(|d| d.def.buttons.len()),
            "inputs": shown.map(|d| d.def.inputs.iter().map(|i| i.key.clone()).collect::<Vec<_>>()),
            "skipped": shown.map(|d| d.def.skipped.clone()),
            "rejected": self.dialogs.rejected,
        })
    }

    fn show_server_dialog(&mut self, def: Arc<ServerDialog>) -> Result<()> {
        if matches!(self.screen, Screen::Loading | Screen::Death) {
            self.dialogs.pending = Some(def);
            return Ok(());
        }
        self.open_server_dialog(def)
    }

    /// Called when the loading/death screen ends.
    pub(crate) fn dialog_after_blocking_screen(&mut self) -> Result<()> {
        if let Some(def) = self.dialogs.pending.take() {
            self.open_server_dialog(def)?;
        }
        Ok(())
    }

    fn open_server_dialog(&mut self, def: Arc<ServerDialog>) -> Result<()> {
        if self.screen != Screen::ServerDialog {
            self.dialogs.return_to = Some(self.screen);
        }
        self.fields.retain(|id, _| !id.starts_with("dialog/"));
        for input in &def.inputs {
            if let InputKind::Text {
                initial,
                max_length,
                multiline,
                ..
            } = &input.kind
            {
                self.field(
                    &field_id(&input.key),
                    initial.clone(),
                    max_length * 4,
                    *multiline,
                )?;
                if let Some(buffer) = self.fields.get_mut(&field_id(&input.key)) {
                    buffer.max_chars = Some(*max_length);
                }
            }
        }
        self.dialogs.shown = Some(ShownDialog::new(def));
        self.scroll = 0;
        self.transition(Screen::ServerDialog);
        self.invalidate();
        Ok(())
    }

    fn clear_server_dialog(&mut self) {
        self.dialogs.pending = None;
        if self.screen != Screen::ServerDialog {
            return;
        }
        self.close_server_dialog();
    }

    /// Back to the screen that was active before the first dialog.
    fn close_server_dialog(&mut self) {
        let back = self.dialogs.return_to.take().unwrap_or(Screen::Game);
        self.dialogs.shown = None;
        self.fields.retain(|id, _| !id.starts_with("dialog/"));
        self.transition(if back == Screen::ServerDialog {
            Screen::Game
        } else {
            back
        });
        self.invalidate();
    }

    /// Escape: run `on_cancel`, closing only when the dialog allows it
    /// (`DialogScreen.onClose` + `shouldCloseOnEsc`).
    pub(crate) fn dialog_escape(&mut self, e: &mut UiEffects) -> Result<()> {
        let Some(shown) = &self.dialogs.shown else {
            return Ok(());
        };
        if shown.waiting_since.is_some() {
            if self.dialog_wait_ticks() >= WAIT_BACK_ACTIVE_TICKS {
                self.close_server_dialog();
            }
            return Ok(());
        }
        if !shown.def.can_close_with_escape {
            return Ok(());
        }
        match shown.def.on_cancel {
            Some(index) => self.run_dialog_button(index, e),
            None => {
                self.close_server_dialog();
                Ok(())
            }
        }
    }

    fn dialog_wait_ticks(&self) -> u64 {
        self.dialogs
            .shown
            .as_ref()
            .and_then(|shown| shown.waiting_since)
            .map_or(0, |since| self.tick.saturating_sub(since))
    }

    /// Waiting screen countdown: repaint when the Back button changes state.
    pub(crate) fn dialog_tick(&mut self) {
        if self
            .dialogs
            .shown
            .as_ref()
            .is_some_and(|shown| shown.waiting_since.is_some())
        {
            let ticks = self.dialog_wait_ticks();
            if ticks % 20 == 0 || ticks >= WAIT_BACK_ACTIVE_TICKS {
                self.dirty = true;
            }
        }
    }

    pub(crate) fn activate_dialog(
        &mut self,
        control: DialogControl,
        e: &mut UiEffects,
    ) -> Result<()> {
        match control {
            DialogControl::Back => {
                if self.dialog_wait_ticks() < WAIT_BACK_ACTIVE_TICKS {
                    return Err("waiting_for_response".into());
                }
                self.close_server_dialog();
            }
            DialogControl::Button(index) => self.run_dialog_button(index, e)?,
            DialogControl::Check(index) => {
                let shown = self.dialogs.shown.as_mut().ok_or("dialog_not_open")?;
                if let Some(InputValue::Bool(on)) = shown.values.get_mut(index) {
                    *on = !*on;
                }
                self.invalidate();
            }
            DialogControl::Cycle(index) => {
                let shown = self.dialogs.shown.as_mut().ok_or("dialog_not_open")?;
                if let (Some(InputValue::Choice(selected)), InputKind::Choice { options, .. }) =
                    (shown.values.get_mut(index), &shown.def.inputs[index].kind)
                {
                    *selected = (*selected + 1) % options.len();
                }
                self.invalidate();
            }
            DialogControl::Slide(index) => {
                // Pointer drags set the value directly; an activation (Enter, automation)
                // steps to the next value and wraps, like the Options sliders.
                let shown = self.dialogs.shown.as_ref().ok_or("dialog_not_open")?;
                self.focus = Some(format!("dialog/input/{}", shown.def.inputs[index].key));
                let next = match (&shown.def.inputs[index].kind, shown.scaled(index)) {
                    (
                        InputKind::Number {
                            start, end, step, ..
                        },
                        Some(scaled),
                    ) => {
                        let delta = step.unwrap_or((end - start) / 10.);
                        let next = scaled_to_slider(*start, *end, scaled + delta);
                        if next > 1.0001 { 0. } else { next }
                    }
                    _ => return Ok(()),
                };
                self.set_dialog_slider(index, next);
                self.invalidate();
            }
        }
        Ok(())
    }

    /// Keyboard on a focused dialog slider; `true` when consumed.
    pub(crate) fn dialog_slider_key(&mut self, key: &str) -> bool {
        let Some(index) = self.focus.as_deref().and_then(|id| {
            self.nodes
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| match n.action {
                    NodeAction::Dialog(DialogControl::Slide(index)) => Some(index),
                    _ => None,
                })
        }) else {
            return false;
        };
        let direction = match key {
            "ArrowLeft" | "ArrowDown" => -1.,
            "ArrowRight" | "ArrowUp" => 1.,
            "Home" => return self.set_dialog_slider(index, 0.),
            "End" => return self.set_dialog_slider(index, 1.),
            _ => return false,
        };
        // Vanilla AbstractSliderButton keyboard step: 1% of the track.
        let current = match self.dialogs.shown.as_ref().map(|d| &d.values[index]) {
            Some(InputValue::Number(slider)) => *slider,
            _ => return false,
        };
        self.set_dialog_slider(index, current + direction * 0.01)
    }

    /// Pointer drag/click on a dialog slider track.
    pub(crate) fn dialog_slider_pointer(&mut self, id: &str) -> bool {
        let Some((index, bounds)) =
            self.nodes
                .iter()
                .find(|n| n.id == id)
                .and_then(|n| match n.action {
                    NodeAction::Dialog(DialogControl::Slide(index)) => Some((index, n.bounds)),
                    _ => None,
                })
        else {
            return false;
        };
        let (track_x, track_w) = super::options::track_span(bounds);
        let t = ((self.pointer[0] - track_x - 3.) / (track_w - 6.).max(1.)).clamp(0., 1.);
        self.set_dialog_slider(index, t)
    }

    fn set_dialog_slider(&mut self, index: usize, value: f32) -> bool {
        let Some(shown) = self.dialogs.shown.as_mut() else {
            return false;
        };
        if let Some(InputValue::Number(slider)) = shown.values.get_mut(index) {
            *slider = value.clamp(0., 1.);
            self.invalidate();
            return true;
        }
        false
    }

    fn run_dialog_button(&mut self, index: usize, e: &mut UiEffects) -> Result<()> {
        let shown = self.dialogs.shown.as_ref().ok_or("dialog_not_open")?;
        if shown.waiting_since.is_some() {
            return Err("waiting_for_response".into());
        }
        let button = shown
            .def
            .buttons
            .get(index)
            .ok_or("dialog_button_missing")?;
        let after = shown.def.after_action;
        let values = shown.collected(&self.fields);
        let command = match button.action.clone() {
            Some(DialogAction::Custom {
                id,
                payload,
                dynamic,
            }) => {
                let payload = if dynamic {
                    let mut entries = match payload {
                        Some(Nbt::Compound(entries)) => entries,
                        _ => Vec::new(),
                    };
                    for (key, tag, _) in &values {
                        match entries.iter_mut().find(|(name, _)| name == key) {
                            Some(entry) => entry.1 = tag.clone(),
                            None => entries.push((key.clone(), tag.clone())),
                        }
                    }
                    Some(Nbt::Compound(entries))
                } else {
                    payload
                };
                Some(UiCommand::DialogClick { id, payload })
            }
            Some(DialogAction::RunCommand { command, dynamic }) => {
                let command = if dynamic {
                    let map = values
                        .iter()
                        .map(|(key, _, substitution)| (key.clone(), substitution.clone()))
                        .collect();
                    instantiate(&command, &map)
                } else {
                    command
                };
                Some(UiCommand::Chat {
                    message: command.strip_prefix('/').unwrap_or(&command).to_owned(),
                    command: true,
                })
            }
            Some(DialogAction::Unsupported(kind)) => {
                self.dialogs
                    .rejected
                    .push(format!("action {kind} not executed"));
                None
            }
            None => None,
        };
        match after {
            AfterAction::None => {}
            AfterAction::Close => self.close_server_dialog(),
            AfterAction::WaitForResponse => {
                if let Some(shown) = self.dialogs.shown.as_mut() {
                    shown.waiting_since = Some(self.tick);
                }
                self.focus = None;
            }
        }
        e.commands.extend(command);
        self.invalidate();
        Ok(())
    }

    /// Vanilla screen class + title the canonical Minecraft tools match against.
    pub(crate) fn dialog_screen_metadata(&self) -> Option<(&'static str, String)> {
        let shown = self.dialogs.shown.as_ref()?;
        if shown.waiting_since.is_some() {
            return Some((
                "net.minecraft.client.gui.screens.dialog.WaitingForResponseScreen",
                self.dialog_plain(&json!({"translate": "gui.waitingForResponse.title"})),
            ));
        }
        let class = match shown.def.layout {
            Layout::Simple => "net.minecraft.client.gui.screens.dialog.SimpleDialogScreen",
            Layout::Grid { .. } => {
                "net.minecraft.client.gui.screens.dialog.MultiButtonDialogScreen"
            }
        };
        Some((class, self.dialog_plain(&shown.def.title)))
    }

    fn dialog_plain(&self, component: &Value) -> String {
        self.text
            .language
            .plain(component)
            .unwrap_or_else(|_| component.as_str().unwrap_or_default().to_owned())
    }

    fn dialog_text_height(&self, component: &Value, width: f32) -> f32 {
        self.text
            .layout(component, width.max(1.))
            .map_or(9., |layout| layout.height.max(9.))
    }

    fn dialog_label(&mut self, id: &str, component: &Value, rect: [f32; 4]) -> Result<()> {
        let label = self.dialog_plain(component);
        self.label(id, label, rect)?;
        if let Some(node) = self.nodes.last_mut() {
            node.component = Some(Arc::new(component.clone()));
        }
        Ok(())
    }

    /// Semantic tree for `Screen::ServerDialog`.
    pub(crate) fn dialog_tree(&mut self) -> Result<()> {
        self.panel();
        let Some(shown) = self.dialogs.shown.clone() else {
            return Err("dialog_state_missing".into());
        };
        let def = Arc::clone(&shown.def);
        if let Some(since) = shown.waiting_since {
            return self.waiting_tree(since);
        }
        let (w, h) = (self.width, self.height);
        // Header: centered title.
        let title_w = (w - 16.).clamp(1., 400.);
        let title_h = self.dialog_text_height(&def.title, title_w);
        self.dialog_label(
            "dialog/title",
            &def.title,
            [
                ((w - title_w) / 2.).floor(),
                ((HEADER - title_h) / 2.).floor().max(2.),
                title_w,
                title_h,
            ],
        )?;
        // Footer.
        let footer_h = match def.layout {
            Layout::Simple => FOOTER,
            Layout::Grid { exit: true, .. } => FOOTER,
            Layout::Grid { exit: false, .. } => 5.,
        };
        let footer_buttons: Vec<usize> = match def.layout {
            Layout::Simple => (0..def.buttons.len()).collect(),
            Layout::Grid { exit: true, .. } => vec![def.buttons.len() - 1],
            Layout::Grid { exit: false, .. } => Vec::new(),
        };
        if !footer_buttons.is_empty() {
            let widths: Vec<f32> = footer_buttons
                .iter()
                .map(|i| def.buttons[*i].width.min(w - 8.))
                .collect();
            let total = widths.iter().sum::<f32>() + 8. * (widths.len() - 1) as f32;
            let mut x = ((w - total) / 2.).floor();
            let y = (h - footer_h + ((footer_h - ROW) / 2.).floor()).floor();
            for (index, width) in footer_buttons.iter().zip(widths) {
                self.dialog_button(*index, &def.buttons[*index], [x, y, width, ROW])?;
                x += width + 8.;
            }
        }
        // Body: collect element heights, then place with scroll.
        let content_w = (w - 16.).max(1.);
        enum Element {
            Text(usize, f32),
            Input(usize, f32),
            Grid(f32),
        }
        let mut elements = Vec::new();
        for (i, item) in def.body.iter().enumerate() {
            let BodyItem::Text { contents, width } = item;
            elements.push(Element::Text(
                i,
                self.dialog_text_height(contents, width.min(content_w)),
            ));
        }
        for (i, input) in def.inputs.iter().enumerate() {
            let label_height = match &input.kind {
                InputKind::Text {
                    label,
                    label_visible: true,
                    ..
                } => self.dialog_text_height(label, input.width.min(content_w)) + 4.,
                _ => 0.,
            };
            let control = match &input.kind {
                InputKind::Text {
                    multiline: true, ..
                } => 4. * 9. + 8.,
                _ => ROW,
            };
            elements.push(Element::Input(i, label_height + control));
        }
        let grid_columns = match def.layout {
            Layout::Grid { columns, exit } => {
                let count = def.buttons.len() - usize::from(exit);
                Some((columns, count))
            }
            Layout::Simple => None,
        };
        if let Some((columns, count)) = grid_columns {
            let rows = count.div_ceil(columns);
            elements.push(Element::Grid(
                rows as f32 * ROW + rows.saturating_sub(1) as f32 * 2.,
            ));
        }
        let heights: Vec<f32> = elements
            .iter()
            .map(|element| match element {
                Element::Text(_, h) | Element::Input(_, h) | Element::Grid(h) => *h,
            })
            .collect();
        let content_h =
            heights.iter().sum::<f32>() + SPACING * heights.len().saturating_sub(1) as f32;
        let view_top = HEADER;
        let view_h = (h - footer_h - HEADER).max(1.);
        let max_scroll = (content_h - view_h).max(0.).ceil() as usize;
        self.dialogs.max_scroll = max_scroll.div_ceil(12);
        self.scroll = self.scroll.min(self.dialogs.max_scroll);
        let offset = (self.scroll * 12) as f32;
        let mut y = if content_h <= view_h {
            view_top + ((view_h - content_h) / 2.).floor()
        } else {
            view_top - offset
        };
        let first_body_node = self.nodes.len();
        for element in &elements {
            match element {
                Element::Text(i, height) => {
                    let BodyItem::Text { contents, width } = &def.body[*i];
                    let width = width.min(content_w);
                    self.dialog_label(
                        &format!("dialog/body/{i}"),
                        contents,
                        [((w - width) / 2.).floor(), y, width, *height],
                    )?;
                    y += height + SPACING;
                }
                Element::Input(i, height) => {
                    self.dialog_input_nodes(
                        &shown,
                        *i,
                        ((w - def.inputs[*i].width.min(content_w)) / 2.).floor(),
                        y,
                        content_w,
                    )?;
                    y += height + SPACING;
                }
                Element::Grid(height) => {
                    let (columns, count) = grid_columns.unwrap_or((1, 0));
                    self.dialog_grid(&def, columns, count, y)?;
                    y += height + SPACING;
                }
            }
        }
        // Text overlapping the viewport paints clipped to it (every wrapped line stays reachable
        // by scrolling); other controls are shown only when entirely inside.
        let (clip_top, clip_bottom) = (view_top, view_top + view_h);
        self.dialogs.view = [clip_top, clip_bottom];
        for node in &mut self.nodes[first_body_node..] {
            if node.role == "text" {
                let clip = dialog_clip(node.bounds, clip_top, clip_bottom);
                node.visible = clip.is_some();
                node.clip = clip.filter(|clip| *clip != node.bounds);
            } else if node.bounds[1] < clip_top || node.bounds[1] + node.bounds[3] > clip_bottom {
                node.visible = false;
            }
        }
        Ok(())
    }

    /// Scroll the open dialog just far enough that body node `id` lies inside the viewport, as a
    /// wheel would. Returns whether the scroll offset changed.
    pub(crate) fn dialog_reveal(&mut self, id: &str) -> bool {
        if self.screen != Screen::ServerDialog {
            return false;
        }
        let Some(node) = self.nodes.iter().find(|n| n.id == id) else {
            return false;
        };
        let [top, bottom] = self.dialogs.view;
        let [_, y, _, h] = node.bounds;
        let delta = if y < top {
            y - top
        } else if y + h > bottom {
            // A node taller than the viewport is aligned by its top edge.
            (y + h - bottom).min(y - top)
        } else {
            return false;
        };
        let steps = (delta.abs() / 12.).ceil() as usize;
        let target = if delta < 0. {
            self.scroll.saturating_sub(steps)
        } else {
            self.scroll
                .saturating_add(steps)
                .min(self.dialogs.max_scroll)
        };
        let moved = target != self.scroll;
        self.scroll = target;
        if moved {
            self.invalidate();
        }
        moved
    }

    fn dialog_button(&mut self, index: usize, button: &DialogButton, rect: [f32; 4]) -> Result<()> {
        let label = self.dialog_plain(&button.label);
        let id = format!("dialog/action/{index}");
        self.button(
            &id,
            &label,
            rect,
            NodeAction::Dialog(DialogControl::Button(index)),
            true,
            None,
        )?;
        if let Some(node) = self.nodes.last_mut() {
            node.component = Some(Arc::new(button.label.clone()));
        }
        Ok(())
    }

    fn dialog_grid(
        &mut self,
        def: &ServerDialog,
        columns: usize,
        count: usize,
        top: f32,
    ) -> Result<()> {
        let w = self.width;
        let max = (w - 8.).max(1.);
        let rows = count.div_ceil(columns);
        let mut y = top;
        for row in 0..rows {
            let start = row * columns;
            let in_row = (count - start).min(columns);
            // Full rows align to shared column widths; the last partial row is centered.
            let widths: Vec<f32> = if in_row == columns {
                (0..columns)
                    .map(|col| {
                        (0..rows)
                            .filter_map(|r| {
                                let i = r * columns + col;
                                (i < count && (r + 1) * columns <= count)
                                    .then(|| def.buttons[i].width)
                            })
                            .fold(0., f32::max)
                            .min(max)
                    })
                    .collect()
            } else {
                (start..start + in_row)
                    .map(|i| def.buttons[i].width.min(max))
                    .collect()
            };
            let total = widths.iter().sum::<f32>() + 2. * (in_row.saturating_sub(1)) as f32;
            let mut x = ((w - total) / 2.).floor();
            for (offset, width) in widths.iter().enumerate() {
                let index = start + offset;
                self.dialog_button(index, &def.buttons[index], [x, y, *width, ROW])?;
                x += width + 2.;
            }
            y += ROW + 2.;
        }
        Ok(())
    }

    fn dialog_input_nodes(
        &mut self,
        shown: &ShownDialog,
        index: usize,
        x: f32,
        y: f32,
        content_w: f32,
    ) -> Result<()> {
        let input = &shown.def.inputs[index];
        let width = input.width.min(content_w);
        match (&input.kind, &shown.values[index]) {
            (
                InputKind::Text {
                    label,
                    label_visible,
                    multiline,
                    ..
                },
                _,
            ) => {
                let mut control_y = y;
                if *label_visible {
                    let label_h = self.dialog_text_height(label, width);
                    self.dialog_label(
                        &format!("{}/label", field_id(&input.key)),
                        label,
                        [x, y, width, label_h],
                    )?;
                    control_y += label_h + 4.;
                }
                let control_h = if *multiline { 4. * 9. + 8. } else { ROW };
                let plain = self.dialog_plain(label);
                self.edit_node(
                    &field_id(&input.key),
                    &plain,
                    [x, control_y, width, control_h],
                    true,
                )?;
            }
            (
                InputKind::Number {
                    label,
                    label_format,
                    ..
                },
                InputValue::Number(slider),
            ) => {
                let scaled = shown.scaled(index).unwrap_or_default();
                let message = json!({"translate": label_format, "fallback": "%s: %s", "with": [label, number_text(scaled)]});
                let text = self.dialog_plain(&message);
                self.add(
                    format!("dialog/input/{}", input.key),
                    "slider",
                    text,
                    [x, y, width, ROW],
                    true,
                    None,
                    NodeAction::Dialog(DialogControl::Slide(index)),
                    Paint::Slider {
                        t: *slider,
                        desc: "",
                    },
                )?;
            }
            (InputKind::Bool { label, .. }, InputValue::Bool(on)) => {
                let text = format!(
                    "{}: {}",
                    self.dialog_plain(label),
                    if *on { "ON" } else { "OFF" }
                );
                self.add(
                    format!("dialog/input/{}", input.key),
                    "switch",
                    text,
                    [x, y, width, ROW],
                    true,
                    None,
                    NodeAction::Dialog(DialogControl::Check(index)),
                    Paint::Toggle { on: *on, desc: "" },
                )?;
            }
            (
                InputKind::Choice {
                    label,
                    label_visible,
                    options,
                    ..
                },
                InputValue::Choice(selected),
            ) => {
                let value = self.dialog_plain(&options[*selected].1);
                let text = if *label_visible {
                    format!("{}: {value}", self.dialog_plain(label))
                } else {
                    value
                };
                self.button(
                    &format!("dialog/input/{}", input.key),
                    &text,
                    [x, y, width, ROW],
                    NodeAction::Dialog(DialogControl::Cycle(index)),
                    true,
                    None,
                )?;
            }
            _ => {}
        }
        Ok(())
    }

    fn waiting_tree(&mut self, since: u64) -> Result<()> {
        let (w, h) = (self.width, self.height);
        let title = json!({"translate": "gui.waitingForResponse.title"});
        self.dialog_label("dialog/title", &title, [0., 12., w, 12.])?;
        let ticks = self.tick.saturating_sub(since);
        if ticks >= WAIT_BACK_VISIBLE_TICKS {
            let secs = (ticks / 20) as usize;
            let (label, active) = if ticks >= WAIT_BACK_ACTIVE_TICKS {
                (self.dialog_plain(&json!({"translate": "gui.back"})), true)
            } else {
                (
                    self.dialog_plain(&json!({
                        "translate": "gui.waitingForResponse.button.inactive",
                        "with": [(5 - secs).to_string()],
                    })),
                    false,
                )
            };
            self.button(
                "dialog/back",
                &label,
                [((w - 200.) / 2.).floor(), (h / 2.).floor(), 200., ROW],
                NodeAction::Dialog(DialogControl::Back),
                active,
                if active {
                    None
                } else {
                    Some("waiting_for_response")
                },
            )?;
        }
        Ok(())
    }
}

/// Vertical intersection of a body node with the scroll viewport; `None` when they do not overlap.
fn dialog_clip(bounds: [f32; 4], top: f32, bottom: f32) -> Option<[f32; 4]> {
    let [x, y, w, h] = bounds;
    let (from, to) = (y.max(top), (y + h).min(bottom));
    (to > from).then_some([x, from, w, to - from])
}

#[cfg(test)]
#[path = "dialog_tests.rs"]
mod tests;
