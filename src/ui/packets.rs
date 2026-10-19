//! Presentation-only packets. IDs: C00 packet-ids.json; codecs: pinned 26.1.2 javap in .analysis/ui-contract.
use super::{
    render::{Painter, TextSink, WorldPainter},
    state::{EditorState, Screen, UiState},
};
use crate::{
    api::{DynamicGeometry, EntityMaterial, MeshGeometry},
    inventory::{Stack, read_template},
    protocol::{Reader, Result, content::Content},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, sync::Arc};
#[derive(Clone, Debug)]
pub struct ChatReceipt {
    pub sequence: u64,
    pub global_index: i32,
    pub index: i32,
    pub sender: String,
    pub signature: Option<Arc<[u8]>>,
    pub content: Arc<str>,
    pub timestamp: i64,
    pub salt: i64,
    pub last_seen: Vec<Arc<[u8]>>,
    pub unsigned: Option<Value>,
    pub filter: i32,
    pub filter_bits: Vec<i64>,
}
#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatVerification {
    Verified,
    NotVerified,
    InvalidSignature,
    InvalidCertificate,
    ExpiredKey,
    MissingKey,
    Unsigned,
}
impl ChatVerification {
    fn label(self) -> &'static str {
        match self {
            Self::Verified => "verified",
            Self::NotVerified => "not_verified",
            Self::InvalidSignature => "invalid_signature",
            Self::InvalidCertificate => "invalid_certificate",
            Self::ExpiredKey => "expired_key",
            Self::MissingKey => "missing_key",
            Self::Unsigned => "unsigned",
        }
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct ChatLine {
    pub sequence: u64,
    pub tick: u64,
    pub component: Value,
    pub sender: Option<String>,
    pub signature_present: bool,
    pub verification: &'static str,
    pub display_trust: &'static str,
    pub signed_content: Option<Arc<str>>,
    #[serde(skip)]
    pub display_modified: bool,
    pub filtered: bool,
    #[serde(skip)]
    pub signature: Option<Arc<[u8]>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MapDecoration {
    pub kind: i32,
    pub asset_id: Arc<str>,
    pub show_on_item_frame: bool,
    pub x: i8,
    pub y: i8,
    pub rotation: u8,
    pub name: Option<Value>,
    #[serde(skip)]
    sprite: Arc<str>,
    #[serde(skip)]
    pub(crate) label_layout: Option<Arc<super::shaping::TextLayout>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct MapState {
    pub id: i32,
    pub scale: u8,
    pub locked: bool,
    pub decorations: Vec<MapDecoration>,
    #[serde(skip)]
    pub colors: Arc<[u8; 16384]>,
    pub revision: u64,
    #[serde(skip)]
    runs: Arc<[MapPixelRun]>,
}
#[derive(Clone, Debug)]
struct MapPixelRun {
    bounds: [f32; 4],
    color: [f32; 3],
    alpha: f32,
}
fn map_pixel_runs(colors: &[u8; 16384]) -> Arc<[MapPixelRun]> {
    let mut runs = Vec::new();
    for y in 0..128 {
        let mut x = 0;
        while x < 128 {
            let value = colors[y * 128 + x];
            let start = x;
            x += 1;
            while x < 128 && colors[y * 128 + x] == value {
                x += 1;
            }
            let (color, alpha) = super::palette::color(value);
            if alpha != 0. {
                runs.push(MapPixelRun {
                    bounds: [start as f32, y as f32, (x - start) as f32, 1.],
                    color,
                    alpha,
                });
            }
        }
    }
    runs.into()
}
#[derive(Clone, Debug, Serialize)]
pub struct BossBar {
    pub name: Value,
    pub progress: f32,
    pub color: i32,
    pub overlay: i32,
    pub properties: u8,
}
#[derive(Clone, Debug, Serialize)]
pub struct AdvancementState {
    pub parent: Option<String>,
    pub display: Option<Value>,
    pub description: Option<Value>,
    pub icon: Option<Stack>,
    pub background: Option<String>,
    pub frame: i32,
    pub flags: i32,
    pub position: [f32; 2],
    pub requirements: Vec<Vec<String>>,
    pub progress: BTreeMap<String, Option<i64>>,
    pub done: bool,
    pub telemetry: bool,
}
impl UiState {
    pub fn chat(&self) -> &std::collections::VecDeque<ChatLine> {
        &self.chat
    }
    pub fn maps(&self) -> &BTreeMap<i32, MapState> {
        &self.maps
    }
    pub fn advancements(&self) -> &BTreeMap<String, AdvancementState> {
        &self.advancements
    }
    pub fn bosses(&self) -> &BTreeMap<String, BossBar> {
        &self.bosses
    }
    pub fn take_chat_receipt(&mut self) -> Option<ChatReceipt> {
        self.chat_receipts.pop_front()
    }
    pub fn mark_chat_verification(&mut self, sequence: u64, status: ChatVerification) {
        if let Some(line) = self
            .chat
            .iter_mut()
            .rev()
            .find(|line| line.sequence == sequence && line.signature_present)
        {
            line.verification = status.label();
            line.display_trust = if matches!(status, ChatVerification::Verified) {
                if line.display_modified {
                    "modified"
                } else {
                    "secure"
                }
            } else {
                "not_secure"
            };
            if !matches!(status, ChatVerification::Verified) {
                if let Some(signature) = &line.signature {
                    self.last_seen.ignore_pending(signature);
                }
            }
            self.invalidate();
        }
    }
    pub fn last_seen_update(&mut self) -> super::LastSeenUpdate {
        self.last_seen.update()
    }
    pub fn take_packet_command(&mut self) -> Option<super::UiCommand> {
        self.packet_commands.pop_front()
    }
    fn read_signature(&self, r: &mut Reader<'_>) -> Result<Arc<[u8]>> {
        match r.var()? {
            0 => Ok(Arc::from(r.take(256)?)),
            index if (1..=128).contains(&index) => self
                .signature_cache
                .get(index as usize - 1)
                .ok_or_else(|| "chat_signature_not_cached".into()),
            _ => Err("invalid_signature_reference".into()),
        }
    }
    pub fn clear_session(&mut self) {
        self.chat.clear();
        self.chat_expected_index = 0;
        self.chat_sequence = 0;
        self.signature_cache = super::chat_tracking::SignatureCache::default();
        self.last_seen = super::chat_tracking::LastSeenTracker::default();
        self.chat_receipts.clear();
        self.packet_commands.clear();
        self.maps.clear();
        self.bosses.clear();
        self.advancements.clear();
        self.selected_advancement = None;
        self.scroll = 0;
        self.statistics = super::statistics::StatisticsState::default();
        self.statistics_group = super::statistics::StatisticsGroup::default();
        self.title = None;
        self.subtitle = None;
        self.tab_header = None;
        self.tab_footer = None;
        self.editor = None;
        self.loader = None;
        self.fields.clear();
        self.hud = super::HudState::default();
        self.network_hud = super::hud::HudPackets::default();
        self.hud_sidebar = None;
        self.hud_level = None;
        self.hud_waypoint_styles.clear();
        self.death = None;
        self.loading = None;
        self.transition(Screen::Game);
        self.release_all();
        self.invalidate();
    }
    pub fn open_book(&mut self, stack: &Stack, slot: i32) -> Result<()> {
        let written = stack.components.get("minecraft:written_book_content");
        let writable = stack.components.get("minecraft:writable_book_content");
        let content = written.or(writable).ok_or("item_is_not_book")?;
        let raw = |v: &Value| -> Result<String> {
            if let Some(s) = v.as_str() {
                return Ok(s.into());
            }
            if let Some(s) = v["raw"].as_str() {
                return Ok(s.into());
            }
            self.text.language.plain(v.get("raw").unwrap_or(v))
        };
        let mut pages = Vec::new();
        for p in content["pages"]
            .as_array()
            .ok_or("book_pages_not_observed")?
        {
            pages.push(raw(p)?);
        }
        if pages.is_empty() {
            pages.push(String::new());
        }
        let title = content.get("title").map(raw).transpose()?;
        let author = content["author"].as_str().map(str::to_owned);
        self.open_editor(
            EditorState::Book {
                slot,
                pages,
                title,
                author,
                writable: writable.is_some(),
            },
            false,
        )
    }
    pub fn apply_hud_packet(
        &mut self,
        name: &str,
        bytes: &[u8],
        content: &Content,
    ) -> Result<bool> {
        let handled = self.network_hud.apply(name, bytes, |id| {
            content.registry_name("minecraft:number_format_type", id)
        })?;
        if handled {
            if name != "WAYPOINT" {
                self.hud_sidebar = None;
            }
            self.invalidate();
        }
        Ok(handled)
    }
    pub fn scoreboard(&self) -> &super::hud::ScoreboardState {
        &self.network_hud.scoreboard
    }
    pub fn waypoints(&self) -> &super::hud::WaypointState {
        &self.network_hud.waypoints
    }
    pub fn apply(&mut self, id: i32, r: &mut Reader<'_>, content: &Content) -> Result<bool> {
        // Returning false does not advance the reader, so another owner can consume its own packet.
        if !matches!(
            id,
            3 | 9 | 14 | 31 | 33 | 51 | 65 | 68 | 85 | 112 | 114 | 115 | 121 | 122 | 130 | 139 | 140
        ) {
            return Ok(false);
        }
        match id {
            139 | 140 => self.apply_dialog_packet(id, r)?,
            3 => self.statistics.apply(r, content)?,
            9 => {
                let key = uuid(r)?;
                let op = r.var()?;
                match op {
                    0 => {
                        if self.bosses.len() >= 64 && !self.bosses.contains_key(&key) {
                            return Err("boss_bar_limit".into());
                        }
                        let b = BossBar {
                            name: r.nbt()?,
                            progress: r.f32()?,
                            color: r.var()?,
                            overlay: r.var()?,
                            properties: r.u8()?,
                        };
                        if !(0..=6).contains(&b.color) || !(0..=4).contains(&b.overlay) {
                            return Err("boss_style_bound".into());
                        }
                        self.bosses.insert(key, b);
                    }
                    1 => {
                        self.bosses.remove(&key);
                    }
                    2 => {
                        let value = r.f32()?;
                        if let Some(b) = self.bosses.get_mut(&key) {
                            b.progress = value;
                        }
                    }
                    3 => {
                        let value = r.nbt()?;
                        if let Some(b) = self.bosses.get_mut(&key) {
                            b.name = value;
                        }
                    }
                    4 => {
                        let color = r.var()?;
                        let overlay = r.var()?;
                        if !(0..=6).contains(&color) || !(0..=4).contains(&overlay) {
                            return Err("boss_style_bound".into());
                        }
                        if let Some(b) = self.bosses.get_mut(&key) {
                            b.color = color;
                            b.overlay = overlay;
                        }
                    }
                    5 => {
                        let value = r.u8()?;
                        if let Some(b) = self.bosses.get_mut(&key) {
                            b.properties = value;
                        }
                    }
                    _ => return Err("boss_operation_bound".into()),
                }
            }
            14 => {
                let reset = r.bool()?;
                self.title = None;
                self.subtitle = None;
                if reset {
                    self.title_timing = [10, 70, 20];
                }
            }
            31 => {
                let signature = self.read_signature(r)?;
                self.chat
                    .retain(|line| line.signature.as_ref() != Some(&signature));
                self.last_seen.ignore_pending(&signature);
            }
            33 => {
                let message = r.nbt()?;
                let bound = read_chat_bound(r, content)?;
                let component = decorate(message, bound)?;
                self.push_chat(component, None, None, false)?;
            }
            51 => self.read_map(r, content)?,
            65 => {
                let global = r.var()?;
                if global != self.chat_expected_index {
                    return Err("chat_global_index_out_of_order".into());
                }
                self.chat_expected_index = self.chat_expected_index.wrapping_add(1);
                let sender = uuid(r)?;
                let index = r.var()?;
                if global < 0 || index < 0 {
                    return Err("invalid_chat_index".into());
                }
                let signature = if r.bool()? {
                    Some(Arc::from(r.take(256)?))
                } else {
                    None
                };
                let message = r.string(256)?;
                let timestamp = r.i64()?;
                let salt = r.i64()?;
                let mut last_seen = Vec::with_capacity(20);
                for _ in 0..r.count(20)? {
                    last_seen.push(self.read_signature(r)?);
                }
                let unsigned = if r.bool()? { Some(r.nbt()?) } else { None };
                let mask = r.var()?;
                let bits = if mask == 2 { r.longs(4)? } else { Vec::new() };
                if !(0..=2).contains(&mask) {
                    return Err("chat_filter_bound".into());
                }
                let bound = read_chat_bound(r, content)?;
                if self.chat_receipts.len() >= 128 {
                    return Err("chat_receipt_limit".into());
                }
                self.signature_cache.push(&last_seen, signature.as_ref());
                let signed_content: Arc<str> = Arc::from(message.as_str());
                if let Some(signature) = &signature {
                    self.last_seen.add_pending(Arc::clone(signature), mask != 1);
                    if let Some(offset) = self.last_seen.acknowledgement() {
                        self.packet_commands
                            .push_back(super::UiCommand::ChatAck { offset });
                    }
                }
                self.chat_receipts.push_back(ChatReceipt {
                    sequence: self
                        .chat_sequence
                        .checked_add(1)
                        .ok_or("chat_sequence_exhausted")?,
                    global_index: global,
                    index,
                    sender: sender.clone(),
                    signature: signature.as_ref().map(Arc::clone),
                    content: Arc::clone(&signed_content),
                    timestamp,
                    salt,
                    last_seen,
                    unsigned: unsigned.clone(),
                    filter: mask,
                    filter_bits: bits.iter().map(|&word| word as i64).collect(),
                });
                if mask != 1 {
                    let filtered = if mask == 2 {
                        let mut utf16: Vec<u16> = message.encode_utf16().collect();
                        for (i, c) in utf16.iter_mut().enumerate() {
                            if bits.get(i / 64).is_some_and(|b| b & (1 << (i % 64)) != 0) {
                                *c = u16::from(b'#');
                            }
                        }
                        String::from_utf16(&utf16).map_err(|_| "invalid_filtered_text")?
                    } else {
                        message
                    };
                    let component = decorate(
                        if mask == 2 {
                            json!({"text":filtered})
                        } else {
                            unsigned
                                .as_ref()
                                .cloned()
                                .unwrap_or_else(|| json!({"text":filtered}))
                        },
                        bound,
                    )?;
                    self.push_chat(component, Some(sender), signature, mask != 0)?;
                    let line = self.chat.back_mut().ok_or("chat_line_missing")?;
                    line.display_modified = !self
                        .text
                        .language
                        .plain(&line.component)?
                        .contains(signed_content.as_ref())
                        || unsigned
                            .as_ref()
                            .map(|component| {
                                self.text.language.runs(component).map(|runs| {
                                    runs.iter()
                                        .any(|run| run.style.font.as_ref() != "minecraft:default")
                                })
                            })
                            .transpose()?
                            .unwrap_or(false);
                    line.signed_content = Some(signed_content);
                } else {
                    self.chat_sequence = self
                        .chat_sequence
                        .checked_add(1)
                        .ok_or("chat_sequence_exhausted")?;
                }
            }
            68 => {
                let _player = r.var()?;
                self.death = Some(r.nbt()?);
                self.transition(Screen::Death);
            }
            85 => self.read_advancement_tab(r)?,
            112 => {
                self.subtitle = Some(r.nbt()?);
            }
            114 => {
                self.title = Some(r.nbt()?);
                self.title_started = self.tick;
            }
            115 => {
                let times = [r.i32()?, r.i32()?, r.i32()?];
                for (i, t) in times.into_iter().enumerate() {
                    if t >= 0 {
                        self.title_timing[i] = t.min(12000);
                    }
                }
            }
            121 => {
                let component = r.nbt()?;
                let overlay = r.bool()?;
                if overlay {
                    self.subtitle = Some(component);
                    self.title_started = self.tick;
                } else {
                    self.push_chat(component, None, None, false)?;
                }
            }
            122 => {
                self.tab_header = Some(r.nbt()?);
                self.tab_footer = Some(r.nbt()?);
            }
            130 => self.read_advancements(r, content)?,
            _ => unreachable!(),
        }
        r.done()?;
        self.invalidate();
        Ok(true)
    }
    fn push_chat(
        &mut self,
        component: Value,
        sender: Option<String>,
        signature: Option<Arc<[u8]>>,
        filtered: bool,
    ) -> Result<()> {
        self.text.language.runs(&component)?;
        if self.chat.len() == 100 {
            self.chat.pop_front();
        }
        self.chat_sequence = self
            .chat_sequence
            .checked_add(1)
            .ok_or("chat_sequence_exhausted")?;
        let sequence = self.chat_sequence;
        self.chat.push_back(ChatLine {
            sequence,
            tick: self.tick,
            component,
            sender,
            signature_present: signature.is_some(),
            verification: if signature.is_some() {
                "not_verified"
            } else {
                "unsigned"
            },
            display_trust: "not_secure",
            signed_content: None,
            display_modified: false,
            filtered,
            signature,
        });
        Ok(())
    }
    fn read_map(&mut self, r: &mut Reader<'_>, content: &Content) -> Result<()> {
        let id = r.var()?;
        let scale = r.u8()?;
        let locked = r.bool()?;
        if id < 0 || scale > 4 {
            return Err("map_header_bound".into());
        }
        let decorations = if r.bool()? {
            let n = r.count(4096)?;
            let mut out = Vec::with_capacity(n);
            for _ in 0..n {
                let kind = r.var()?;
                if kind < 0 {
                    return Err("map_decoration_bound".into());
                }
                let name = content.registry_name("minecraft:map_decoration_type", kind)?;
                let properties = self.creative_source["map_decorations"]
                    .get(name)
                    .ok_or_else(|| {
                        format!("verified map decoration metadata unavailable: {name}")
                    })?;
                let asset_id: Arc<str> = Arc::from(
                    properties["asset_id"]
                        .as_str()
                        .ok_or("map decoration asset ID")?,
                );
                let show_on_item_frame = properties["show_on_item_frame"]
                    .as_bool()
                    .ok_or("map decoration frame predicate")?;
                let (namespace, path) = asset_id
                    .split_once(':')
                    .ok_or("map decoration canonical asset ID")?;
                let sprite = Arc::from(format!("{namespace}:map/decorations/{path}"));
                let x = r.u8()? as i8;
                let y = r.u8()? as i8;
                let rotation = r.u8()? & 15;
                let name = if r.bool()? { Some(r.nbt()?) } else { None };
                let label_layout = if self.text.can_render() {
                    name.as_ref()
                        .map(|name| self.text.layout(name, f32::MAX).map(Arc::new))
                        .transpose()?
                } else {
                    None
                };
                out.push(MapDecoration {
                    kind,
                    asset_id,
                    show_on_item_frame,
                    x,
                    y,
                    rotation,
                    name,
                    sprite,
                    label_layout,
                });
            }
            Some(out)
        } else {
            None
        };
        let width = usize::from(r.u8()?);
        let patch = if width != 0 {
            let height = usize::from(r.u8()?);
            let x = usize::from(r.u8()?);
            let y = usize::from(r.u8()?);
            let bytes = r.array(16384)?;
            if height == 0 || x + width > 128 || y + height > 128 || bytes.len() != width * height {
                return Err("map_patch_bound".into());
            }
            Some((height, x, y, bytes))
        } else {
            None
        };
        r.done()?;
        if self.maps.len() >= 1024 && !self.maps.contains_key(&id) {
            return Err("map_residency_limit".into());
        }
        let map = self.maps.entry(id).or_insert_with(|| MapState {
            id,
            scale,
            locked,
            decorations: Vec::new(),
            colors: Arc::new([0; 16384]),
            revision: 0,
            runs: Arc::from([]),
        });
        map.scale = scale;
        map.locked = locked;
        if let Some(decorations) = decorations {
            map.decorations = decorations;
        }
        if let Some((height, x, y, bytes)) = patch {
            let pixels = Arc::make_mut(&mut map.colors);
            for row in 0..height {
                pixels[(y + row) * 128 + x..(y + row) * 128 + x + width]
                    .copy_from_slice(&bytes[row * width..(row + 1) * width]);
            }
            map.runs = map_pixel_runs(&map.colors);
        }
        map.revision += 1;
        Ok(())
    }
    fn read_advancements(&mut self, r: &mut Reader<'_>, content: &Content) -> Result<()> {
        let reset = r.bool()?;
        let mut added = Vec::new();
        for _ in 0..r.count(4096)? {
            let id = r.string(32767)?;
            let parent = if r.bool()? {
                Some(r.string(32767)?)
            } else {
                None
            };
            let mut a = AdvancementState {
                parent,
                display: None,
                description: None,
                icon: None,
                background: None,
                frame: 0,
                flags: 0,
                position: [0.; 2],
                requirements: Vec::new(),
                progress: BTreeMap::new(),
                done: false,
                telemetry: false,
            };
            if r.bool()? {
                a.display = Some(r.nbt()?);
                a.description = Some(r.nbt()?);
                a.icon = Some(read_template(r, content)?);
                a.frame = r.var()?;
                if !(0..=2).contains(&a.frame) {
                    return Err("advancement_frame_bound".into());
                }
                a.flags = r.i32()?;
                if a.flags & 1 != 0 {
                    a.background = Some(r.string(32767)?);
                }
                a.position = [r.f32()?, r.f32()?];
            }
            for _ in 0..r.count(4096)? {
                let mut group = Vec::new();
                for _ in 0..r.count(4096)? {
                    group.push(r.string(32767)?);
                }
                a.requirements.push(group);
            }
            a.telemetry = r.bool()?;
            added.push((id, a));
        }
        let mut removed = Vec::new();
        for _ in 0..r.count(4096)? {
            removed.push(r.string(32767)?);
        }
        let mut progress = Vec::new();
        for _ in 0..r.count(4096)? {
            let id = r.string(32767)?;
            let mut criteria = BTreeMap::new();
            for _ in 0..r.count(4096)? {
                let name = r.string(32767)?;
                let time = if r.bool()? { Some(r.i64()?) } else { None };
                if criteria.insert(name, time).is_some() {
                    return Err("duplicate_advancement_criterion".into());
                }
            }
            progress.push((id, criteria));
        }
        let _show = r.bool()?;
        r.done()?;
        self.apply_advancement_update(reset, added, removed, progress)?;
        Ok(())
    }
    pub fn append_map(
        &self,
        id: i32,
        content: &Content,
        transform: glam::Mat4,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        let map = self.maps.get(&id).ok_or("map_not_observed")?;
        map.append_world_geometry(content, &self.text, transform, [-1.; 2], false, out)
    }
}
impl MapState {
    /// One source renderer serves held maps, GUI maps, and item-frame maps.
    pub fn append_world_geometry(
        &self,
        content: &Content,
        text: &super::render::TextRenderer,
        transform: glam::Mat4,
        light: [f32; 2],
        in_frame: bool,
        out: &mut DynamicGeometry,
    ) -> Result<()> {
        let mut painter = WorldPainter::new(content, out, transform, light, EntityMaterial::Text)?;
        self.append_geometry(text, transform, in_frame, &mut painter)
    }
    pub fn append_gui_geometry(
        &self,
        content: &Content,
        text: &super::render::TextRenderer,
        transform: glam::Mat4,
        in_frame: bool,
        out: &mut MeshGeometry,
    ) -> Result<()> {
        self.append_geometry(
            text,
            transform,
            in_frame,
            &mut Painter {
                content,
                out,
                transform,
                light: [-1.; 2],
                render_layer: crate::api::RenderLayer::Translucent,
            },
        )
    }
    fn append_geometry<S: TextSink>(
        &self,
        text: &super::render::TextRenderer,
        transform: glam::Mat4,
        in_frame: bool,
        painter: &mut S,
    ) -> Result<()> {
        use glam::{Mat4, Vec3};
        painter.set_transform(transform * Mat4::from_translation(Vec3::new(0., 0., -0.01)));
        for run in self.runs.iter() {
            painter.solid(run.bounds, run.color, run.alpha);
        }
        let mut index = 0;
        for decoration in &self.decorations {
            if in_frame && !decoration.show_on_item_frame {
                continue;
            }
            let x = f32::from(decoration.x) / 2. + 64.;
            let y = f32::from(decoration.y) / 2. + 64.;
            let icon = transform
                * Mat4::from_translation(Vec3::new(x, y, -0.02))
                * Mat4::from_rotation_z(
                    f32::from(decoration.rotation) * std::f32::consts::TAU / 16.,
                )
                * Mat4::from_scale(Vec3::new(4., 4., 3.))
                * Mat4::from_translation(Vec3::new(-0.125, 0.125, -0.001 * index as f32))
                * Mat4::from_scale(Vec3::new(1., -1., 1.));
            painter.set_transform(icon);
            painter.image(
                &decoration.sprite,
                [-1., -1., 2., 2.],
                [0., 0., 1., 1.],
                [1.; 3],
                1.,
            )?;
            if let Some(layout) = &decoration.label_layout {
                let width = layout.width.ceil();
                let scale = (25. / width).clamp(0., 6. / 9.);
                let label = transform
                    * Mat4::from_translation(Vec3::new(x - width * scale / 2., y + 4., -0.025))
                    * Mat4::from_scale(Vec3::new(scale, scale, -1.))
                    * Mat4::from_translation(Vec3::new(0., 0., 0.1));
                painter.set_transform(label);
                text.append_text_layout(
                    layout,
                    0xffffffff,
                    &super::render::WorldTextOptions {
                        background: Some(0x80000000),
                        ..Default::default()
                    },
                    painter,
                )?;
            }
            index += 1;
        }
        Ok(())
    }
}
fn uuid(r: &mut Reader<'_>) -> Result<String> {
    let bytes = r.take(16)?;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    ))
}
fn read_decoration(r: &mut Reader<'_>) -> Result<Value> {
    let key = r.string(32767)?;
    let n = r.count(16)?;
    let mut args = Vec::with_capacity(n);
    for _ in 0..n {
        args.push(match r.var()? {
            0 => "sender",
            1 => "target",
            2 => "content",
            _ => return Err("chat_parameter_bound".into()),
        });
    }
    let style = r.nbt()?;
    Ok(json!({"translation_key":key,"parameters":args,"style":style}))
}
fn read_chat_bound(r: &mut Reader<'_>, content: &Content) -> Result<(Value, Value, Option<Value>)> {
    let holder = r.var()?;
    if holder < 0 {
        return Err("chat_type_bound".into());
    }
    let decoration = if holder == 0 {
        let chat = read_decoration(r)?;
        let _narration = read_decoration(r)?;
        chat
    } else {
        content.registry_entry("minecraft:chat_type", holder - 1)?["chat"].clone()
    };
    let name = r.nbt()?;
    let target = if r.bool()? { Some(r.nbt()?) } else { None };
    Ok((decoration, name, target))
}
fn decorate(
    message: Value,
    (decoration, name, target): (Value, Value, Option<Value>),
) -> Result<Value> {
    let key = decoration["translation_key"]
        .as_str()
        .ok_or("chat_decoration_missing")?;
    let mut args = Vec::new();
    for parameter in decoration["parameters"]
        .as_array()
        .ok_or("chat_parameters_missing")?
    {
        args.push(match parameter.as_str() {
            Some("sender") => name.clone(),
            Some("target") => target.clone().ok_or("chat_target_missing")?,
            Some("content") => message.clone(),
            _ => return Err("invalid_chat_parameter".into()),
        });
    }
    let mut result = decoration["style"].as_object().cloned().unwrap_or_default();
    result.insert("translate".into(), json!(key));
    result.insert("with".into(), json!(args));
    Ok(Value::Object(result))
}
