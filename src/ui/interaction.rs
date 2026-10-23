use super::{
    ClipboardRequest, EditorState, Screen, UiCommand, UiEffects, UiIntent,
    state::{NodeAction, Paint, UiState},
};
use crate::{
    inventory::{InventoryStore, Stack},
    protocol::{Result, content::Content},
};
use serde_json::{Value, json};
use std::sync::Arc;

impl UiState {
    /// Profile-owned canonical templates; null denotes EMPTY, not an unknown slot.
    pub fn set_saved_hotbars(&mut self, value: &Value, content: &Content) -> Result<()> {
        let rows = value
            .as_array()
            .filter(|rows| rows.len() == 9)
            .ok_or("saved_hotbar_rows")?;
        let mut prepared = std::array::from_fn(|_| None);
        for (index, row) in rows.iter().enumerate() {
            if row.is_null() {
                continue;
            }
            let slots = row
                .as_array()
                .filter(|slots| slots.len() == 9)
                .ok_or("saved_hotbar_slots")?;
            let stacks = slots
                .iter()
                .map(|value| {
                    if value.is_null() {
                        Ok(Stack::default())
                    } else {
                        crate::inventory::template_stack(value, content)
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            prepared[index] = Some(Arc::new(stacks));
        }
        self.saved_hotbars = prepared;
        self.invalidate();
        Ok(())
    }
    /// Written pages retain components rather than serializing their text into edit buffers.
    pub fn open_read_only_book(
        &mut self,
        slot: i32,
        pages: Vec<Value>,
        title: Option<String>,
        author: Option<String>,
    ) -> Result<()> {
        if pages.is_empty() || pages.len() > 100 {
            return Err("book_page_limit".into());
        }
        let plain = pages
            .iter()
            .map(|page| self.text.language.plain(page))
            .collect::<Result<Vec<_>>>()?;
        self.open_editor(
            EditorState::Book {
                slot,
                pages: plain,
                title,
                author,
                writable: false,
            },
            false,
        )?;
        self.book_components = pages.into_iter().map(Arc::new).collect();
        self.invalidate();
        Ok(())
    }
    pub(crate) fn save_hotbar(
        &mut self,
        index: usize,
        inventory: &InventoryStore,
        content: &Content,
        effects: &mut UiEffects,
    ) -> Result<()> {
        if self.screen != Screen::Creative || index >= 9 {
            return Err("creative_hotbar_unavailable".into());
        }
        let stacks = inventory.player.slots[36..45]
            .iter()
            .map(|value| value.clone().ok_or("hotbar_slot_not_observed".into()))
            .collect::<Result<Vec<_>>>()?;
        let mut saved = self.saved_hotbars.clone();
        saved[index] = Some(Arc::new(stacks));
        let rows = saved
            .iter()
            .map(|row| {
                row.as_ref()
                    .map(|stacks| {
                        stacks
                            .iter()
                            .map(|stack| {
                                if stack.empty() {
                                    Ok(Value::Null)
                                } else {
                                    stack.persistent_template(content)?.json()
                                }
                            })
                            .collect::<Result<Vec<_>>>()
                            .map(Value::Array)
                    })
                    .unwrap_or(Ok(Value::Null))
            })
            .collect::<Result<Vec<_>>>()?;
        self.saved_hotbars = saved;
        effects.commands.push(UiCommand::Settings {
            key: "saved_hotbars".into(),
            value: Value::Array(rows),
        });
        self.invalidate();
        Ok(())
    }
    pub(crate) fn load_hotbar(
        &mut self,
        index: usize,
        inventory: &InventoryStore,
        effects: &mut UiEffects,
    ) -> Result<()> {
        if self.screen != Screen::Creative || index >= 9 {
            return Err("creative_hotbar_unavailable".into());
        }
        for slot in 0..9 {
            let stack = self.saved_hotbars[index]
                .as_ref()
                .map_or_else(Stack::default, |stacks| stacks[slot].clone());
            effects.commands.push(UiCommand::CreativeSetStack {
                context: self.context(inventory),
                slot: (36 + slot) as i16,
                stack,
            });
        }
        Ok(())
    }
    pub(crate) fn rich_label(
        &mut self,
        id: &str,
        component: Arc<Value>,
        bounds: [f32; 4],
        interactive: bool,
    ) -> Result<()> {
        let label = self.text.language.plain(&component)?;
        let layout = Arc::new(self.text.layout(&component, bounds[2].max(1.))?);
        self.add(
            id.into(),
            "text",
            label,
            bounds,
            true,
            None,
            NodeAction::None,
            Paint::RichText(Arc::clone(&layout)),
        )?;
        self.nodes.last_mut().expect("new rich node").component = Some(component);
        if interactive {
            // One region per contiguous run/visual line; byte order need not equal visual order.
            let mut regions: Vec<(usize, [f32; 4])> = Vec::new();
            for glyph in &layout.glyphs {
                if layout.runs[glyph.style].style.click.is_none() || glyph.advance <= 0. {
                    continue;
                }
                if let Some((style, rect)) = regions.last_mut() {
                    if *style == glyph.style
                        && rect[1] == glyph.position[1]
                        && (rect[0] + rect[2] - glyph.position[0]).abs() < 0.01
                    {
                        rect[2] += glyph.advance;
                        continue;
                    }
                }
                regions.push((
                    glyph.style,
                    [glyph.position[0], glyph.position[1], glyph.advance, 9.],
                ));
            }
            for (index, (style, mut rect)) in regions.into_iter().enumerate() {
                rect[0] += bounds[0];
                rect[1] += bounds[1];
                if rect[1] + rect[3] > bounds[1] + bounds[3] {
                    continue;
                }
                let event = layout.runs[style]
                    .style
                    .click
                    .as_ref()
                    .expect("click region");
                self.add(
                    format!("{id}/action/{index}"),
                    "link",
                    layout.runs[style].text.clone(),
                    rect,
                    true,
                    None,
                    NodeAction::ComponentClick(Arc::clone(event)),
                    Paint::HitRegion,
                )?;
                // The parent rich node paints text once. Interaction regions are transparent.
            }
        }
        Ok(())
    }
    pub(crate) fn click_component(
        &mut self,
        event: &Value,
        inventory: &mut InventoryStore,
        content: &Content,
        effects: &mut UiEffects,
    ) -> Result<()> {
        if !matches!(self.screen, Screen::Chat | Screen::Book) {
            return Err("component_action_not_visible".into());
        }
        match event["action"].as_str().ok_or("component_click_action")? {
            "open_url" => {
                let value = event["url"].as_str().ok_or("component_url_required")?;
                self.pending_link = Some(super::links::safe_url(value)?);
                self.focus = None;
                self.release_all();
                self.invalidate();
            }
            "copy_to_clipboard" => {
                let value = event["value"].as_str().ok_or("component_clipboard_value")?;
                if value.len() > 32767 || self.clipboard.len() >= 4 {
                    return Err("clipboard_queue_or_value_bound".into());
                }
                self.clipboard
                    .push_back(ClipboardRequest::Set(value.into()));
            }
            "suggest_command" => {
                let value = event["command"]
                    .as_str()
                    .ok_or("component_command_required")?;
                self.execute_inner(
                    UiIntent::OpenChat { command: false },
                    inventory,
                    content,
                    effects,
                )?;
                self.fields
                    .get_mut("chat/input")
                    .ok_or("chat_input_not_visible")?
                    .insert(value)?;
                self.invalidate();
            }
            "run_command" => {
                let value = event["command"]
                    .as_str()
                    .ok_or("component_command_required")?;
                if !value.starts_with('/') {
                    return Err("component_command_prefix_required".into());
                }
                let was_book = self.screen == Screen::Book;
                if was_book {
                    self.transition(Screen::Chat);
                }
                self.execute_inner(
                    UiIntent::SendChat { text: value.into() },
                    inventory,
                    content,
                    effects,
                )?;
                if was_book {
                    self.transition(Screen::Book);
                }
            }
            "change_page" => {
                if self.screen != Screen::Book || self.book_components.is_empty() {
                    return Err("component_page_unavailable".into());
                }
                let page = event["page"].as_u64().ok_or("component_page_required")?;
                if page == 0 || page > self.book_components.len() as u64 {
                    return Err("book_page_bound".into());
                }
                self.page = page as usize - 1;
                self.invalidate();
            }
            _ => return Err("component_click_action_not_allowed".into()),
        }
        Ok(())
    }
    pub(crate) fn rich_overlays(&mut self, content: &Content) -> Result<()> {
        if let Some(url) = self.pending_link.clone() {
            for node in &mut self.nodes {
                node.enabled = false;
                node.deny_reason = Some("link_confirmation_required");
            }
            let bounds = [self.width / 2. - 165., self.height / 2. - 75., 330., 150.];
            self.add(
                "link/background".into(),
                "dialog",
                url.clone(),
                bounds,
                true,
                None,
                NodeAction::None,
                Paint::Solid([0.08; 3], 1.),
            )?;
            let title = self
                .text
                .language
                .plain(&json!({"translate":"chat.link.confirm"}))?;
            self.label(
                "link/title",
                title,
                [bounds[0] + 8., bounds[1] + 8., 314., 20.],
            )?;
            self.rich_label(
                "link/url",
                Arc::new(json!({"text":url})),
                [bounds[0] + 8., bounds[1] + 32., 314., 42.],
                false,
            )?;
            let warning = self
                .text
                .language
                .plain(&json!({"translate":"chat.link.warning"}))?;
            self.label(
                "link/warning",
                warning,
                [bounds[0] + 8., bounds[1] + 76., 314., 28.],
            )?;
            for (index, (id, key, action)) in [
                ("link/accept", "gui.yes", NodeAction::LinkAccept),
                ("link/copy", "chat.copy", NodeAction::LinkCopy),
                ("link/cancel", "gui.no", NodeAction::LinkCancel),
            ]
            .into_iter()
            .enumerate()
            {
                let label = self.text.language.plain(&json!({"translate":key}))?;
                self.button(
                    id,
                    &label,
                    [
                        bounds[0] + 8. + index as f32 * 106.,
                        bounds[1] + 116.,
                        100.,
                        20.,
                    ],
                    action,
                    true,
                    None,
                )?;
            }
            return Ok(());
        }
        let hovered = self.nodes.iter().rev().find_map(|node| {
            let [x, y, w, h] = node.bounds;
            let [px, py] = self.pointer;
            if !node.visible || !node.enabled || px < x || py < y || px >= x + w || py >= y + h {
                return None;
            }
            let Paint::RichText(layout) = &node.paint else {
                return None;
            };
            let p = [
                self.pointer[0] - node.bounds[0],
                self.pointer[1] - node.bounds[1],
            ];
            layout
                .glyphs
                .iter()
                .find(|glyph| {
                    p[0] >= glyph.position[0]
                        && p[0] < glyph.position[0] + glyph.advance
                        && p[1] >= glyph.position[1]
                        && p[1] < glyph.position[1] + 9.
                })
                .and_then(|glyph| layout.runs[glyph.style].style.hover.clone())
        });
        let Some(event) = hovered else {
            return Ok(());
        };
        let component = match event["action"].as_str() {
            Some("show_text") => event.get("value").cloned().ok_or("hover_text_required")?,
            Some("show_item") => {
                let stack = crate::inventory::template_stack(&event, content)?;
                super::text::item_name_component(content, &stack)?.into_owned()
            }
            Some("show_entity") => {
                if self
                    .settings
                    .get("advanced_tooltips")
                    .and_then(Value::as_bool)
                    != Some(true)
                {
                    return Ok(());
                }
                let entity = event["type"].as_str().ok_or("hover_entity_type_required")?;
                let id = event["id"].as_str().ok_or("hover_entity_id_required")?;
                let mut lines = Vec::new();
                if let Some(name) = event.get("name") {
                    lines.push(name.clone());
                    lines.push(json!({"text":"\n"}));
                }
                lines.push(json!({"translate":format!("entity.{}",entity.replace(':', "."))}));
                lines.push(json!({"text":format!("\n{id}")}));
                Value::Array(lines)
            }
            _ => return Err("component_hover_action_not_allowed".into()),
        };
        let width = 240f32.min((self.width - 16.).max(1.));
        let layout = self.text.layout(&component, width)?;
        let x = (self.pointer[0] + 12.).min(self.width - width - 8.).max(0.);
        let y = (self.pointer[1] + 12.)
            .min(self.height - layout.height - 8.)
            .max(0.);
        self.add(
            "component/hover/background".into(),
            "tooltip",
            String::new(),
            [x - 3., y - 3., width + 6., layout.height + 6.],
            true,
            None,
            NodeAction::None,
            Paint::Solid([0.06; 3], 0.94),
        )?;
        self.rich_label(
            "component/hover/text",
            Arc::new(component),
            [x, y, width, layout.height],
            false,
        )?;
        Ok(())
    }
    pub(crate) fn narrate_focus(&self, effects: &mut UiEffects) -> Result<()> {
        if !matches!(
            self.settings.get("narrator").and_then(Value::as_u64),
            Some(1 | 3)
        ) {
            return Ok(());
        }
        let Some(id) = &self.focus else {
            return Ok(());
        };
        let Some(node) = self
            .nodes
            .iter()
            .find(|node| &node.id == id && node.visible && node.enabled)
        else {
            return Ok(());
        };
        let text = if node.label.is_empty() {
            self.fields
                .get(id)
                .map(|field| field.text.as_str())
                .unwrap_or(id)
        } else {
            node.label.as_str()
        };
        if !text.is_empty() {
            effects
                .commands
                .push(UiCommand::Window(super::WindowEffect::Narrate(text.into())));
        }
        Ok(())
    }
    pub(crate) fn hud_accessibility_tree(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
    ) -> Result<()> {
        let mut rows = Vec::new();
        if let Some(health) = self.hud.health {
            rows.push(self.hud.max_health.map_or_else(
                || format!("Health {health}"),
                |max| format!("Health {health}/{max}"),
            ));
        }
        for (label, value) in [
            ("Food", self.hud.food),
            ("Armor", self.hud.armor),
            ("Air", self.hud.air),
            ("Experience level", self.hud.level),
        ] {
            if let Some(value) = value {
                rows.push(format!("{label} {value}"));
            }
        }
        if self.hud.absorption > 0. {
            rows.push(format!("Absorption {}", self.hud.absorption));
        }
        let slot = 36 + usize::from(inventory.selected_slot);
        if let Some(stack) = inventory.player.slots.get(slot).and_then(Option::as_ref) {
            if !stack.empty() {
                rows.push(format!(
                    "Selected {} ×{}",
                    self.text
                        .language
                        .plain(super::text::item_name_component(content, stack)?.as_ref())?,
                    stack.count
                ));
            }
        }
        self.add(
            "hud/status".into(),
            "status",
            rows.join("; "),
            [0., self.height - 1., 1., 1.],
            true,
            None,
            NodeAction::None,
            Paint::HitRegion,
        )
    }
}
