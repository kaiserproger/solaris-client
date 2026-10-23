//! Vanilla 26.1.2 `CreativeModeInventoryScreen` presentation and item search.
//!
//! Geometry (GUI pixels, relative to the 195x136 panel at
//! `((width - 195) / 2, (height - 136) / 2)`) mirrors the vanilla screen:
//! 9x5 item grid at (9,18), hotbar row at (9,112), scroll bar at (175,18),
//! search box at (82,6) 80x9, 26x32 tab sprites above/below the panel and the
//! destroy-item slot at (173,112). All art comes from the pinned client assets
//! (`gui/container/creative_inventory/tab_*.png`, `gui/sprites/container/
//! creative_inventory/*`).
use super::state::{NodeAction, Paint, Screen, UiState};
use crate::{
    inventory::InventoryStore,
    protocol::{Result, content::Content},
};
use serde_json::{Value, json};
use std::{collections::BTreeSet, sync::Arc};

const PANEL_WIDTH: f32 = 195.;
const PANEL_HEIGHT: f32 = 136.;
pub(crate) const COLUMNS: usize = 9;
pub(crate) const ROWS: usize = 5;
/// Vanilla `searchBox.setMaxLength(50)`.
pub(crate) const SEARCH_LIMIT: usize = 50;
pub(crate) const SEARCH_FIELD: &str = "creative/search";
pub(crate) const SCROLLBAR: &str = "creative/scrollbar";
/// `creative_stacks` keys from here up hold tab icons, never palette items.
pub(crate) const TAB_ICON_KEY: u32 = 0x8000_0000;
const SPRITES: &str = "gui/sprites/container/creative_inventory";
const SCROLL_TRACK_TOP: f32 = 18.;
const SCROLL_TRACK_HEIGHT: f32 = 112.;
const SCROLL_KNOB_HEIGHT: f32 = 15.;

/// One searchable palette entry of the selected tab.
#[derive(Clone, Debug)]
pub(crate) struct IndexEntry {
    /// Position in the tab's `items` array (also the `creative/item/{index}` id).
    pub index: u32,
    /// Registry item id.
    pub item: u32,
    /// Namespaced item name, e.g. `minecraft:stone`.
    pub name: String,
    /// Tooltip lines in the current language, newline separated.
    pub label: String,
    /// `label` lowercased once for case-insensitive search.
    pub lower: String,
}

/// Vanilla search semantics: the query is lowercased; an empty query lists
/// everything, `#tag` lists items in tags whose name contains `tag`
/// (`tag_items`), anything else matches the localized tooltip text or the
/// namespaced item name by substring. Returns positions into `entries`.
pub(crate) fn matching(
    entries: &[IndexEntry],
    query: &str,
    tag_items: &BTreeSet<u32>,
) -> Vec<usize> {
    let query = query.to_lowercase();
    entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            if query.starts_with('#') {
                tag_items.contains(&entry.item)
            } else {
                query.is_empty() || entry.lower.contains(&query) || entry.name.contains(&query)
            }
        })
        .map(|(position, _)| position)
        .collect()
}

/// Vanilla `isAlignedRight()` tabs sit against the panel's right edge.
fn aligned_right(tab: &Value) -> bool {
    matches!(tab["type"].as_str(), Some("HOTBAR" | "SEARCH" | "INVENTORY"))
        || tab["id"] == "minecraft:op_blocks"
}

/// Vanilla slot positions of the Survival Inventory tab, relative to the panel.
fn inventory_tab_slot(slot: usize) -> Option<[f32; 2]> {
    Some(match slot {
        5..=8 => {
            let index = slot - 5;
            [54. + (index / 2) as f32 * 54., 6. + (index % 2) as f32 * 27.]
        }
        9..=35 => [
            9. + ((slot - 9) % 9) as f32 * 18.,
            54. + ((slot - 9) / 9) as f32 * 18.,
        ],
        36..=44 => [9. + (slot - 36) as f32 * 18., 112.],
        45 => [35., 20.],
        _ => return None,
    })
}

impl UiState {
    /// `type` of the selected creative tab (`CATEGORY`, `INVENTORY`, `HOTBAR`, `SEARCH`).
    pub(crate) fn creative_kind(&self) -> &str {
        self.creative_source["tabs"]
            .get(self.creative_tab)
            .and_then(|tab| tab["type"].as_str())
            .unwrap_or("")
    }
    pub(crate) fn creative_hotbar_tab(&self) -> bool {
        self.screen == Screen::Creative && self.creative_kind() == "HOTBAR"
    }
    /// Vanilla `selectTab`: the search tab starts empty with its box focused;
    /// every other tab drops focus.
    pub(crate) fn creative_select_tab(&mut self, tab: usize, entered: bool) -> Result<()> {
        self.creative_tab = tab;
        self.creative_stacks.clear();
        self.creative_index = None;
        self.scroll = 0;
        if self.creative_kind() == "SEARCH" {
            if entered || !self.fields.contains_key(SEARCH_FIELD) {
                self.fields.remove(SEARCH_FIELD);
                self.ensure_search_field()?;
            }
            self.focus = Some(SEARCH_FIELD.into());
        } else {
            if self.focus.as_deref() == Some(SEARCH_FIELD) {
                self.focus = None;
            }
            self.fields.remove(SEARCH_FIELD);
        }
        self.invalidate();
        Ok(())
    }
    /// Text layout of an edit field plus its inset and horizontal scroll. The
    /// themed field wraps at its width; the borderless vanilla box lays text on one
    /// line and scrolls so the caret stays inside it (the node clip trims the rest).
    pub(crate) fn field_layout(
        &self,
        node: &super::state::SemanticNode,
        text: &str,
        cursor: usize,
    ) -> Result<(super::shaping::TextLayout, f32, f32)> {
        if matches!(node.paint, Paint::BareField(_)) {
            let layout = self.text.layout(&json!({ "text": text }), 4096.)?;
            let shift = (layout.caret(cursor)[0] - (node.bounds[2] - 1.)).max(0.);
            Ok((layout, 0., shift))
        } else {
            let layout = self
                .text
                .layout(&json!({ "text": text }), (node.bounds[2] - 8.).max(1.))?;
            Ok((layout, 4., 0.))
        }
    }
    fn ensure_search_field(&mut self) -> Result<()> {
        if !self.fields.contains_key(SEARCH_FIELD) {
            self.field(SEARCH_FIELD, String::new(), 256, false)?;
            self.fields
                .get_mut(SEARCH_FIELD)
                .expect("search field just created")
                .max_chars = Some(SEARCH_LIMIT);
        }
        Ok(())
    }
    /// Entries of the selected tab in the current language; rebuilt only when
    /// the tab, language or enabled features change.
    fn creative_entries(&mut self, content: &Content) -> Result<Arc<Vec<IndexEntry>>> {
        if let Some((tab, entries)) = &self.creative_index {
            if *tab == self.creative_tab {
                return Ok(Arc::clone(entries));
            }
        }
        let source = Arc::clone(&self.creative_source);
        let items = source["tabs"]
            .get(self.creative_tab)
            .ok_or("creative_tab_not_observed")?["items"]
            .as_array()
            .ok_or("creative_tab_items_required")?;
        if items.len() > 65536 {
            return Err("creative_tab_item_bound".into());
        }
        let by_name: std::collections::HashMap<&str, u32> = content
            .item_names
            .iter()
            .map(|(&id, name)| (name.as_str(), id))
            .collect();
        let mut entries = Vec::with_capacity(items.len());
        for (index, template) in items.iter().enumerate() {
            let name = template["id"]
                .as_str()
                .ok_or("creative_template_id_required")?;
            let item = *by_name
                .get(name)
                .ok_or("creative_template_item_not_observed")?;
            if !content
                .item_required_features(item)?
                .iter()
                .all(|flag| self.enabled_features.contains(flag))
            {
                continue;
            }
            let tooltip = template["tooltip"]
                .as_array()
                .ok_or("creative_tooltip_required")?;
            let mut label = String::new();
            for line in tooltip {
                if !label.is_empty() {
                    label.push('\n');
                }
                label.push_str(&self.text.language.plain(line)?);
            }
            entries.push(IndexEntry {
                index: index as u32,
                item,
                name: name.to_owned(),
                lower: label.to_lowercase(),
                label,
            });
        }
        let entries = Arc::new(entries);
        self.creative_index = Some((self.creative_tab, Arc::clone(&entries)));
        Ok(entries)
    }
    fn creative_tab_node(
        &mut self,
        index: usize,
        tab: &Value,
        selected: bool,
        panel: [f32; 2],
        content: &Content,
    ) -> Result<()> {
        let column = (tab["column"].as_u64().unwrap_or(0) as usize).min(6);
        let top_row = tab["row"] == "TOP";
        let x = panel[0]
            + if aligned_right(tab) {
                PANEL_WIDTH - 27. * (7 - column) as f32 + 1.
            } else {
                27. * column as f32
            };
        let y = if top_row {
            panel[1] - 28.
        } else {
            panel[1] + PANEL_HEIGHT - 4.
        };
        let id = tab["id"].as_str().unwrap_or("");
        let sprite = format!(
            "{SPRITES}/tab_{}_{}_{}",
            if top_row { "top" } else { "bottom" },
            if selected { "selected" } else { "unselected" },
            column + 1
        );
        let label = self.text.language.plain(&tab["title_component"])?;
        self.add(
            format!("creative/tab/{id}"),
            "button",
            label,
            [x, y, 26., 32.],
            true,
            None,
            NodeAction::CreativeTab(index),
            Paint::Image(sprite, [0., 0., 1., 1.]),
        )?;
        let key = TAB_ICON_KEY + index as u32;
        if !self.creative_stacks.contains_key(&key) {
            self.creative_stacks
                .insert(key, crate::inventory::template_stack(&tab["icon"], content)?);
        }
        self.add(
            format!("creative/tab/{id}/icon"),
            "image",
            String::new(),
            [x + 5., y + if top_row { 9. } else { 7. }, 16., 16.],
            true,
            None,
            NodeAction::None,
            Paint::Creative(key),
        )
    }
    pub(crate) fn creative_tree(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
    ) -> Result<()> {
        self.panel();
        let source = Arc::clone(&self.creative_source);
        let tabs = source["tabs"]
            .as_array()
            .ok_or("creative_tabs_required")?;
        let selected = tabs
            .get(self.creative_tab)
            .ok_or("creative_tab_not_observed")?;
        let kind = selected["type"].as_str().unwrap_or("");
        let panel = [
            ((self.width - PANEL_WIDTH) / 2.).floor(),
            ((self.height - PANEL_HEIGHT) / 2.).floor(),
        ];
        let listed = |tab: &Value| tab["id"] != "minecraft:op_blocks" || self.operator_permission;
        // Vanilla draws every unselected tab, then the panel, then the selected tab
        // on top so it merges with the panel border.
        let visible_tabs: Vec<_> = tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| listed(tab))
            .map(|(index, tab)| (index, tab.clone()))
            .collect();
        let retained = self.creative_tab;
        for (index, tab) in visible_tabs.iter().filter(|(index, _)| *index != retained) {
            self.creative_tab_node(*index, tab, false, panel, content)?;
        }
        let background = match kind {
            "SEARCH" => "tab_item_search",
            "INVENTORY" => "tab_inventory",
            _ => "tab_items",
        };
        self.add(
            "creative/background".into(),
            "image",
            String::new(),
            [panel[0], panel[1], PANEL_WIDTH, PANEL_HEIGHT],
            true,
            None,
            NodeAction::None,
            Paint::Image(
                format!("gui/container/creative_inventory/{background}"),
                [0., 0., PANEL_WIDTH, PANEL_HEIGHT],
            ),
        )?;
        if kind == "SEARCH" {
            self.ensure_search_field()?;
            let label = self
                .text
                .language
                .plain(&json!({"translate":"itemGroup.search"}))?;
            self.add(
                SEARCH_FIELD.into(),
                "textbox",
                label,
                [panel[0] + 82., panel[1] + 6., 80., 9.],
                true,
                None,
                NodeAction::Focus(SEARCH_FIELD.into()),
                Paint::BareField(SEARCH_FIELD.into()),
            )?;
            // Caret and drop shadow overhang the 80x9 box by a pixel.
            self.nodes.last_mut().expect("search node").clip =
                Some([panel[0] + 82., panel[1] + 5., 80., 11.]);
        } else {
            let title = self.text.language.plain(&selected["title_component"])?;
            self.add(
                "creative/title".into(),
                "text",
                title,
                [panel[0] + 8., panel[1] + 6., 160., 9.],
                true,
                None,
                NodeAction::None,
                Paint::Plain(0xff40_4040, false),
            )?;
        }
        match kind {
            "INVENTORY" => self.creative_inventory_tab(inventory, panel)?,
            "HOTBAR" => {
                self.creative_hotbar_grid(content, panel)?;
                self.creative_scrollbar(panel)?;
                self.creative_hotbar_slots(inventory, panel)?;
            }
            _ => {
                self.creative_palette(content, panel, kind == "SEARCH")?;
                self.creative_scrollbar(panel)?;
                self.creative_hotbar_slots(inventory, panel)?;
            }
        }
        if let Some((index, tab)) = visible_tabs.iter().find(|(index, _)| *index == retained) {
            self.creative_tab_node(*index, tab, true, panel, content)?;
        }
        Ok(())
    }
    fn creative_palette(&mut self, content: &Content, panel: [f32; 2], search: bool) -> Result<()> {
        let entries = self.creative_entries(content)?;
        let query = if search {
            self.fields
                .get(SEARCH_FIELD)
                .map(|field| field.text.to_lowercase())
                .unwrap_or_default()
        } else {
            String::new()
        };
        let tag_items = match query.strip_prefix('#') {
            Some(tag) => self.tag_item_ids(content, tag)?,
            None => BTreeSet::new(),
        };
        let matched = matching(&entries, &query, &tag_items);
        self.creative_scroll_max = matched.len().div_ceil(COLUMNS).saturating_sub(ROWS);
        self.scroll = self.scroll.min(self.creative_scroll_max);
        let visible: Vec<usize> = matched
            .into_iter()
            .skip(self.scroll * COLUMNS)
            .take(COLUMNS * ROWS)
            .collect();
        let keep: BTreeSet<u32> = visible.iter().map(|&at| entries[at].index).collect();
        self.creative_stacks
            .retain(|key, _| *key >= TAB_ICON_KEY || keep.contains(key));
        let source = Arc::clone(&self.creative_source);
        let items = source["tabs"][self.creative_tab]["items"]
            .as_array()
            .ok_or("creative_tab_items_required")?;
        for (position, at) in visible.into_iter().enumerate() {
            let entry = &entries[at];
            let template = &items[entry.index as usize];
            if !self.creative_stacks.contains_key(&entry.index) {
                self.creative_stacks.insert(
                    entry.index,
                    crate::inventory::template_stack(template, content)?,
                );
            }
            self.add(
                format!("creative/item/{}", entry.index),
                "slot",
                entry.label.clone(),
                [
                    panel[0] + 8. + (position % COLUMNS) as f32 * 18.,
                    panel[1] + 17. + (position / COLUMNS) as f32 * 18.,
                    18.,
                    18.,
                ],
                true,
                None,
                NodeAction::Creative(entry.index),
                Paint::Creative(entry.index),
            )?;
            self.nodes.last_mut().expect("new creative node").component = template["tooltip"]
                .as_array()
                .and_then(|lines| lines.first())
                .cloned()
                .map(Arc::new);
        }
        Ok(())
    }
    /// Item ids in item tags whose name contains `tag` (vanilla `#tag` search).
    fn tag_item_ids(&self, content: &Content, tag: &str) -> Result<BTreeSet<u32>> {
        let mut ids = BTreeSet::new();
        for (name, members) in content.registry_tags("minecraft:item")? {
            if name.to_lowercase().contains(tag) {
                ids.extend(members.iter().filter_map(|id| u32::try_from(*id).ok()));
            }
        }
        Ok(ids)
    }
    /// Vanilla hotbar tab: nine saved hotbars as grid rows; Ctrl+digit saves, digit loads.
    fn creative_hotbar_grid(&mut self, content: &Content, panel: [f32; 2]) -> Result<()> {
        self.creative_scroll_max = 9usize.saturating_sub(ROWS);
        self.scroll = self.scroll.min(self.creative_scroll_max);
        self.creative_stacks.retain(|key, _| *key >= TAB_ICON_KEY);
        for row in self.scroll..(self.scroll + ROWS).min(9) {
            let Some(stacks) = self.saved_hotbars[row].clone() else {
                continue;
            };
            for (column, stack) in stacks.iter().enumerate() {
                if stack.empty() {
                    continue;
                }
                let key = (row * COLUMNS + column) as u32;
                self.creative_stacks.insert(key, stack.clone());
                let name = self
                    .text
                    .language
                    .plain(super::text::item_name_component(content, stack)?.as_ref())?;
                self.add(
                    format!("creative/item/{key}"),
                    "slot",
                    name,
                    [
                        panel[0] + 8. + column as f32 * 18.,
                        panel[1] + 17. + (row - self.scroll) as f32 * 18.,
                        18.,
                        18.,
                    ],
                    true,
                    None,
                    NodeAction::Creative(key),
                    Paint::Creative(key),
                )?;
            }
        }
        Ok(())
    }
    fn creative_scrollbar(&mut self, panel: [f32; 2]) -> Result<()> {
        let max = self.creative_scroll_max;
        self.add(
            SCROLLBAR.into(),
            "scrollbar",
            String::new(),
            [
                panel[0] + 175.,
                panel[1] + SCROLL_TRACK_TOP,
                14.,
                SCROLL_TRACK_HEIGHT,
            ],
            true,
            None,
            NodeAction::CreativeScroll,
            Paint::HitRegion,
        )?;
        let offset = if max == 0 {
            0.
        } else {
            self.scroll as f32 / max as f32
        };
        let travel = SCROLL_TRACK_HEIGHT - SCROLL_KNOB_HEIGHT - 2.;
        self.add(
            "creative/scrollbar/knob".into(),
            "image",
            String::new(),
            [
                panel[0] + 175.,
                panel[1] + SCROLL_TRACK_TOP + (travel * offset).floor(),
                12.,
                SCROLL_KNOB_HEIGHT,
            ],
            true,
            None,
            NodeAction::None,
            Paint::Image(
                format!(
                    "{SPRITES}/{}",
                    if max > 0 { "scroller" } else { "scroller_disabled" }
                ),
                [0., 0., 1., 1.],
            ),
        )
    }
    /// Scroll row for a pointer at GUI y (vanilla drag mapping onto the 97px knob travel).
    pub(crate) fn creative_scroll_to_pointer(&mut self, y: f32) {
        let Some(top) = self
            .nodes
            .iter()
            .find(|node| node.id == SCROLLBAR)
            .map(|node| node.bounds[1])
        else {
            return;
        };
        let max = self.creative_scroll_max;
        if max == 0 {
            return;
        }
        let travel = SCROLL_TRACK_HEIGHT - SCROLL_KNOB_HEIGHT;
        let offset = ((y - top - SCROLL_KNOB_HEIGHT / 2.) / travel).clamp(0., 1.);
        let row = (offset * max as f32).round() as usize;
        if row != self.scroll {
            self.scroll = row.min(max);
            self.invalidate();
        }
    }
    fn creative_hotbar_slots(&mut self, inventory: &InventoryStore, panel: [f32; 2]) -> Result<()> {
        for index in 36..45usize {
            self.creative_slot(inventory, index, inventory_tab_slot(index), panel)?;
        }
        Ok(())
    }
    fn creative_slot(
        &mut self,
        inventory: &InventoryStore,
        index: usize,
        at: Option<[f32; 2]>,
        panel: [f32; 2],
    ) -> Result<()> {
        let at = at.ok_or("creative_slot_position")?;
        let known = inventory
            .menu()
            .slots
            .get(index)
            .is_some_and(Option::is_some);
        self.add(
            format!("slot/{index}"),
            "slot",
            String::new(),
            [panel[0] + at[0] - 1., panel[1] + at[1] - 1., 18., 18.],
            known,
            (!known).then_some("slot_not_observed"),
            NodeAction::Slot(index as i16),
            Paint::Slot(index),
        )
    }
    /// Survival Inventory tab: armor, offhand, preview, 3x9 storage, hotbar and destroy slot.
    fn creative_inventory_tab(
        &mut self,
        inventory: &InventoryStore,
        panel: [f32; 2],
    ) -> Result<()> {
        self.add(
            "menu/player_preview".into(),
            "image",
            String::new(),
            [panel[0] + 73., panel[1] + 6., 32., 43.],
            true,
            None,
            NodeAction::None,
            Paint::PlayerPreview,
        )?;
        for index in (5..=45usize).filter(|index| inventory_tab_slot(*index).is_some()) {
            self.creative_slot(inventory, index, inventory_tab_slot(index), panel)?;
        }
        self.add(
            "creative/trash".into(),
            "button",
            "Destroy Item".into(),
            [panel[0] + 172., panel[1] + 111., 18., 18.],
            true,
            None,
            NodeAction::CreativeClear,
            Paint::HitRegion,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::content::Content;

    fn entry(index: u32, item: u32, name: &str, label: &str) -> IndexEntry {
        IndexEntry {
            index,
            item,
            name: name.into(),
            label: label.into(),
            lower: label.to_lowercase(),
        }
    }
    fn sample() -> Vec<IndexEntry> {
        vec![
            entry(0, 1, "minecraft:stone", "Stone"),
            entry(1, 2, "minecraft:cobblestone", "Cobblestone"),
            entry(2, 3, "minecraft:stone_bricks", "Stone Bricks"),
            entry(3, 4, "minecraft:dirt", "Dirt"),
            entry(
                4,
                5,
                "minecraft:diamond_pickaxe",
                "Diamond Pickaxe\nWhen in Main Hand:\n+1 Attack Damage",
            ),
        ]
    }
    #[test]
    fn empty_query_lists_everything_in_order() {
        assert_eq!(matching(&sample(), "", &BTreeSet::new()), vec![0, 1, 2, 3, 4]);
    }
    #[test]
    fn name_contains_is_case_insensitive_and_live() {
        let entries = sample();
        let none = BTreeSet::new();
        assert_eq!(matching(&entries, "stone", &none), vec![0, 1, 2]);
        assert_eq!(matching(&entries, "STONE", &none), vec![0, 1, 2]);
        assert_eq!(matching(&entries, "StOn", &none), vec![0, 1, 2]);
        assert_eq!(matching(&entries, "stone b", &none), vec![2]);
        assert_eq!(matching(&entries, "dirt", &none), vec![3]);
        assert_eq!(matching(&entries, "zzz", &none), Vec::<usize>::new());
    }
    #[test]
    fn tooltip_lines_and_registry_names_match() {
        let entries = sample();
        let none = BTreeSet::new();
        assert_eq!(matching(&entries, "attack damage", &none), vec![4]);
        assert_eq!(matching(&entries, "minecraft:dirt", &none), vec![3]);
        assert_eq!(matching(&entries, "pickaxe", &none), vec![4]);
    }
    #[test]
    fn hash_query_selects_tag_members_only() {
        let entries = sample();
        let tagged: BTreeSet<u32> = [1, 3].into();
        // Items 1 and 3 are the entries at positions 0 and 2.
        assert_eq!(matching(&entries, "#stone", &tagged), vec![0, 2]);
        assert_eq!(matching(&entries, "#", &BTreeSet::new()), Vec::<usize>::new());
    }
    #[test]
    fn search_tab_focuses_an_empty_limited_field_and_other_tabs_drop_it() {
        let mut content = Content::test_cube();
        content.creative_tabs_report = Arc::from(
            br#"{"tabs":[
                {"id":"minecraft:building_blocks","type":"CATEGORY","row":"TOP","column":0,"items":[]},
                {"id":"minecraft:search","type":"SEARCH","row":"TOP","column":6,"items":[]}]}"#
                .as_slice(),
        );
        let mut ui = UiState::new_state(&content).expect("state-only UI");
        ui.transition(Screen::Creative);
        assert!(!ui.text_input_active(), "no field before the search tab");
        ui.creative_select_tab(1, true).unwrap();
        assert_eq!(ui.focus.as_deref(), Some(SEARCH_FIELD));
        assert!(ui.text_input_active(), "keyboard text must reach the search box");
        assert_eq!(ui.fields[SEARCH_FIELD].max_chars, Some(SEARCH_LIMIT));
        ui.fields.get_mut(SEARCH_FIELD).unwrap().insert("stone").unwrap();
        assert!(ui.fields.get_mut(SEARCH_FIELD).unwrap().insert(&"x".repeat(46)).is_err());
        ui.creative_select_tab(0, true).unwrap();
        assert_eq!(ui.focus, None);
        assert!(!ui.fields.contains_key(SEARCH_FIELD));
        ui.creative_select_tab(1, true).unwrap();
        assert_eq!(ui.fields[SEARCH_FIELD].text, "", "entering the tab starts empty");
        assert_eq!(ui.focus.as_deref(), Some(SEARCH_FIELD));
    }
    #[test]
    fn inventory_tab_slots_follow_vanilla_positions() {
        assert_eq!(inventory_tab_slot(5), Some([54., 6.]));
        assert_eq!(inventory_tab_slot(6), Some([54., 33.]));
        assert_eq!(inventory_tab_slot(7), Some([108., 6.]));
        assert_eq!(inventory_tab_slot(8), Some([108., 33.]));
        assert_eq!(inventory_tab_slot(9), Some([9., 54.]));
        assert_eq!(inventory_tab_slot(35), Some([153., 90.]));
        assert_eq!(inventory_tab_slot(36), Some([9., 112.]));
        assert_eq!(inventory_tab_slot(44), Some([153., 112.]));
        assert_eq!(inventory_tab_slot(45), Some([35., 20.]));
        assert_eq!(inventory_tab_slot(0), None);
    }
}
