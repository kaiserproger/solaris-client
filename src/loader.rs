//! Native Loader wire-3 admission. Server bytes never grant local permissions.
pub(crate) mod items;
pub(crate) mod previews;
pub(crate) mod sounds;
use crate::{
    protocol::{Reader, Result},
    ui::UiState,
};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
    sync::Arc,
};

const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_MESSAGE: usize = 64 * 1024;
const PERMISSIONS: [&str; 7] = [
    "present_views",
    "send_view_actions",
    "load_assets",
    "register_items",
    "send_world_selection",
    "present_world_previews",
    "play_sounds",
];
const CONTENT: [&str; 7] = [
    "views",
    "view_actions",
    "assets",
    "items",
    "world_selection",
    "world_previews",
    "sounds",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    protocol: u16,
    bundles: Vec<Bundle>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    owner: String,
    id: String,
    version: String,
    artifact: String,
    sha256: String,
    size_bytes: usize,
    loaders: Vec<String>,
    content: Vec<String>,
    permissions: Vec<String>,
    cache_key: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Index {
    schema: u16,
    #[serde(default)]
    screens: Vec<Value>,
    #[serde(default)]
    assets: Vec<Asset>,
    #[serde(default)]
    world_previews: Vec<Value>,
    #[serde(default)]
    blocks: Vec<Value>,
    #[serde(default)]
    items: Vec<Value>,
    #[serde(default)]
    sounds: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asset {
    id: String,
    path: String,
    sha256: String,
    size_bytes: usize,
}
struct Transfer {
    manifest: Manifest,
    next: usize,
    bytes: Vec<u8>,
    digest: Sha256,
}
struct Screen {
    definition: Value,
    actions: bool,
    world_selection: bool,
}
struct LiveView {
    id: String,
    key: String,
    revision: u64,
    last_action: Option<u64>,
    selections: BTreeMap<String, String>,
}
#[derive(Default)]
pub(crate) struct Loader {
    transfer: Option<Transfer>,
    screens: BTreeMap<String, Screen>,
    assets: BTreeMap<String, Arc<[u8]>>,
    verified_paths: BTreeMap<String, Arc<[u8]>>,
    previews: BTreeMap<String, Arc<previews::WorldPreview>>,
    sounds: sounds::Registry,
    items: Vec<Value>,
    admitted_content: BTreeSet<String>,
    content_ids: BTreeSet<String>,
    accepted: BTreeSet<String>,
    cache_keys: Vec<String>,
    acknowledged: bool,
    view: Option<LiveView>,
}
pub(crate) struct Outgoing {
    pub channel: &'static str,
    pub payload: Vec<u8>,
}

impl Loader {
    /// This opt-in is supplied by the local launcher, never a received manifest.
    pub fn local_permissions() -> Result<BTreeSet<String>> {
        let value = std::env::var("SOLARIS_CLIENT_LOADER_PERMISSIONS").unwrap_or_default();
        let mut grants = BTreeSet::new();
        for permission in value.split(',').filter(|s| !s.is_empty()) {
            if !PERMISSIONS.contains(&permission) || !grants.insert(permission.to_owned()) {
                return Err("unsupported or duplicate local Loader permission".into());
            }
        }
        Ok(grants)
    }
    pub fn begin(&mut self, payload: &[u8], grants: &BTreeSet<String>) -> Result<Outgoing> {
        if self.transfer.is_some() || self.acknowledged {
            return Err("duplicate Loader manifest".into());
        }
        if payload.len() > 32767 {
            return Err("Loader manifest byte limit".into());
        }
        let mut manifest: Manifest = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
        if manifest.protocol != 3 || manifest.bundles.is_empty() || manifest.bundles.len() > 64 {
            return Err("invalid Loader manifest protocol/bundle count".into());
        }
        manifest.bundles.retain(|bundle| {
            bundle.loaders.iter().any(|platform| platform == "native")
                && bundle.permissions.iter().all(|permission| {
                    PERMISSIONS.contains(&permission.as_str()) && grants.contains(permission)
                })
        });
        validate_manifest(&manifest, grants)?;
        if manifest.bundles.is_empty() {
            return self.acknowledge();
        }
        let size = manifest.bundles[0].size_bytes;
        self.transfer = Some(Transfer {
            manifest,
            next: 0,
            bytes: Vec::with_capacity(size),
            digest: Sha256::new(),
        });
        self.request_artifact()
    }
    fn request_artifact(&self) -> Result<Outgoing> {
        let transfer = self.transfer.as_ref().ok_or("no Loader transfer")?;
        outgoing(
            "solaris:loader/request",
            json!({"protocol":3,"cache_key":transfer.manifest.bundles[transfer.next].cache_key}),
        )
    }
    fn acknowledge(&mut self) -> Result<Outgoing> {
        self.acknowledged = true;
        outgoing(
            "solaris:loader/ack",
            json!({"protocol":3,"platform":"native","loader_version":env!("CARGO_PKG_VERSION"),"accepted_permissions":self.accepted,"cached_bundles":self.cache_keys,"carrier_block_state_ids":{}}),
        )
    }
    pub fn artifact(&mut self, payload: &[u8]) -> Result<Option<Outgoing>> {
        let transfer = self.transfer.as_mut().ok_or("unexpected Loader artifact")?;
        let bundle = &transfer.manifest.bundles[transfer.next];
        let mut r = Reader::new(payload);
        if r.i16()? != 3 {
            return Err("unsupported Loader artifact protocol".into());
        }
        let key_len = r.i16()?;
        if !(1..=256).contains(&key_len) {
            return Err("Loader artifact cache key length".into());
        }
        let key = std::str::from_utf8(r.take(key_len as usize)?).map_err(|e| e.to_string())?;
        if key != bundle.cache_key {
            return Err("Loader artifact cache identity mismatch".into());
        }
        let offset = r.i64()?;
        let last = r.bool()?;
        let bytes = r.take(r.bytes.len() - r.offset)?;
        if bytes.is_empty()
            || bytes.len() > 30 * 1024
            || offset < 0
            || offset as usize != transfer.bytes.len()
        {
            return Err("Loader artifact chunk offset/size mismatch".into());
        }
        let length = transfer
            .bytes
            .len()
            .checked_add(bytes.len())
            .ok_or("Loader artifact size overflow")?;
        if length > bundle.size_bytes {
            return Err("Loader artifact exceeds declared size".into());
        }
        transfer.bytes.extend_from_slice(bytes);
        transfer.digest.update(bytes);
        if !last {
            if length == bundle.size_bytes {
                return Err("Loader artifact missing final flag".into());
            }
            return Ok(None);
        }
        if length != bundle.size_bytes {
            return Err("Loader artifact final size mismatch".into());
        }
        let digest = std::mem::replace(&mut transfer.digest, Sha256::new()).finalize();
        if format!("{digest:x}") != bundle.sha256 {
            return Err("Loader artifact SHA-256 mismatch".into());
        }
        let activated = activate(
            bundle,
            &transfer.bytes,
            &self.screens,
            &self.assets,
            &self.verified_paths,
            &self.content_ids,
        )?;
        if self.previews.len() + activated.previews.len() > 64
            || self.items.len() + activated.items.len() > 128
        {
            return Err("Loader activated preview/item registry limit".into());
        }
        self.sounds
            .activate(&bundle.owner, &activated.sounds, &activated.paths)?;
        self.screens.extend(activated.screens);
        self.assets.extend(activated.assets);
        self.verified_paths.extend(activated.paths);
        self.previews.extend(activated.previews);
        self.items.extend(activated.items);
        self.admitted_content.extend(bundle.content.iter().cloned());
        self.content_ids.extend(activated.ids);
        self.accepted.extend(bundle.permissions.iter().cloned());
        self.cache_keys.push(bundle.cache_key.clone());
        transfer.next += 1;
        if transfer.next < transfer.manifest.bundles.len() {
            transfer.bytes.clear();
            transfer
                .bytes
                .reserve(transfer.manifest.bundles[transfer.next].size_bytes);
            return self.request_artifact().map(Some);
        }
        self.transfer = None;
        // This is activation acknowledgement, not an acknowledgement of receiving a manifest.
        self.acknowledge().map(Some)
    }
    pub fn sound(&mut self, payload: &[u8], active: bool, tick: u64) -> Result<sounds::Resolved> {
        self.sounds
            .resolve(payload, active && self.acknowledged, tick)
    }
    pub fn verified_paths(&self) -> &BTreeMap<String, Arc<[u8]>> {
        &self.verified_paths
    }
    pub fn item_entries(&self) -> &[Value] {
        &self.items
    }
    pub fn validate_previews(
        &self,
        content: &crate::protocol::content::Content,
        presentation: bool,
    ) -> Result<()> {
        if !presentation && !self.previews.is_empty() {
            return Err(
                "native Loader world_previews requires --windowed for block-model preflight".into(),
            );
        }
        for preview in self.previews.values() {
            preview.validate_content(content)?;
        }
        Ok(())
    }
    pub fn request_view(&self, key: &str, live: bool) -> Result<Outgoing> {
        resource_id(key)?;
        if !live || !self.acknowledged {
            return Err("loader_not_admitted: acknowledged live Play connection required".into());
        }
        if !self.screens.contains_key(key) {
            return Err("undeclared_loader_view".into());
        }
        outgoing(
            "solaris:loader/view_action",
            json!({"protocol":3,"message":"view_request","request_kind":key}),
        )
    }
    pub fn view(&mut self, payload: &[u8], ui: &mut UiState, live: bool) -> Result<String> {
        if !live || !self.acknowledged {
            return Err("Loader view before live admission".into());
        }
        let message = document(payload)?;
        let id = identifier(&message, "view_instance_id")?.to_owned();
        match message["message"].as_str() {
            Some("open_view") => {
                closed(
                    &message,
                    &[
                        "protocol",
                        "message",
                        "view_instance_id",
                        "revision",
                        "view_id",
                        "title",
                        "model",
                    ],
                )?;
                let key = identifier(&message, "view_id")?.to_owned();
                resource_id(&key)?;
                let screen = self.screens.get(&key).ok_or("undeclared Loader view")?;
                let revision = revision(&message, "revision")?;
                let title = message["title"]
                    .as_str()
                    .ok_or("Loader view title missing")?;
                if title.is_empty() || title.len() > 128 {
                    return Err("Loader view title limit".into());
                }
                if self.view.as_ref().is_some_and(|view| view.id == id) {
                    return Err("reused Loader view instance".into());
                }
                ui.present_loader(
                    &screen.definition,
                    &message,
                    &self.previews,
                    screen.actions && screen.world_selection,
                )?;
                self.view = Some(LiveView {
                    id,
                    key,
                    revision,
                    last_action: None,
                    selections: selection_actions(&message["model"]),
                });
            }
            Some("present_view") => {
                closed(
                    &message,
                    &[
                        "protocol",
                        "message",
                        "view_instance_id",
                        "revision",
                        "model",
                    ],
                )?;
                let revision = revision(&message, "revision")?;
                let view = self.view.as_mut().ok_or("stale Loader presentation")?;
                if view.id != id || revision <= view.revision {
                    return Err("stale Loader view revision".into());
                }
                ui.present_loader(
                    &self.screens[&view.key].definition,
                    &message,
                    &self.previews,
                    self.screens[&view.key].actions && self.screens[&view.key].world_selection,
                )?;
                view.revision = revision;
                view.last_action = None;
                view.selections = selection_actions(&message["model"]);
            }
            Some("close_view") => {
                closed(&message, &["protocol", "message", "view_instance_id"])?;
                if self.view.as_ref().is_none_or(|view| view.id != id) {
                    return Err("stale Loader close".into());
                }
                ui.close_loader(&id)?;
                self.view = None;
            }
            _ => return Err("unsupported Loader view message".into()),
        }
        Ok(message["message"].as_str().unwrap().to_owned())
    }
    pub fn action(&mut self, message: Value, live: bool) -> Result<Outgoing> {
        if !live || !self.acknowledged {
            return Err("loader_not_admitted".into());
        }
        if message["message"] == "cancel_selection" {
            closed(&message, &["protocol", "message", "selection_context_id"])?;
            if message["protocol"] != 3 {
                return Err("unsupported Loader action protocol".into());
            }
            let token = identifier(&message, "selection_context_id")?;
            let view = self
                .view
                .as_mut()
                .ok_or("stale Loader selection cancellation")?;
            if !self.screens[&view.key].actions
                || !self.screens[&view.key].world_selection
                || !view.selections.contains_key(token)
            {
                return Err("stale or unpermitted Loader selection cancellation".into());
            }
            let token = token.to_owned();
            let out = outgoing("solaris:loader/view_action", message)?;
            view.selections.remove(&token);
            return Ok(out);
        }
        closed(
            &message,
            &[
                "protocol",
                "message",
                "view_instance_id",
                "view_revision",
                "action_id",
                "action_sequence",
                "fields",
                "selection_token",
            ],
        )?;
        if message["protocol"] != 3 || message["message"] != "view_action" {
            return Err("unsupported Loader action envelope".into());
        }
        let view = self.view.as_mut().ok_or("stale Loader action")?;
        if identifier(&message, "view_instance_id")? != view.id
            || revision(&message, "view_revision")? != view.revision
        {
            return Err("stale Loader action context".into());
        }
        if !self.screens[&view.key].actions {
            return Err("Loader view action permission not declared".into());
        }
        identifier(&message, "action_id")?;
        let sequence = revision(&message, "action_sequence")?;
        if view.last_action.is_some_and(|last| sequence <= last) {
            return Err("stale Loader action sequence".into());
        }
        let fields = message["fields"]
            .as_array()
            .ok_or("Loader action fields missing")?;
        if fields.len() > 16 {
            return Err("Loader action field limit".into());
        }
        let mut ids = BTreeSet::new();
        for field in fields {
            closed(field, &["id", "number", "text", "selected"])?;
            if !ids.insert(identifier(field, "id")?) {
                return Err("duplicate Loader action field".into());
            }
            let count = ["number", "text", "selected"]
                .iter()
                .filter(|key| field.get(**key).is_some())
                .count();
            if count != 1 {
                return Err("Loader action field must have exactly one typed value".into());
            }
            if let Some(number) = field.get("number") {
                if number.as_f64().is_none_or(|n| !n.is_finite()) {
                    return Err("invalid Loader number".into());
                }
            }
            if let Some(text) = field.get("text") {
                if text.as_str().is_none_or(|s| s.len() > 256) {
                    return Err("Loader text limit".into());
                }
            }
            if field.get("selected").is_some() {
                identifier(field, "selected")?;
            }
        }
        if message.get("selection_token").is_some() {
            if !self.screens[&view.key].world_selection {
                return Err("Loader world selection permission not declared".into());
            }
            let token = identifier(&message, "selection_token")?;
            if view.selections.get(token).map(String::as_str) != message["action_id"].as_str() {
                return Err("stale Loader selection token".into());
            }
        }
        let out = outgoing("solaris:loader/view_action", message)?;
        view.last_action = Some(sequence);
        Ok(out)
    }
    pub fn clear_view(&mut self, ui: Option<&mut UiState>) -> Result<()> {
        if let Some(view) = self.view.take() {
            if let Some(ui) = ui {
                if ui.screen() == crate::ui::Screen::Loader {
                    ui.close_loader(&view.id)?;
                }
            }
        }
        Ok(())
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn observation(&self) -> Value {
        json!({"protocol":3,"platform":"native","loader_version":env!("CARGO_PKG_VERSION"),"acknowledged":self.acknowledged,
            "transfer_pending":self.transfer.is_some(),"declared_views":if self.acknowledged {self.screens.keys().collect::<Vec<_>>()} else {Vec::new()},
            "activated_assets":if self.acknowledged {self.assets.keys().collect::<Vec<_>>()} else {Vec::new()},
            "registered_items":if self.acknowledged {self.items.iter().map(|v| &v["id"]).collect::<Vec<_>>()} else {Vec::new()},
            "world_previews":if self.acknowledged {self.previews.keys().collect::<Vec<_>>()} else {Vec::new()},
            "registered_sounds":self.sounds.ids().filter(|_| self.acknowledged).collect::<Vec<_>>(),
            "admitted_content":self.admitted_content.iter().filter(|_| self.acknowledged).collect::<Vec<_>>(),
            "accepted_permissions":self.accepted.iter().filter(|_| self.acknowledged).collect::<Vec<_>>(),
            "cached_bundles":if self.acknowledged {self.cache_keys.as_slice()} else {&[]},
            "view":self.view.as_ref().map(|v| json!({"view_instance_id":v.id,"view_id":v.key,"revision":v.revision})),
            "supported_content":CONTENT,"origin":"hash_verified_received_bundle",
            "gated_content":{"blocks":"dynamic block registry/carrier-state mapping unsupported","overlay":"native Loader overlays unsupported","input_bindings":"native Loader keyboard bindings unsupported"}})
    }
}
fn selection_actions(model: &Value) -> BTreeMap<String, String> {
    model["markers"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|marker| {
            Some((
                marker["selection_token"].as_str()?.to_owned(),
                marker["action_id"].as_str()?.to_owned(),
            ))
        })
        .collect()
}
fn outgoing(channel: &'static str, message: Value) -> Result<Outgoing> {
    let payload = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
    if payload.len() > MAX_MESSAGE {
        return Err("Loader outgoing message byte limit".into());
    }
    Ok(Outgoing { channel, payload })
}
fn document(payload: &[u8]) -> Result<Value> {
    if payload.len() > MAX_MESSAGE {
        return Err("Loader view byte limit".into());
    }
    let value: Value = serde_json::from_slice(payload).map_err(|e| e.to_string())?;
    if value["protocol"] != 3 {
        return Err("unsupported Loader protocol".into());
    }
    Ok(value)
}
fn closed(value: &Value, fields: &[&str]) -> Result<()> {
    let object = value.as_object().ok_or("Loader object required")?;
    if object.keys().any(|key| !fields.contains(&key.as_str())) {
        return Err("unknown Loader field".into());
    }
    Ok(())
}
fn revision(value: &Value, key: &str) -> Result<u64> {
    value[key]
        .as_u64()
        .filter(|n| *n <= i64::MAX as u64)
        .ok_or_else(|| format!("invalid Loader {key}"))
}
fn identifier<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    let text = value[key]
        .as_str()
        .ok_or_else(|| format!("Loader {key} missing"))?;
    if text.is_empty() || text.len() > 128 {
        return Err(format!("invalid Loader {key}"));
    }
    Ok(text)
}
fn resource_id(text: &str) -> Result<()> {
    let (namespace, path) = text
        .split_once(':')
        .ok_or("Loader key must be namespaced")?;
    if namespace.is_empty()
        || path.is_empty()
        || text.len() > 128
        || !namespace
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-.".contains(&b))
        || !path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-./".contains(&b))
    {
        return Err("invalid Loader resource key".into());
    }
    Ok(())
}
fn literal(text: &str, max: usize) -> Result<()> {
    if text.is_empty()
        || text.len() > max
        || matches!(text, "." | "..")
        || !text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err("invalid Loader bundle identity".into());
    }
    Ok(())
}
fn canonical_path(text: &str, max: usize) -> Result<()> {
    if text.is_empty()
        || text.len() > max
        || text.split('/').any(|part| literal(part, max).is_err())
    {
        return Err("invalid Loader archive path".into());
    }
    Ok(())
}
fn hash(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn unique_list(list: &[String]) -> bool {
    !list.is_empty() && list.len() <= 64 && list.iter().collect::<BTreeSet<_>>().len() == list.len()
}
fn validate_manifest(manifest: &Manifest, grants: &BTreeSet<String>) -> Result<()> {
    if manifest.protocol != 3 || manifest.bundles.len() > 64 {
        return Err("invalid Loader manifest protocol/bundle count".into());
    }
    let mut keys = BTreeSet::new();
    let mut total = 0usize;
    for bundle in &manifest.bundles {
        literal(&bundle.owner, 64)?;
        literal(&bundle.id, 48)?;
        literal(&bundle.version, 32)?;
        canonical_path(&bundle.artifact, 160)?;
        if !hash(&bundle.sha256) || !(1..=MAX_BYTES).contains(&bundle.size_bytes) {
            return Err("invalid Loader manifest hash/size".into());
        }
        total = total
            .checked_add(bundle.size_bytes)
            .ok_or("Loader transfer total overflow")?;
        if total > MAX_BYTES {
            return Err("native Loader total bundle byte limit".into());
        }
        if !unique_list(&bundle.loaders)
            || !bundle.loaders.iter().any(|p| p == "native")
            || bundle
                .loaders
                .iter()
                .any(|p| !["native", "fabric", "neoforge", "forge"].contains(&p.as_str()))
        {
            return Err("Loader bundle does not support native platform".into());
        }
        if !unique_list(&bundle.content) || !unique_list(&bundle.permissions) {
            return Err("invalid Loader manifest list".into());
        }
        for content in &bundle.content {
            let permission = match content.as_str() {
                "views" => "present_views",
                "view_actions" => "send_view_actions",
                "assets" => "load_assets",
                "items" => "register_items",
                "world_selection" => "send_world_selection",
                "world_previews" => "present_world_previews",
                "sounds" => "play_sounds",
                "blocks" => return Err("unsupported native Loader content blocks: dynamic block registry and carrier states are not implemented".into()),
                _ => return Err(format!("unsupported native Loader content {content}")),
            };
            if !bundle.permissions.iter().any(|p| p == permission) {
                return Err("Loader content missing required permission".into());
            }
        }
        if bundle
            .permissions
            .iter()
            .any(|p| !PERMISSIONS.contains(&p.as_str()) || !grants.contains(p))
        {
            return Err("Loader permission not locally granted or unsupported".into());
        }
        let expected = format!(
            "{}:{}/{}/{}",
            bundle.owner, bundle.id, bundle.version, bundle.sha256
        );
        if expected != bundle.cache_key || !keys.insert(&bundle.cache_key) {
            return Err("Loader cache identity mismatch/duplicate".into());
        }
    }
    Ok(())
}
struct ActivatedBundle {
    screens: BTreeMap<String, Screen>,
    assets: BTreeMap<String, Arc<[u8]>>,
    paths: BTreeMap<String, Arc<[u8]>>,
    previews: BTreeMap<String, Arc<previews::WorldPreview>>,
    items: Vec<Value>,
    sounds: Vec<Value>,
    ids: BTreeSet<String>,
}
fn activate(
    bundle: &Bundle,
    bytes: &[u8],
    screens: &BTreeMap<String, Screen>,
    assets: &BTreeMap<String, Arc<[u8]>>,
    paths: &BTreeMap<String, Arc<[u8]>>,
    existing_ids: &BTreeSet<String>,
) -> Result<ActivatedBundle> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    if zip.is_empty() || zip.len() > 129 {
        return Err("Loader archive entry limit".into());
    }
    let mut first = zip.by_index(0).map_err(|e| e.to_string())?;
    if first.name() != "solaris-client.json" || first.is_dir() || first.size() > MAX_MESSAGE as u64
    {
        return Err("Loader archive must begin with bounded solaris-client.json".into());
    }
    let mut index_bytes = Vec::with_capacity(first.size() as usize);
    first
        .by_ref()
        .take((MAX_MESSAGE + 1) as u64)
        .read_to_end(&mut index_bytes)
        .map_err(|e| e.to_string())?;
    if index_bytes.len() > MAX_MESSAGE {
        return Err("Loader index byte limit".into());
    }
    drop(first);
    let index: Index = serde_json::from_slice(&index_bytes).map_err(|e| e.to_string())?;
    if index.schema != 2 {
        return Err(format!(
            "unsupported native Loader index schema {}",
            index.schema
        ));
    }
    if !index.blocks.is_empty() {
        return Err("unsupported native Loader index blocks: dynamic block registry and carrier states are not implemented".into());
    }
    if screens.len() + index.screens.len() > 64 || assets.len() + index.assets.len() > 128 {
        return Err("Loader activated registry limit".into());
    }
    let declared = |kind| bundle.content.iter().any(|c| c == kind);
    for (kind, populated) in [
        ("views", !index.screens.is_empty()),
        ("assets", !index.assets.is_empty()),
        ("items", !index.items.is_empty()),
        ("world_previews", !index.world_previews.is_empty()),
        ("sounds", !index.sounds.is_empty()),
    ] {
        if declared(kind) != populated {
            return Err(format!(
                "Loader {kind} index disagrees with manifest content"
            ));
        }
    }
    let previews = previews::parse(&bundle.owner, &index.world_previews)?;
    previews::validate_references(&index.screens, &previews)?;
    let mut ids = BTreeSet::new();
    let mut staged_screens = BTreeMap::new();
    for definition in index.screens {
        UiState::validate_loader_definition(&definition)?;
        let id = identifier(&definition, "id")?.to_owned();
        owned(&id, &bundle.owner)?;
        if definition.get("item_id").is_some() || definition.get("block_id").is_some() {
            return Err("native Loader item/block-bound view opening unsupported".into());
        }
        if !ids.insert(id.clone()) || existing_ids.contains(&id) {
            return Err("duplicate activated Loader id".into());
        }
        staged_screens.insert(
            id,
            Screen {
                definition,
                actions: declared("view_actions"),
                world_selection: declared("world_selection"),
            },
        );
    }
    for entry in index
        .items
        .iter()
        .chain(index.world_previews.iter())
        .chain(index.sounds.iter())
    {
        let id = identifier(entry, "id")?;
        owned(id, &bundle.owner)?;
        if !ids.insert(id.to_owned()) || existing_ids.contains(id) {
            return Err(format!("duplicate activated Loader id {id}"));
        }
    }
    let mut by_path = BTreeMap::new();
    let mut total = assets.values().map(|data| data.len()).sum::<usize>();
    for asset in index.assets {
        owned(&asset.id, &bundle.owner)?;
        canonical_path(&asset.path, 256)?;
        if !asset.path.starts_with(&format!("assets/{}/", bundle.owner))
            || !hash(&asset.sha256)
            || asset.size_bytes == 0
            || asset.size_bytes > MAX_BYTES
            || !ids.insert(asset.id.clone())
            || existing_ids.contains(&asset.id)
            || paths.contains_key(&asset.path)
        {
            return Err(
                "invalid Loader asset index: owned unique canonical path and content id required"
                    .into(),
            );
        }
        total = total
            .checked_add(asset.size_bytes)
            .ok_or("Loader asset byte overflow")?;
        if total > MAX_BYTES {
            return Err("Loader activated asset byte limit".into());
        }
        if by_path.insert(asset.path.clone(), asset).is_some() {
            return Err("duplicate Loader asset path".into());
        }
    }
    let mut staged_assets = BTreeMap::new();
    let mut staged_paths = BTreeMap::new();
    for position in 1..zip.len() {
        let mut entry = zip.by_index(position).map_err(|e| e.to_string())?;
        if entry.is_dir() {
            return Err("Loader archive directory entry unsupported".into());
        }
        let asset = by_path
            .remove(entry.name())
            .ok_or("undeclared/duplicate Loader archive entry")?;
        if entry.size() != asset.size_bytes as u64 {
            return Err("Loader asset declared size mismatch".into());
        }
        let mut data = Vec::with_capacity(asset.size_bytes);
        entry
            .by_ref()
            .take((asset.size_bytes + 1) as u64)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        if data.len() != asset.size_bytes || format!("{:x}", Sha256::digest(&data)) != asset.sha256
        {
            return Err("Loader asset hash/size mismatch".into());
        }
        let data: Arc<[u8]> = data.into();
        staged_assets.insert(asset.id, Arc::clone(&data));
        staged_paths.insert(asset.path, data);
    }
    if !by_path.is_empty() {
        return Err("Loader archive missing declared asset".into());
    }
    Ok(ActivatedBundle {
        screens: staged_screens,
        assets: staged_assets,
        paths: staged_paths,
        previews,
        items: index.items,
        sounds: index.sounds,
        ids,
    })
}
fn owned(id: &str, owner: &str) -> Result<()> {
    resource_id(id)?;
    if id.split_once(':').map(|(namespace, _)| namespace) != Some(owner) {
        return Err("Loader content id outside bundle owner".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    fn bundle() -> Vec<u8> {
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "solaris-client.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(br#"{"schema":2,"screens":[{"id":"example:panel","title":"Panel","widgets":[{"type":"action_button","action_id":"example:accept","label":"Accept","enabled":true}]}]}"#).unwrap();
        zip.finish().unwrap().into_inner()
    }
    fn manifest(bytes: &[u8]) -> Value {
        let sha = format!("{:x}", Sha256::digest(bytes));
        json!({"protocol":3,"bundles":[{"owner":"example","id":"ui","version":"1","artifact":"ui.zip","sha256":sha,"size_bytes":bytes.len(),"loaders":["native"],"content":["views","view_actions"],"permissions":["present_views","send_view_actions"],"cache_key":format!("example:ui/1/{sha}")} ]})
    }
    fn begin(value: &Value) -> Result<Loader> {
        let mut loader = Loader::default();
        loader.begin(
            &serde_json::to_vec(value).unwrap(),
            &["present_views".into(), "send_view_actions".into()]
                .into_iter()
                .collect(),
        )?;
        Ok(loader)
    }
    fn chunk(value: &Value, bytes: &[u8]) -> Vec<u8> {
        let key = value["bundles"][0]["cache_key"].as_str().unwrap();
        let mut out = Vec::new();
        out.extend(3u16.to_be_bytes());
        out.extend((key.len() as u16).to_be_bytes());
        out.extend(key.as_bytes());
        out.extend(0i64.to_be_bytes());
        out.push(1);
        out.extend(bytes);
        out
    }
    #[test]
    fn native_loader_skips_foreign_or_ungranted_bundle_before_validation() {
        let bytes = bundle();
        let mut value = manifest(&bytes);
        value["bundles"][0]["loaders"] = json!(["fabric"]);
        let mut loader = Loader::default();
        let ack = loader
            .begin(&serde_json::to_vec(&value).unwrap(), &BTreeSet::new())
            .unwrap();
        assert_eq!(ack.channel, "solaris:loader/ack");
        assert!(loader.acknowledged);
        assert_eq!(
            serde_json::from_slice::<Value>(&ack.payload).unwrap()["cached_bundles"],
            json!([])
        );

        value = manifest(&bytes);
        value["bundles"][0]["cache_key"] = json!("wrong");
        assert!(begin(&value).is_err());
        let mut loader = Loader::default();
        let ack = loader
            .begin(
                &serde_json::to_vec(&manifest(&bytes)).unwrap(),
                &BTreeSet::new(),
            )
            .unwrap();
        assert_eq!(ack.channel, "solaris:loader/ack");
        assert!(loader.acknowledged);
        value = manifest(&bytes);
        value["bundles"][0]["content"] = json!(["blocks"]);
        assert!(begin(&value).is_err());
    }
    #[test]
    fn native_loader_verifies_transfer_before_request_admission_and_cleanup() {
        let bytes = bundle();
        let value = manifest(&bytes);
        let mut loader = begin(&value).unwrap();
        assert!(loader.request_view("example:panel", true).is_err());
        let ack = loader.artifact(&chunk(&value, &bytes)).unwrap().unwrap();
        assert_eq!(ack.channel, "solaris:loader/ack");
        assert!(loader.request_view("example:panel", true).is_ok());
        assert!(loader.request_view("example:other", true).is_err());
        assert!(loader.request_view("example:panel", false).is_err());
        loader = Loader::default();
        assert!(loader.request_view("example:panel", true).is_err());
        assert_eq!(loader.observation()["declared_views"], json!([]));
    }
    #[test]
    fn native_loader_rejects_corrupt_and_wrong_sized_transfer() {
        let bytes = bundle();
        let value = manifest(&bytes);
        let mut corrupt = bytes.clone();
        corrupt[0] ^= 1;
        let mut loader = begin(&value).unwrap();
        assert!(loader.artifact(&chunk(&value, &corrupt)).is_err());
        assert!(!loader.acknowledged);
        let mut loader = begin(&value).unwrap();
        assert!(
            loader
                .artifact(&chunk(&value, &bytes[..bytes.len() - 1]))
                .is_err()
        );
        assert!(!loader.acknowledged);
    }
    #[test]
    fn native_loader_rejects_mismatched_schema_and_undeclared_archive_content() {
        for index in [
            br#"{"schema":1,"screens":[]}"#.as_slice(),
            br#"{"schema":2,"screens":[],"world_previews":[{"id":"example:unsupported"}]}"#
                .as_slice(),
        ] {
            let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
            zip.start_file(
                "solaris-client.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(index).unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            let value = manifest(&bytes);
            let mut loader = begin(&value).unwrap();
            assert!(loader.artifact(&chunk(&value, &bytes)).is_err());
            assert!(!loader.acknowledged);
            assert!(loader.request_view("example:panel", true).is_err());
        }
    }
    #[test]
    fn native_loader_verifies_indexed_asset_hash_not_only_bundle_hash() {
        let index = json!({"schema":2,"assets":[{"id":"example:data","path":"assets/example/data.txt","size_bytes":3,"sha256":"0".repeat(64)}]});
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "solaris-client.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&serde_json::to_vec(&index).unwrap()).unwrap();
        zip.start_file(
            "assets/example/data.txt",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(b"abc").unwrap();
        let bytes = zip.finish().unwrap().into_inner();
        let mut value = manifest(&bytes);
        value["bundles"][0]["content"] = json!(["assets"]);
        value["bundles"][0]["permissions"] = json!(["load_assets"]);
        let mut loader = Loader::default();
        loader
            .begin(
                &serde_json::to_vec(&value).unwrap(),
                &["load_assets".into()].into_iter().collect(),
            )
            .unwrap();
        assert!(loader.artifact(&chunk(&value, &bytes)).is_err());
        assert!(!loader.acknowledged);
    }
    #[test]
    fn rejected_bundle_does_not_publish_a_verified_prefix() {
        let index = json!({"schema":2,
        "screens":[{"id":"example:panel","title":"Panel","widgets":[]}],
        "assets":[
            {"id":"example:first","path":"assets/example/first.txt","size_bytes":3,"sha256":format!("{:x}", Sha256::digest(b"abc"))},
            {"id":"example:second","path":"assets/example/second.txt","size_bytes":3,"sha256":"0".repeat(64)}
        ]});
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.start_file(
            "solaris-client.json",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        zip.write_all(&serde_json::to_vec(&index).unwrap()).unwrap();
        for path in ["assets/example/first.txt", "assets/example/second.txt"] {
            zip.start_file(path, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"abc").unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let mut value = manifest(&bytes);
        value["bundles"][0]["content"] = json!(["views", "assets"]);
        value["bundles"][0]["permissions"] = json!(["present_views", "load_assets"]);
        let mut loader = Loader::default();
        loader
            .begin(
                &serde_json::to_vec(&value).unwrap(),
                &PERMISSIONS.iter().map(|s| (*s).to_owned()).collect(),
            )
            .unwrap();
        assert!(loader.artifact(&chunk(&value, &bytes)).is_err());
        assert!(loader.assets.is_empty());
        assert!(loader.verified_paths.is_empty());
        assert!(loader.screens.is_empty());
        assert!(loader.cache_keys.is_empty());
        assert!(!loader.acknowledged);
    }
    #[test]
    fn native_loader_invalidates_actions_on_dimension_and_connection_cleanup() {
        let bytes = bundle();
        let mut value = manifest(&bytes);
        value["bundles"][0]["content"] = json!(["views", "view_actions", "world_selection"]);
        value["bundles"][0]["permissions"] =
            json!(["present_views", "send_view_actions", "send_world_selection"]);
        let mut loader = Loader::default();
        loader
            .begin(
                &serde_json::to_vec(&value).unwrap(),
                &PERMISSIONS.iter().map(|s| (*s).to_owned()).collect(),
            )
            .unwrap();
        loader.artifact(&chunk(&value, &bytes)).unwrap();
        loader.view = Some(LiveView {
            id: "view-1".into(),
            key: "example:panel".into(),
            revision: 7,
            last_action: None,
            selections: [("token-1".into(), "example:accept".into())]
                .into_iter()
                .collect(),
        });
        let mut action = json!({"protocol":3,"message":"view_action","view_instance_id":"view-1","view_revision":7,"action_id":"example:accept","action_sequence":0,"fields":[]});
        assert!(loader.action(action.clone(), true).is_ok());
        assert!(loader.action(action.clone(), true).is_err());
        action["action_sequence"] = json!(1);
        action["view_revision"] = json!(6);
        assert!(loader.action(action.clone(), true).is_err());
        action["view_revision"] = json!(7);
        assert!(loader.action(action.clone(), false).is_err());
        action["selection_token"] = json!("stale-token");
        assert!(loader.action(action.clone(), true).is_err());
        action["selection_token"] = json!("token-1");
        assert!(loader.action(action.clone(), true).is_ok());
        let cancel =
            json!({"protocol":3,"message":"cancel_selection","selection_context_id":"token-1"});
        assert!(loader.action(cancel.clone(), true).is_ok());
        assert!(loader.action(cancel, true).is_err());
        action["action_sequence"] = json!(2);
        assert!(loader.action(action.clone(), true).is_err());
        loader.clear_view(None).unwrap();
        assert!(loader.action(action.clone(), true).is_err());
        assert!(loader.request_view("example:panel", true).is_ok());
        loader.reset();
        assert_eq!(loader.observation()["acknowledged"], false);
        assert!(loader.action(action, true).is_err());
        assert!(loader.request_view("example:panel", true).is_err());
    }
}
