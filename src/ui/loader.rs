//! Schema-2 declarative views use the retained native controls, not a packet editor.
use super::{
    edit::EditBuffer,
    state::{NodeAction, Paint, Screen, UiCommand, UiEffects, UiState},
};
use crate::loader::previews::{PreviewCatalog, validate_references};
use crate::protocol::Result;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct LoaderView {
    definition: Value,
    model: Value,
    pub(crate) instance: String,
    pub(crate) title: String,
    revision: u64,
    sequence: u64,
    selections: BTreeMap<String, usize>,
    previews: PreviewCatalog,
    pub(crate) preview_meshes: BTreeMap<String, std::sync::Arc<crate::api::MeshGeometry>>,
    cancelled: BTreeSet<String>,
    selection_permitted: bool,
    selected_tab: Option<String>,
    page: usize,
}

fn closed(v: &Value, keys: &[&str]) -> Result<()> {
    let object = v.as_object().ok_or("loader object required")?;
    if object.keys().any(|key| !keys.contains(&key.as_str())) {
        return Err("unknown loader object field".into());
    }
    Ok(())
}
fn text<'a>(v: &'a Value, key: &str, max: usize, nonempty: bool) -> Result<&'a str> {
    let s = v
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("loader {key} string required"))?;
    if s.len() > max || (nonempty && s.is_empty()) {
        return Err(format!("loader {key} text bound"));
    }
    Ok(s)
}
fn identifier<'a>(v: &'a Value, key: &str) -> Result<&'a str> {
    text(v, key, 128, true)
}
fn optional_text(v: &Value, key: &str, max: usize) -> Result<()> {
    if v.get(key).is_some() {
        text(v, key, max, false)?;
    }
    Ok(())
}
fn number(v: &Value, key: &str, min: f64, max: f64) -> Result<f64> {
    let n = v[key]
        .as_f64()
        .ok_or_else(|| format!("loader {key} number required"))?;
    if !n.is_finite() || n < min || n > max {
        return Err(format!("loader {key} number bound"));
    }
    Ok(n)
}
fn integer(v: &Value, key: &str, min: u64, max: u64) -> Result<u64> {
    let n = v[key]
        .as_u64()
        .ok_or_else(|| format!("loader {key} integer required"))?;
    if n < min || n > max {
        return Err(format!("loader {key} integer bound"));
    }
    Ok(n)
}
fn array<'a>(v: &'a Value, key: &str, max: usize) -> Result<&'a [Value]> {
    let a = v[key]
        .as_array()
        .ok_or_else(|| format!("loader {key} array required"))?;
    if a.len() > max {
        return Err(format!("loader {key} count bound"));
    }
    Ok(a)
}
fn unique<'a>(set: &mut BTreeSet<&'a str>, id: &'a str) -> Result<()> {
    if !set.insert(id) {
        return Err(format!("duplicate loader id: {id}"));
    }
    Ok(())
}
fn entries(v: &Value, key: &str) -> Result<()> {
    let a = array(v, key, 16)?;
    if a.is_empty() {
        return Err("loader entries must not be empty".into());
    }
    let mut ids = BTreeSet::new();
    for e in a {
        closed(e, &["id", "label"])?;
        unique(&mut ids, identifier(e, "id")?)?;
        text(e, "label", 256, true)?;
    }
    Ok(())
}
fn formation_radius(v: &Value) -> Result<()> {
    if v.get("formation").is_some()
        && !matches!(
            v["formation"].as_str(),
            Some("line" | "column" | "wedge" | "square")
        )
    {
        return Err("unknown loader formation".into());
    }
    if v.get("radius").is_some() {
        number(v, "radius", 0., 128.)?;
    }
    Ok(())
}
fn validate_definition(v: &Value) -> Result<()> {
    closed(
        v,
        &[
            "id",
            "overlay",
            "title",
            "item_id",
            "block_id",
            "widgets",
            "input_bindings",
        ],
    )?;
    identifier(v, "id")?;
    text(v, "title", 128, true)?;
    if v.get("overlay")
        .is_some_and(|overlay| overlay.as_bool() != Some(false))
    {
        return Err("native Loader overlay presentation unsupported".into());
    }
    // These declarations need additional native activation/input owners. Never silently omit them.
    if v.get("item_id").is_some() || v.get("block_id").is_some() {
        return Err("native Loader item/block views unsupported".into());
    }
    if v.get("input_bindings").is_some() && !array(v, "input_bindings", 16)?.is_empty() {
        return Err("native Loader keyboard bindings unsupported".into());
    }
    let mut ids = BTreeSet::new();
    for w in array(v, "widgets", 32)? {
        let kind = text(w, "type", 128, true)?;
        let keys: &[&str] = match kind {
            "paged_table" => &["type", "id", "columns"],
            "tabs" => &["type", "id", "entries"],
            "input_number" => &["type", "id", "label", "min", "max", "step"],
            "input_text" => &["type", "id", "label", "max_bytes"],
            "select_enum" => &["type", "id", "label", "options"],
            "resource_panel" => &["type", "id", "label", "entries"],
            "action_button" => &["type", "action_id", "label", "enabled", "deny_reason"],
            "world_marker" => &[
                "type",
                "id",
                "label",
                "action_id",
                "preview_id",
                "formation",
                "radius",
            ],
            _ => return Err(format!("unknown Loader widget type: {kind}")),
        };
        closed(w, keys)?;
        let id = identifier(
            w,
            if kind == "action_button" {
                "action_id"
            } else {
                "id"
            },
        )?;
        unique(&mut ids, id)?;
        if !matches!(kind, "paged_table" | "tabs") {
            text(w, "label", 256, true)?;
        }
        match kind {
            "paged_table" => {
                let columns = array(w, "columns", 16)?;
                if columns.is_empty() {
                    return Err("loader table requires columns".into());
                }
                let mut columns_ids = BTreeSet::new();
                for c in columns {
                    closed(c, &["id", "label", "align", "width_bucket"])?;
                    unique(&mut columns_ids, identifier(c, "id")?)?;
                    text(c, "label", 256, true)?;
                    if !matches!(c["align"].as_str(), Some("left" | "center" | "right")) {
                        return Err("loader column alignment".into());
                    }
                    integer(c, "width_bucket", 1, 4)?;
                }
            }
            "tabs" | "resource_panel" => entries(w, "entries")?,
            "select_enum" => entries(w, "options")?,
            "input_text" => {
                integer(w, "max_bytes", 1, 256)?;
            }
            "input_number" => {
                for k in ["min", "max", "step"] {
                    if w.get(k).is_some() {
                        number(w, k, -f64::MAX, f64::MAX)?;
                    }
                }
                if w.get("min").is_some()
                    && w.get("max").is_some()
                    && w["min"].as_f64() > w["max"].as_f64()
                {
                    return Err("loader number min exceeds max".into());
                }
                if w.get("step").is_some() && w["step"].as_f64().unwrap() <= 0. {
                    return Err("loader number step must be positive".into());
                }
            }
            "action_button" => {
                if w.get("enabled").is_some() && !w["enabled"].is_boolean() {
                    return Err("loader action enabled boolean required".into());
                }
                optional_text(w, "deny_reason", 128)?;
            }
            "world_marker" => {
                identifier(w, "action_id")?;
                if w.get("preview_id").is_some() {
                    identifier(w, "preview_id")?;
                }
                formation_radius(w)?;
            }
            _ => {}
        }
    }
    Ok(())
}
fn validate_model(v: &Value) -> Result<()> {
    closed(
        v,
        &[
            "page",
            "page_count",
            "rows",
            "fields",
            "actions",
            "tabs",
            "resource_entries",
            "markers",
            "reason",
        ],
    )?;
    let page = integer(v, "page", 0, i32::MAX as u64)?;
    if page >= integer(v, "page_count", 1, i32::MAX as u64)? {
        return Err("loader page outside page count".into());
    }
    optional_text(v, "reason", 256)?;
    for row in array(v, "rows", 64)? {
        closed(row, &["cells"])?;
        for cell in array(row, "cells", 16)? {
            if cell.as_str().is_none_or(|s| s.len() > 256) {
                return Err("loader table cell bound".into());
            }
        }
    }
    let mut ids = BTreeSet::new();
    for f in array(v, "fields", 16)? {
        closed(f, &["id", "number", "text", "selected"])?;
        unique(&mut ids, identifier(f, "id")?)?;
        if ["number", "text", "selected"]
            .iter()
            .filter(|k| f.get(**k).is_some())
            .count()
            != 1
        {
            return Err("loader field requires exactly one typed value".into());
        }
        if f.get("number").is_some() {
            number(f, "number", -f64::MAX, f64::MAX)?;
        }
        if f.get("text").is_some() {
            text(f, "text", 256, false)?;
        }
        if f.get("selected").is_some() {
            identifier(f, "selected")?;
        }
    }
    let mut actions = BTreeSet::new();
    for a in array(v, "actions", 16)? {
        closed(a, &["action_id", "enabled", "label", "deny_reason"])?;
        unique(&mut actions, identifier(a, "action_id")?)?;
        if !a["enabled"].is_boolean() {
            return Err("loader action enabled boolean required".into());
        }
        optional_text(a, "label", 128)?;
        optional_text(a, "deny_reason", 128)?;
    }
    for (key, keys) in [
        ("tabs", &["id", "label"][..]),
        ("resource_entries", &["id", "have", "need"][..]),
    ] {
        let mut ids = BTreeSet::new();
        for e in array(v, key, 16)? {
            closed(e, keys)?;
            unique(&mut ids, identifier(e, "id")?)?;
            if key == "tabs" {
                text(e, "label", 256, true)?;
            } else {
                number(e, "have", 0., f64::MAX)?;
                number(e, "need", 0., f64::MAX)?;
            }
        }
    }
    let mut markers = BTreeSet::new();
    let mut tokens = BTreeSet::new();
    for m in array(v, "markers", 16)? {
        closed(
            m,
            &[
                "marker_id",
                "selection_token",
                "action_id",
                "formation",
                "radius",
                "position",
            ],
        )?;
        unique(&mut markers, identifier(m, "marker_id")?)?;
        if m.get("selection_token").is_some() {
            unique(&mut tokens, identifier(m, "selection_token")?)?;
            identifier(m, "action_id")?;
        }
        if m.get("action_id").is_some() {
            identifier(m, "action_id")?;
        }
        formation_radius(m)?;
        if let Some(p) = m.get("position") {
            closed(p, &["dimension", "x", "y", "z"])?;
            identifier(p, "dimension")?;
            number(p, "x", -30_000_000., 30_000_000.)?;
            number(p, "y", -20_000_000., 20_000_000.)?;
            number(p, "z", -30_000_000., 30_000_000.)?;
        }
    }
    Ok(())
}
fn field_id(id: &str) -> String {
    format!("loader/field/{id}")
}
fn widgets(d: &Value) -> &[Value] {
    d["widgets"].as_array().unwrap()
}
fn values<'a>(m: &'a Value, key: &str) -> &'a [Value] {
    m[key].as_array().unwrap()
}
fn find<'a>(a: &'a [Value], key: &str, id: &str) -> Option<&'a Value> {
    a.iter().find(|v| v[key].as_str() == Some(id))
}

impl LoaderView {
    fn editors(&self) -> Result<BTreeMap<String, EditBuffer>> {
        let mut editors = BTreeMap::new();
        for f in values(&self.model, "fields") {
            let id = f["id"].as_str().ok_or("loader field id required")?;
            let Some((value, limit)) = editor_value(&self.definition, f)? else {
                continue;
            };
            editors.insert(field_id(id), EditBuffer::new(value, limit, false)?);
        }
        Ok(editors)
    }
    fn action(
        &mut self,
        id: &str,
        token: Option<&str>,
        editors: &BTreeMap<String, EditBuffer>,
    ) -> Result<Value> {
        let a = find(values(&self.model, "actions"), "action_id", id)
            .ok_or("loader action not presented")?;
        if a["enabled"] != true {
            return Err("loader action disabled".into());
        }
        let declared = widgets(&self.definition)
            .iter()
            .any(|w| w["type"] == "action_button" && w["action_id"] == id);
        if let Some(token) = token {
            if !self.selection_permitted {
                return Err("Loader world selection permission not declared".into());
            }
            let m = values(&self.model, "markers")
                .iter()
                .find(|m| m["selection_token"] == token && m["action_id"] == id)
                .filter(|_| !self.cancelled.contains(token))
                .ok_or("stale loader selection token")?;
            if !widgets(&self.definition).iter().any(|w| {
                w["type"] == "world_marker" && w["id"] == m["marker_id"] && w["action_id"] == id
            }) {
                return Err("undeclared loader selection action".into());
            }
            let widget = find(
                widgets(&self.definition),
                "id",
                m["marker_id"].as_str().unwrap(),
            )
            .unwrap();
            if let Some(preview) = widget["preview_id"].as_str() {
                if self
                    .previews
                    .get(preview)
                    .is_none_or(|p| p.blocks.is_empty())
                {
                    return Err("loader selection preview unavailable".into());
                }
            }
        } else if !declared {
            return Err("undeclared loader action".into());
        }
        let mut fields = values(&self.model, "fields").to_vec();
        for f in &mut fields {
            let id = f["id"].as_str().unwrap();
            if let Some(edit) = editors.get(&field_id(id)) {
                if f.get("text").is_some() {
                    f["text"] = json!(edit.text);
                } else if f.get("number").is_some() {
                    // Java preserves the presented number if an edit is not a finite number.
                    if let Ok(n) = edit.text.trim().parse::<f64>() {
                        if n.is_finite() {
                            f["number"] = json!(n);
                        }
                    }
                }
            } else if let Some(index) = self.selections.get(id) {
                if let Some(w) = find(widgets(&self.definition), "id", id) {
                    let options = values(w, "options");
                    f["selected"] = options[index % options.len()]["id"].clone();
                }
            }
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or("loader action sequence exhausted")?;
        let mut message = json!({"protocol":3,"message":"view_action","view_instance_id":self.instance,"view_revision":self.revision,"action_id":id,"action_sequence":sequence,"fields":fields});
        if let Some(token) = token {
            message["selection_token"] = json!(token);
        }
        self.sequence = sequence;
        Ok(message)
    }
    fn cancel(&mut self, token: &str) -> Result<Value> {
        let marker = values(&self.model, "markers")
            .iter()
            .find(|m| m["selection_token"] == token)
            .ok_or("stale loader selection token")?;
        if !self.selection_permitted
            || self.cancelled.contains(token)
            || !widgets(&self.definition).iter().any(|w| {
                w["type"] == "world_marker"
                    && w["id"] == marker["marker_id"]
                    && w["action_id"] == marker["action_id"]
            })
        {
            return Err("stale or unpermitted loader selection cancellation".into());
        }
        self.cancelled.insert(token.to_owned());
        Ok(json!({"protocol":3,"message":"cancel_selection","selection_context_id":token}))
    }
}

impl UiState {
    pub(crate) fn prepare_loader_previews(
        &mut self,
        content: &crate::protocol::content::Content,
    ) -> Result<()> {
        if let Some(view) = &mut self.loader {
            for (id, preview) in &view.previews {
                if !view.preview_meshes.contains_key(id) {
                    view.preview_meshes.insert(
                        id.clone(),
                        std::sync::Arc::new(preview.build_geometry(content)?),
                    );
                }
            }
        }
        Ok(())
    }
    pub fn validate_loader_definition(definition: &Value) -> Result<()> {
        validate_definition(definition)
    }
    pub(crate) fn loader_view_instance(&self) -> Option<&str> {
        (self.screen == Screen::Loader)
            .then_some(())
            .and(self.loader.as_ref())
            .map(|view| view.instance.as_str())
    }
    pub(crate) fn present_loader(
        &mut self,
        definition: &Value,
        message: &Value,
        previews: &PreviewCatalog,
        selection_permitted: bool,
    ) -> Result<()> {
        validate_definition(definition)?;
        validate_references(std::slice::from_ref(definition), previews)?;
        closed(
            message,
            match message["message"].as_str() {
                Some("open_view") => &[
                    "protocol",
                    "message",
                    "view_instance_id",
                    "revision",
                    "view_id",
                    "title",
                    "model",
                ][..],
                Some("present_view") => &[
                    "protocol",
                    "message",
                    "view_instance_id",
                    "revision",
                    "model",
                ][..],
                _ => return Err("loader presentation message required".into()),
            },
        )?;
        if message["protocol"].as_u64() != Some(3) {
            return Err("loader presentation protocol required".into());
        }
        let model = message.get("model").ok_or("loader model required")?;
        let active_previews = widgets(definition)
            .iter()
            .filter_map(|widget| widget["preview_id"].as_str())
            .map(|id| (id.to_owned(), std::sync::Arc::clone(&previews[id])))
            .collect();
        validate_model(model)?;
        let instance = identifier(message, "view_instance_id")?;
        let revision = integer(message, "revision", 0, i64::MAX as u64)?;
        let title = match message["message"].as_str() {
            Some("open_view") => {
                if message["view_id"] != definition["id"] {
                    return Err("loader view definition mismatch".into());
                }
                text(message, "title", 128, true)?.to_owned()
            }
            Some("present_view") => {
                let current = self.loader.as_ref().ok_or("loader view not open")?;
                if self.screen != Screen::Loader {
                    return Err("stale loader view instance".into());
                }
                if current.instance != instance || current.definition != *definition {
                    return Err("stale loader view instance".into());
                }
                current.title.clone()
            }
            _ => unreachable!(),
        };
        if self
            .loader
            .as_ref()
            .is_some_and(|v| v.instance == instance && revision <= v.revision)
        {
            return Err("stale loader view revision".into());
        }
        let mut view = LoaderView {
            definition: definition.clone(),
            model: model.clone(),
            instance: instance.into(),
            title,
            revision,
            sequence: 0,
            selections: BTreeMap::new(),
            selected_tab: None,
            previews: active_previews,
            preview_meshes: BTreeMap::new(),
            cancelled: BTreeSet::new(),
            selection_permitted,
            page: 0,
        };
        let editors = view.editors()?;
        if let Some(current) = &mut self.loader {
            if current.instance == instance
                && current.previews.len() == view.previews.len()
                && view.previews.iter().all(|(id, preview)| {
                    current
                        .previews
                        .get(id)
                        .is_some_and(|old| std::sync::Arc::ptr_eq(old, preview))
                })
            {
                view.preview_meshes = std::mem::take(&mut current.preview_meshes);
            }
        }
        self.fields.retain(|id, _| !id.starts_with("loader/"));
        self.fields.extend(editors);
        self.focus = None;
        self.release_all();
        self.loader = Some(view);
        self.transition(Screen::Loader);
        self.invalidate();
        Ok(())
    }
    pub fn close_loader(&mut self, view_instance_id: &str) -> Result<()> {
        if self
            .loader
            .as_ref()
            .is_none_or(|v| v.instance != view_instance_id)
        {
            return Err("stale loader view instance".into());
        }
        self.loader = None;
        self.fields.retain(|id, _| !id.starts_with("loader/"));
        if self.screen == Screen::Loader {
            self.transition(Screen::Game);
        }
        self.invalidate();
        Ok(())
    }
    pub(crate) fn loader_observation(&self) -> Value {
        if self.screen != Screen::Loader {
            return Value::Null;
        }
        self.loader.as_ref().map_or(Value::Null, |v| json!({"view_instance_id":v.instance,"view_id":v.definition["id"],"view_revision":v.revision,"action_sequence":v.sequence,"title":v.title,"overlay":v.definition["overlay"].as_bool().unwrap_or(false),"content_page":v.page,"model":v.model}))
    }
    pub(crate) fn activate_loader(
        &mut self,
        action: LoaderControl,
        effects: &mut UiEffects,
    ) -> Result<()> {
        if self.screen != Screen::Loader {
            return Err("loader view not open".into());
        }
        let view = self.loader.as_mut().ok_or("loader view not open")?;
        match action {
            LoaderControl::Action(id, token) => {
                effects.commands.push(UiCommand::Loader(view.action(
                    &id,
                    token.as_deref(),
                    &self.fields,
                )?));
            }
            LoaderControl::Cancel(token) => {
                effects
                    .commands
                    .push(UiCommand::Loader(view.cancel(&token)?));
            }
            LoaderControl::Enum(id) => {
                let (options, current) = {
                    let widget = find(widgets(&view.definition), "id", &id)
                        .ok_or("loader enum not presented")?;
                    let field = find(values(&view.model, "fields"), "id", &id)
                        .ok_or("loader enum not presented")?;
                    if widget["type"] != "select_enum" || field.get("selected").is_none() {
                        return Err("loader enum not presented".into());
                    }
                    let options = values(widget, "options").to_vec();
                    let current = options
                        .iter()
                        .position(|o| o["id"] == field["selected"])
                        .unwrap_or(0);
                    (options, current)
                };
                let index = view.selections.entry(id).or_insert(current);
                *index = (*index + 1) % options.len();
            }
            LoaderControl::Tab(tab_id) => {
                let admitted = widgets(&view.definition).iter().any(|w| {
                    w["type"] == "tabs" && find(values(w, "entries"), "id", &tab_id).is_some()
                });
                if !admitted || find(values(&view.model, "tabs"), "id", &tab_id).is_none() {
                    return Err("loader tab not presented".into());
                }
                view.selected_tab = Some(tab_id);
            }
            LoaderControl::Page(page) => {
                view.page = page;
                self.focus = None;
            }
        }
        self.invalidate();
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub(crate) enum LoaderControl {
    Action(String, Option<String>),
    Cancel(String),
    Enum(String),
    Tab(String),
    Page(usize),
}

// Each section is paginated as a unit, so fields/actions never exist offscreen as hidden responders.
struct Control {
    id: String,
    label: String,
    role: &'static str,
    action: NodeAction,
    paint: Paint,
    enabled: bool,
    width: f32,
    height: f32,
    align: &'static str,
}
type Section = Vec<Control>;
impl UiState {
    fn loader_line(&self, id: String, label: String, width: f32) -> Result<Control> {
        let layout = self.text.layout(&json!({"text":label}), width.max(1.))?;
        Ok(Control {
            id,
            label,
            role: "text",
            action: NodeAction::None,
            paint: Paint::Text,
            enabled: true,
            width,
            height: layout.height.max(12.),
            align: "left",
        })
    }
    fn loader_button(
        id: String,
        label: String,
        control: LoaderControl,
        enabled: bool,
        width: f32,
    ) -> Control {
        Control {
            id,
            label,
            role: "button",
            action: NodeAction::Loader(control),
            paint: Paint::Button,
            enabled,
            width,
            height: 20.,
            align: "left",
        }
    }
    pub(crate) fn loader_tree(&mut self) -> Result<()> {
        let (definition, model, title, selected_tab, selections, content_page, previews, cancelled) =
            self.loader
                .as_ref()
                .map(|view| {
                    (
                        view.definition.clone(),
                        view.model.clone(),
                        view.title.clone(),
                        view.selected_tab.clone(),
                        view.selections.clone(),
                        view.page,
                        view.previews.clone(),
                        view.cancelled.clone(),
                    )
                })
                .ok_or("loader screen state missing")?;
        let available_width = (self.width - 16.).max(1.);
        let control_width = available_width.min(200.);
        let mut sections: Vec<Section> = Vec::new();
        if definition
            .get("overlay")
            .is_some_and(|overlay| overlay.as_bool() != Some(false))
        {
            return Err("native Loader overlay presentation unsupported".into());
        }
        self.add(
            "loader/background".into(),
            "presentation",
            String::new(),
            [0., 0., self.width, self.height],
            true,
            None,
            NodeAction::None,
            Paint::Solid([0.; 3], 0.75),
        )?;
        sections.push(vec![self.loader_line(
            "loader/title".into(),
            title,
            available_width,
        )?]);
        if let Some(reason) = model["reason"].as_str() {
            sections.push(vec![self.loader_line(
                "loader/reason".into(),
                format!("reason: {reason}"),
                available_width,
            )?]);
        }
        sections.push(vec![self.loader_line(
            "loader/server-page".into(),
            format!(
                "Page {} of {}",
                model["page"].as_u64().unwrap() + 1,
                model["page_count"]
            ),
            available_width,
        )?]);
        let mut maps: BTreeMap<String, Vec<(Value, String)>> = BTreeMap::new();
        for w in widgets(&definition) {
            let kind = w["type"].as_str().unwrap();
            let id = w[if kind == "action_button" {
                "action_id"
            } else {
                "id"
            }]
            .as_str()
            .unwrap();
            match kind {
                "paged_table" => {
                    let columns = values(w, "columns");
                    let total: f32 = columns
                        .iter()
                        .map(|c| c["width_bucket"].as_u64().unwrap() as f32)
                        .sum();
                    for (row_index, row) in std::iter::once(None)
                        .chain(values(&model, "rows").iter().map(Some))
                        .enumerate()
                    {
                        let mut cells = Vec::new();
                        for (i, c) in columns.iter().enumerate() {
                            let value = row.map_or(c["label"].as_str().unwrap(), |r| {
                                r["cells"][i].as_str().unwrap_or("")
                            });
                            let width = available_width
                                * c["width_bucket"].as_u64().unwrap() as f32
                                / total;
                            let mut cell = self.loader_line(
                                format!("loader/table/{id}/{row_index}/{i}"),
                                value.into(),
                                width - 4.,
                            )?;
                            cell.width = width;
                            cell.align = match c["align"].as_str().unwrap() {
                                "right" => "right",
                                "center" => "center",
                                _ => "left",
                            };
                            cells.push(cell);
                        }
                        sections.push(cells);
                    }
                }
                "tabs" => {
                    for tab in values(&model, "tabs") {
                        let tab_id = tab["id"].as_str().unwrap();
                        if find(values(w, "entries"), "id", tab_id).is_none() {
                            continue;
                        }
                        let label = tab["label"].as_str().unwrap();
                        let label = if selected_tab.as_deref() == Some(tab_id) {
                            format!("[{label}]")
                        } else {
                            label.into()
                        };
                        sections.push(vec![Self::loader_button(
                            format!("loader/tab/{id}/{tab_id}"),
                            label,
                            LoaderControl::Tab(tab_id.into()),
                            true,
                            control_width,
                        )]);
                    }
                }
                "input_number" | "input_text" => {
                    if !self.fields.contains_key(&field_id(id)) {
                        continue;
                    }
                    sections.push(vec![self.loader_line(
                        format!("loader/label/{id}"),
                        w["label"].as_str().unwrap().into(),
                        available_width,
                    )?]);
                    let fid = field_id(id);
                    sections.push(vec![Control {
                        id: fid.clone(),
                        label: w["label"].as_str().unwrap().into(),
                        role: "textbox",
                        action: NodeAction::Focus(fid.clone()),
                        paint: Paint::Field(fid),
                        enabled: true,
                        width: available_width.min(160.),
                        height: 20.,
                        align: "left",
                    }]);
                }
                "select_enum" => {
                    let Some(f) = find(values(&model, "fields"), "id", id) else {
                        continue;
                    };
                    let options = values(w, "options");
                    let index = selections.get(id).copied().unwrap_or_else(|| {
                        options
                            .iter()
                            .position(|o| o["id"] == f["selected"])
                            .unwrap_or(0)
                    });
                    let label = format!(
                        "{}: {}",
                        w["label"].as_str().unwrap(),
                        options[index % options.len()]["label"].as_str().unwrap()
                    );
                    sections.push(vec![Self::loader_button(
                        format!("loader/enum/{id}"),
                        label,
                        LoaderControl::Enum(id.into()),
                        true,
                        control_width,
                    )]);
                }
                "resource_panel" => {
                    for e in values(w, "entries") {
                        let value = find(
                            values(&model, "resource_entries"),
                            "id",
                            e["id"].as_str().unwrap(),
                        )
                        .map_or_else(|| "-".into(), |v| format!("{}/{}", v["have"], v["need"]));
                        sections.push(vec![self.loader_line(
                            format!("loader/resource/{id}/{}", e["id"].as_str().unwrap()),
                            format!(
                                "{} {}: {value}",
                                w["label"].as_str().unwrap(),
                                e["label"].as_str().unwrap()
                            ),
                            available_width,
                        )?]);
                    }
                }
                "action_button" => {
                    if let Some(a) = find(values(&model, "actions"), "action_id", id) {
                        let enabled = a["enabled"].as_bool().unwrap();
                        sections.push(vec![Self::loader_button(
                            format!("loader/action/{id}"),
                            a["label"]
                                .as_str()
                                .unwrap_or(w["label"].as_str().unwrap())
                                .into(),
                            LoaderControl::Action(id.into(), None),
                            enabled,
                            control_width,
                        )]);
                        if !enabled {
                            if let Some(reason) =
                                a["deny_reason"].as_str().or(w["deny_reason"].as_str())
                            {
                                sections.push(vec![self.loader_line(
                                    format!("loader/deny/{id}"),
                                    format!("denied: {reason}"),
                                    available_width,
                                )?]);
                            }
                        }
                    }
                }
                "world_marker" => {
                    if let Some(m) = find(values(&model, "markers"), "marker_id", id) {
                        if m.get("action_id").is_some() && m["action_id"] != w["action_id"] {
                            continue;
                        }
                        sections.push(vec![self.loader_line(
                            format!("loader/marker/{id}/label"),
                            w["label"].as_str().unwrap().to_owned(),
                            available_width,
                        )?]);
                        if let Some(formation) = m["formation"].as_str().or(w["formation"].as_str())
                        {
                            sections.push(vec![self.loader_line(
                                format!("loader/marker/{id}/formation"),
                                format!("Formation: {formation}"),
                                available_width,
                            )?]);
                        }
                        if let Some(radius) = m["radius"].as_f64().or(w["radius"].as_f64()) {
                            sections.push(vec![self.loader_line(
                                format!("loader/marker/{id}/radius"),
                                format!("Radius: {radius}"),
                                available_width,
                            )?]);
                        }
                        if let Some(preview_id) = w["preview_id"].as_str() {
                            let preview = &previews[preview_id];
                            let size = preview.rotated_size();
                            sections.push(vec![self.loader_line(
                                format!("loader/preview/{id}/description"),
                                format!(
                                    "Blueprint {} @ {} degrees, {}x{}x{} ({})",
                                    preview.blueprint_id,
                                    preview.rotation as u16 * 90,
                                    size[0],
                                    size[1],
                                    size[2],
                                    &preview.content_hash[..12]
                                ),
                                available_width,
                            )?]);
                            if !preview.blocks.is_empty() {
                                sections.push(vec![Control {
                                    id: format!("loader/preview/{id}/geometry"),
                                    label: String::new(),
                                    role: "presentation",
                                    action: NodeAction::None,
                                    paint: Paint::LoaderPreview(std::sync::Arc::clone(preview)),
                                    enabled: true,
                                    width: available_width.min(400.),
                                    height: 180.,
                                    align: "left",
                                }]);
                            }
                        }
                        if let Some(p) = m.get("position") {
                            maps.entry(p["dimension"].as_str().unwrap().to_owned())
                                .or_default()
                                .push((p.clone(), w["label"].as_str().unwrap().to_owned()));
                        }
                        if m["action_id"] == w["action_id"]
                            && self.loader.as_ref().unwrap().selection_permitted
                        {
                            let preview_available = w["preview_id"]
                                .as_str()
                                .is_none_or(|id| !previews[id].blocks.is_empty());
                            if let Some(token) = m["selection_token"].as_str() {
                                if cancelled.contains(token) {
                                    continue;
                                }
                                let aid = w["action_id"].as_str().unwrap();
                                if let Some(a) = find(values(&model, "actions"), "action_id", aid) {
                                    sections.push(vec![Self::loader_button(
                                        format!("loader/select/{id}"),
                                        format!("Select: {aid}"),
                                        LoaderControl::Action(aid.into(), Some(token.into())),
                                        a["enabled"].as_bool().unwrap() && preview_available,
                                        control_width,
                                    )]);
                                }
                                sections.push(vec![Self::loader_button(
                                    format!("loader/cancel/{id}"),
                                    "Cancel selection".into(),
                                    LoaderControl::Cancel(token.into()),
                                    true,
                                    control_width,
                                )]);
                            }
                        }
                    }
                }
                _ => unreachable!(),
            }
        }
        for (dimension, points) in maps {
            let locations = points
                .iter()
                .map(|(p, _)| [p["x"].as_f64().unwrap(), p["z"].as_f64().unwrap()])
                .collect();
            sections.push(vec![self.loader_line(
                format!("loader/map/{dimension}/title"),
                dimension.clone(),
                available_width,
            )?]);
            sections.push(vec![Control {
                id: format!("loader/map/{dimension}"),
                label: String::new(),
                role: "presentation",
                action: NodeAction::None,
                paint: Paint::LoaderMap(locations),
                enabled: true,
                width: available_width.min(400.),
                height: 160.,
                align: "left",
            }]);
            for (index, (position, label)) in points.into_iter().enumerate() {
                sections.push(vec![self.loader_line(
                    format!("loader/map/{dimension}/{index}"),
                    format!(
                        "{}: {} ({}, {}, {})",
                        index + 1,
                        label,
                        position["x"],
                        position["y"],
                        position["z"]
                    ),
                    available_width,
                )?]);
            }
        }
        let available_height = (self.height - 36.).max(1.);
        let mut pages: Vec<Vec<Section>> = vec![Vec::new()];
        let mut used_height = 0.;
        for section in sections {
            let height = section
                .iter()
                .map(|c| c.height)
                .fold(0., f32::max)
                .min(available_height)
                + 4.;
            if used_height + height > available_height && !pages.last().unwrap().is_empty() {
                pages.push(Vec::new());
                used_height = 0.;
            }
            pages.last_mut().unwrap().push(section);
            used_height += height;
        }
        let page_count = pages.len();
        let page = content_page.min(page_count.saturating_sub(1));
        if let Some(view) = self.loader.as_mut() {
            view.page = page;
        }
        let mut y = 8.;
        for section in std::mem::take(&mut pages[page]) {
            let width: f32 = section.iter().map(|c| c.width).sum();
            let height = section.iter().map(|c| c.height).fold(0., f32::max);
            let mut x = ((self.width - width) / 2.).max(8.);
            for c in section {
                let mut rect = [x, y, c.width, c.height.min(available_height)];
                if c.role == "text" && c.align != "left" {
                    let layout = self
                        .text
                        .layout(&json!({"text":c.label}), (c.width - 4.).max(1.))?;
                    let offset = (c.width - layout.width).max(0.)
                        * if c.align == "right" { 1. } else { 0.5 };
                    rect[0] += offset;
                    rect[2] -= offset;
                }
                self.add(
                    c.id,
                    c.role,
                    c.label,
                    rect,
                    c.enabled,
                    if c.enabled {
                        None
                    } else {
                        Some("loader_action_disabled")
                    },
                    c.action,
                    c.paint,
                )?;
                self.nodes.last_mut().unwrap().owner = "solaris_loader";
                x += c.width;
            }
            y += height + 4.;
        }
        if page_count > 1 {
            let w = available_width.min(110.);
            self.button(
                "loader/page/previous",
                "Previous page",
                [8., self.height - 28., w, 20.],
                NodeAction::Loader(LoaderControl::Page(page.saturating_sub(1))),
                page > 0,
                Some("first_page"),
            )?;
            self.button(
                "loader/page/next",
                "Next page",
                [(self.width - 8. - w).max(8.), self.height - 28., w, 20.],
                NodeAction::Loader(LoaderControl::Page((page + 1).min(page_count - 1))),
                page + 1 < page_count,
                Some("last_page"),
            )?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn definition() -> Value {
        json!({"id":"example:orders","title":"Orders","widgets":[{"type":"input_text","id":"name","label":"Name","max_bytes":8},{"type":"input_number","id":"count","label":"Count","min":0,"max":9},{"type":"select_enum","id":"kind","label":"Kind","options":[{"id":"a","label":"A"},{"id":"b","label":"B"}]},{"type":"action_button","action_id":"example:submit","label":"Submit"},{"type":"world_marker","id":"target","label":"Target","action_id":"example:select"}]})
    }
    fn model() -> Value {
        json!({"page":0,"page_count":1,"rows":[],"fields":[{"id":"name","text":"old"},{"id":"count","number":2},{"id":"kind","selected":"a"}],"actions":[{"action_id":"example:submit","enabled":true},{"action_id":"example:select","enabled":true}],"tabs":[],"resource_entries":[],"markers":[{"marker_id":"target","action_id":"example:select","selection_token":"token","position":{"dimension":"minecraft:overworld","x":1,"y":64,"z":2}}]})
    }
    fn view() -> LoaderView {
        LoaderView {
            definition: definition(),
            model: model(),
            instance: "instance".into(),
            title: "Orders".into(),
            revision: 4,
            sequence: 0,
            selections: BTreeMap::new(),
            previews: PreviewCatalog::new(),
            preview_meshes: BTreeMap::new(),
            cancelled: BTreeSet::new(),
            selection_permitted: true,
            selected_tab: None,
            page: 0,
        }
    }
    #[test]
    fn loader_action_uses_edited_fields_current_revision_and_token() {
        let mut v = view();
        let mut edits = v.editors().unwrap();
        let edit = edits.get_mut(&field_id("name")).unwrap();
        edit.select(0, edit.text.len()).unwrap();
        edit.insert("new").unwrap();
        v.selections.insert("kind".into(), 1);
        let action = v.action("example:select", Some("token"), &edits).unwrap();
        assert_eq!(action["view_revision"], 4);
        assert_eq!(action["action_id"], "example:select");
        assert_eq!(action["action_sequence"], 1);
        assert_eq!(action["selection_token"], "token");
        let fields = action["fields"].as_array().unwrap();
        assert_eq!(find(fields, "id", "name").unwrap()["text"], "new");
        assert_eq!(
            find(fields, "id", "count").unwrap()["number"].as_f64(),
            Some(2.)
        );
        assert_eq!(find(fields, "id", "kind").unwrap()["selected"], "b");
        assert_eq!(
            v.action("example:submit", None, &edits).unwrap()["action_sequence"],
            2
        );
        assert!(v.action("example:select", Some("expired"), &edits).is_err());
    }
    #[test]
    fn loader_nodes_byte_bounds_match_focused_input_checks() {
        assert!(text(&json!({"field":"ééééé"}), "field", 8, false).is_err());
        assert_eq!(
            text(&json!({"field":"éééé"}), "field", 8, false).unwrap(),
            "éééé"
        );
        assert!(validate_definition(&json!({"id":"example:all","title":"All","widgets":[{"type":"paged_table","id":"t","columns":[]}]})).is_err());
        assert!(
            validate_definition(
                &json!({"id":"example:all","title":"All","overlay":true,"widgets":[]})
            )
            .is_err()
        );
    }
    #[test]
    fn selection_permission_disabled_action_and_cancel_fence_do_not_advance_sequence() {
        let mut v = view();
        let edits = v.editors().unwrap();
        v.selection_permitted = false;
        assert!(v.action("example:select", Some("token"), &edits).is_err());
        assert!(v.cancel("token").is_err());
        assert_eq!(v.sequence, 0);
        v.selection_permitted = true;
        v.model["actions"][1]["enabled"] = json!(false);
        assert!(v.action("example:select", Some("token"), &edits).is_err());
        assert_eq!(v.sequence, 0);
        v.model["actions"][1]["enabled"] = json!(true);
        v.action("example:select", Some("token"), &edits).unwrap();
        v.cancel("token").unwrap();
        assert!(v.action("example:select", Some("token"), &edits).is_err());
        assert!(v.cancel("token").is_err());
        assert_eq!(v.sequence, 1);
        v.model["markers"][0]["selection_token"] = json!("replacement");
        assert!(v.action("example:select", Some("token"), &edits).is_err());
        assert_eq!(
            v.action("example:select", Some("replacement"), &edits)
                .unwrap()["action_sequence"],
            2
        );
    }
    #[test]
    fn selection_requires_exact_declared_marker_and_available_preview() {
        let mut v = view();
        let edits = v.editors().unwrap();
        v.model["markers"][0]["marker_id"] = json!("undeclared");
        assert!(v.action("example:select", Some("token"), &edits).is_err());
        assert!(v.cancel("token").is_err());
        v.model["markers"][0]["marker_id"] = json!("target");
        v.definition["widgets"][4]["preview_id"] = json!("example:gate");
        assert!(v.action("example:select", Some("token"), &edits).is_err());
        let preview = json!({"id":"example:gate","blueprint_id":"example:gate","content_hash":"a".repeat(64),
            "rotation":0,"size_x":1,"size_y":1,"size_z":1,"blocks":[]});
        v.previews = crate::loader::previews::parse("example", &[preview.clone()]).unwrap();
        assert!(v.action("example:select", Some("token"), &edits).is_err());
        let mut preview = preview;
        preview["blocks"] = json!([{"x":0,"y":0,"z":0,"block_id":"minecraft:stone"}]);
        v.previews = crate::loader::previews::parse("example", &[preview]).unwrap();
        assert_eq!(
            v.action("example:select", Some("token"), &edits).unwrap()["selection_token"],
            "token"
        );
        assert_eq!(v.sequence, 1);
    }
    #[test]
    fn marker_model_rejects_ambiguous_tokens_and_unbounded_received_locations() {
        let mut m = model();
        let mut duplicate = m["markers"][0].clone();
        duplicate["marker_id"] = json!("different-marker");
        m["markers"].as_array_mut().unwrap().push(duplicate);
        assert!(validate_model(&m).is_err());
        m = model();
        m["markers"][0].as_object_mut().unwrap().remove("action_id");
        assert!(validate_model(&m).is_err());
        m = model();
        m["markers"][0]["radius"] = json!(129);
        assert!(validate_model(&m).is_err());
        m = model();
        m["markers"][0]["formation"] = json!("unknown");
        assert!(validate_model(&m).is_err());
        m = model();
        m["markers"][0]["position"]["x"] = json!(30_000_001);
        assert!(validate_model(&m).is_err());
        m["markers"][0]["position"]["x"] = json!(-30_000_000);
        m["markers"][0]["radius"] = json!(128);
        m["markers"][0]["formation"] = json!("wedge");
        validate_model(&m).unwrap();
    }
}
fn editor_value(definition: &Value, field: &Value) -> Result<Option<(String, usize)>> {
    let id = field["id"].as_str().ok_or("loader field id required")?;
    let Some(w) = find(widgets(definition), "id", id) else {
        return Ok(None);
    };
    Ok(Some(match w["type"].as_str().unwrap() {
        "input_text" if field.get("text").is_some() => (
            field["text"].as_str().unwrap().to_owned(),
            w["max_bytes"].as_u64().unwrap() as usize,
        ),
        "input_number" if field.get("number").is_some() => {
            (field["number"].as_f64().unwrap().to_string(), 32)
        }
        "select_enum" if field.get("selected").is_some() => return Ok(None),
        "input_text" | "input_number" | "select_enum" => {
            return Err("loader declared field type mismatch".into());
        }
        _ => return Ok(None),
    }))
}
