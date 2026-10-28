use super::{
    layout::Layout,
    state::{EditorState, NodeAction, Paint, Screen, UiState},
    statistics::StatisticsGroup,
};
use crate::protocol::content::Content;
use crate::{
    inventory::{InventoryStore, MenuKind},
    protocol::Result,
};
use serde_json::{json, Value};
impl UiState {
    pub(crate) fn rebuild(&mut self, inventory: &InventoryStore, content: &Content) -> Result<()> {
        self.nodes.clear();
        self.anim.nodes_epoch += 1;
        match self.screen {
            Screen::Game => self.hud_tree(inventory, content)?,
            Screen::Menu => self.menu_tree(inventory, content, 0.)?,
            Screen::Creative => self.creative_tree(inventory, content)?,
            Screen::Loader => self.loader_tree()?,
            Screen::ServerDialog => self.dialog_tree()?,
            Screen::Chat => {
                self.hud_tree(inventory, content)?;
                self.edit_node(
                    "chat/input",
                    "Chat message",
                    [4., self.height - 24., self.width - 60., 20.],
                    true,
                )?;
                self.button(
                    "chat/send",
                    "Send",
                    [self.width - 52., self.height - 24., 48., 20.],
                    NodeAction::Send,
                    true,
                    None,
                )?;
            }
            Screen::Sign | Screen::Book | Screen::Command | Screen::Structure | Screen::Jigsaw => {
                self.editor_tree()?
            }
            Screen::Death => {
                // Vanilla DeathScreen: dark-red dirt-ish backdrop, centered
                // title, readable death message, two wide buttons. Compact
                // 20px-high buttons at a 24px pitch fit the 427x240 Auto
                // viewport (1280x720 window, parent-owned scale 3).
                self.panel();
                self.label(
                    "death/title",
                    "You died!".into(),
                    [self.width / 2. - 150., self.height / 5., 300., 24.],
                )?;
                if let Some(death) = &self.death {
                    let label = self.text.language.plain(death)?;
                    self.label(
                        "death/message",
                        label,
                        [self.width / 2. - 190., self.height / 5. + 30., 380., 30.],
                    )?;
                }
                // Large, themed hit areas (26 logical px tall).
                self.button(
                    "death/respawn",
                    "Respawn",
                    [self.width / 2. - 100., self.height / 2. - 14., 200., 26.],
                    NodeAction::Respawn,
                    self.can_respawn,
                    if self.can_respawn {
                        None
                    } else {
                        Some("respawn_forbidden")
                    },
                )?;
                self.button(
                    "death/disconnect",
                    "Title screen",
                    [self.width / 2. - 100., self.height / 2. + 20., 200., 26.],
                    NodeAction::Disconnect,
                    true,
                    None,
                )?;
            }
            Screen::Loading => {
                self.panel();
                self.label(
                    "loading/status",
                    self.loading.clone().unwrap_or_default(),
                    [20., self.height / 2. - 12., self.width - 40., 24.],
                )?;
            }
            Screen::Pause => {
                // Themed panel; Advancements/Statistics share one row as on the
                // source PauseScreen. 26px buttons on a 30px pitch fit 427x240.
                self.panel();
                let frame_h = 208.;
                let frame_y = ((self.height - frame_h) / 2.).floor().max(2.);
                let top = frame_y + 12.;
                self.add(
                    "pause/panel".into(),
                    "presentation",
                    String::new(),
                    [self.width / 2. - 116., frame_y, 232., frame_h],
                    true,
                    None,
                    NodeAction::None,
                    Paint::Frame,
                )?;
                self.label(
                    "pause/title",
                    "Game menu".into(),
                    [self.width / 2. - 100., top, 200., 20.],
                )?;
                for (i, (id, label, a)) in [
                    ("resume", "Back to game", NodeAction::Screen(Screen::Game)),
                    (
                        "advancements",
                        "Advancements",
                        NodeAction::Screen(Screen::Advancements),
                    ),
                    (
                        "statistics",
                        "Statistics",
                        NodeAction::Screen(Screen::Statistics),
                    ),
                    ("settings", "Options", NodeAction::Screen(Screen::Settings)),
                    (
                        "accessibility",
                        "Accessibility",
                        NodeAction::Screen(Screen::Accessibility),
                    ),
                    (
                        "disconnect",
                        if self.singleplayer_active {
                            "Save & Quit to Title"
                        } else {
                            "Disconnect"
                        },
                        NodeAction::Disconnect,
                    ),
                ]
                .into_iter()
                .enumerate()
                {
                    let paired = matches!(id, "advancements" | "statistics");
                    let row = if i > 1 { i - 1 } else { i };
                    self.button(
                        &format!("pause/{id}"),
                        label,
                        [
                            self.width / 2. - 102. + if id == "statistics" { 105. } else { 0. },
                            top + 30. + row as f32 * 30.,
                            if paired { 99. } else { 204. },
                            26.,
                        ],
                        a,
                        true,
                        None,
                    )?;
                }
            }
            Screen::Settings | Screen::Accessibility => self.options_tree()?,
            Screen::Advancements => self.advancements_tree()?,
            Screen::Statistics => self.statistics_tree(content)?,
            Screen::Title => self.title_tree()?,
            Screen::Multiplayer => self.multiplayer_tree()?,
            Screen::DirectConnect => self.connect_tree()?,
            Screen::Disconnected => self.disconnected_tree()?,
            Screen::Singleplayer => self.singleplayer_tree()?,
            Screen::CreateWorld => self.create_world_tree()?,
            Screen::DeleteWorld => self.delete_world_tree()?,
            Screen::SingleplayerStarting => self.local_start_tree()?,
        }
        self.rich_overlays(content)?;
        Ok(())
    }
    /// The list keeps the existing 40-pixel rows; received root buttons wrap
    /// instead of assuming the five built-in vanilla tabs.
    pub(crate) fn advancement_list_viewport(&self) -> (f32, usize) {
        let columns = ((self.width - 48.) / 144.).floor().max(1.) as usize;
        let tabs = self.advancement_tab_ids().len();
        let tab_rows = tabs.div_ceil(columns);
        let top = 40. + tab_rows as f32 * 28.;
        let rows = ((self.height - top - 44.) / 40.).floor().max(1.) as usize;
        (top, rows)
    }
    fn advancements_tree(&mut self) -> Result<()> {
        self.panel();
        let columns = ((self.width - 48.) / 144.).floor().max(1.) as usize;
        let tabs: Vec<_> = self
            .advancement_tab_ids()
            .into_iter()
            .map(str::to_owned)
            .collect();
        let selected = self.selected_advancement_root().map(str::to_owned);
        for (index, id) in tabs.into_iter().enumerate() {
            let display = self.advancements[&id].display.as_ref().unwrap();
            let label = self.text.language.plain(display)?;
            self.button(
                &format!("advancement/tab/{id}"),
                &label,
                [
                    24. + (index % columns) as f32 * 144.,
                    28. + (index / columns) as f32 * 28.,
                    140.,
                    24.,
                ],
                NodeAction::AdvancementTab(id.clone()),
                true,
                None,
            )?;
            self.nodes.last_mut().unwrap().focused = selected.as_deref() == Some(id.as_str());
        }
        let (top, visible_rows) = self.advancement_list_viewport();
        let root_rows = self.advancements.iter().filter(|(id, a)| {
            a.display.is_some()
                && selected.is_some()
                && self.advancement_root_for(id) == selected.as_deref()
        });
        let row_count = root_rows.clone().count();
        let scroll = self.scroll.min(row_count.saturating_sub(visible_rows));
        let entries: Vec<_> = root_rows
            .skip(scroll)
            .take(visible_rows)
            .map(|(id, a)| (id.clone(), a.display.clone().unwrap(), a.done))
            .collect();
        self.scroll = scroll;
        self.add(
            "advancements/list".into(),
            "list",
            "Advancements".into(),
            [24., top, self.width - 48., visible_rows as f32 * 40.],
            true,
            None,
            NodeAction::None,
            Paint::HitRegion,
        )?;
        for (index, (id, display, done)) in entries.into_iter().enumerate() {
            let text = format!(
                "{}{}",
                if done { "[Done] " } else { "" },
                self.text.language.plain(&display)?
            );
            self.label(
                &format!("advancement/{id}"),
                text,
                [24., top + index as f32 * 40., self.width - 48., 36.],
            )?;
        }
        self.button(
            "advancements/done",
            "Done",
            [self.width / 2. - 100., self.height - 34., 200., 26.],
            NodeAction::Screen(Screen::Pause),
            true,
            None,
        )
    }
    fn statistics_tree(&mut self, content: &Content) -> Result<()> {
        self.panel();
        let title = self
            .text
            .language
            .plain(&json!({"translate":"gui.stats","fallback":"Statistics"}))?;
        self.label("statistics/title", title, [24., 20., self.width - 48., 24.])?;
        for (index, (id, key, fallback, group)) in [
            (
                "general",
                "stat.generalButton",
                "General",
                StatisticsGroup::General,
            ),
            ("items", "stat.itemsButton", "Items", StatisticsGroup::Items),
            ("mobs", "stat.mobsButton", "Mobs", StatisticsGroup::Mobs),
        ]
        .into_iter()
        .enumerate()
        {
            let label = self
                .text
                .language
                .plain(&json!({"translate":key,"fallback":fallback}))?;
            self.button(
                &format!("statistics/tab/{id}"),
                &label,
                [self.width / 2. - 156. + index as f32 * 106., 52., 100., 24.],
                NodeAction::StatisticsGroup(group),
                true,
                None,
            )?;
        }
        if !self.statistics.received() {
            let waiting = self.text.language.plain(
                &json!({"translate":"multiplayer.downloadingStats","fallback":"Downloading statistics..."}),
            )?;
            self.label(
                "statistics/waiting",
                waiting,
                [24., self.height / 2. - 12., self.width - 48., 24.],
            )?;
        } else {
            let rows = self.statistics.rows(content, self.statistics_group)?;
            let visible_rows = ((self.height - 132.) / 24.).max(0.) as usize;
            self.scroll = self
                .scroll
                .min(rows.len().saturating_sub(visible_rows.max(1)));
            for (index, row) in rows
                .into_iter()
                .skip(self.scroll)
                .take(visible_rows)
                .enumerate()
            {
                let label = format!(
                    "{}: {}",
                    self.text.language.plain(&row.component)?,
                    row.formatted,
                );
                self.label(
                    &format!("statistics/row/{}", row.id),
                    label,
                    [24., 88. + index as f32 * 24., self.width - 48., 20.],
                )?;
            }
        }
        let done = self
            .text
            .language
            .plain(&json!({"translate":"gui.done","fallback":"Done"}))?;
        self.button(
            "statistics/done",
            &done,
            [self.width / 2. - 100., self.height - 38., 200., 26.],
            NodeAction::Screen(Screen::Pause),
            true,
            None,
        )
    }
    pub(crate) fn panel(&mut self) {
        let _ = self.add(
            "screen/background".into(),
            "presentation",
            String::new(),
            [0., 0., self.width, self.height],
            true,
            None,
            NodeAction::None,
            if self.shell_screen_active() {
                Paint::Solid([0.055, 0.055, 0.065], 1.)
            } else {
                Paint::Solid([0.; 3], 0.65)
            },
        );
    }
    pub(crate) fn label(&mut self, id: &str, label: String, rect: [f32; 4]) -> Result<()> {
        self.add(
            id.into(),
            "text",
            label,
            rect,
            true,
            None,
            NodeAction::None,
            Paint::Text,
        )
    }
    pub(crate) fn button(
        &mut self,
        id: &str,
        label: &str,
        rect: [f32; 4],
        action: NodeAction,
        enabled: bool,
        deny: Option<&'static str>,
    ) -> Result<()> {
        self.add(
            id.into(),
            "button",
            label.into(),
            rect,
            enabled,
            deny,
            action,
            Paint::Button,
        )
    }
    pub(crate) fn edit_node(
        &mut self,
        id: &str,
        label: &str,
        rect: [f32; 4],
        enabled: bool,
    ) -> Result<()> {
        self.add(
            id.into(),
            "textbox",
            label.into(),
            rect,
            enabled,
            if enabled { None } else { Some("read_only") },
            if enabled {
                NodeAction::Focus(id.into())
            } else {
                NodeAction::None
            },
            Paint::Field(id.into()),
        )
    }
    pub(crate) fn sprite(&mut self, id: &str, name: &str, rect: [f32; 4]) -> Result<()> {
        self.add(
            id.into(),
            "image",
            String::new(),
            rect,
            true,
            None,
            NodeAction::None,
            Paint::Image(name.into(), [0., 0., 1., 1.]),
        )
    }
    fn menu_tree(&mut self, inventory: &InventoryStore, content: &Content, min_y: f32) -> Result<()> {
        if self.screen != Screen::Creative {
            self.panel();
        }
        let menu = inventory.menu();
        let layout = Layout::menu(menu)?;
        let has_recipe = menu.kind != MenuKind::Inventory && menu.kind.recipe_book().is_some();
        let compact_recipe = has_recipe && self.recipe_open && self.width < layout.width + 180.;
        let x = if has_recipe && self.recipe_open && !compact_recipe {
            (self.width - layout.width - 180.) / 2. + 180.
        } else {
            (self.width - layout.width) / 2.
        };
        let y = ((self.height - layout.height) / 2.).max(min_y);
        if compact_recipe {
            self.button(
                "recipe/book", "Recipes", [self.width - 54., 8., 50., 20.],
                NodeAction::RecipeBook, true, None,
            )?;
            self.recipe_panel(inventory, content, 30., y)?;
            return Ok(());
        }
        let name = format!("gui/container/{}", menu.kind.name());
        // Themed frame around the (warm-tinted) container texture.
        self.add(
            "menu/frame".into(),
            "presentation",
            String::new(),
            [x - 4., y - 4., layout.width + 8., layout.height + 8.],
            true,
            None,
            NodeAction::None,
            Paint::Frame,
        )?;
        if let MenuKind::Chest(rows) = menu.kind {
            let top = 17. + f32::from(rows) * 18.;
            self.add(
                "menu/background/top".into(),
                "image",
                String::new(),
                [x, y, layout.width, top],
                true,
                None,
                NodeAction::None,
                Paint::Image(name.clone(), [0., 0., 176., top]),
            )?;
            self.add(
                "menu/background/bottom".into(),
                "image",
                String::new(),
                [x, y + top, layout.width, 96.],
                true,
                None,
                NodeAction::None,
                Paint::Image(name, [0., 126., 176., 222.]),
            )?;
        } else {
            self.add(
                "menu/background".into(),
                "image",
                String::new(),
                [x, y, layout.width, layout.height],
                true,
                None,
                NodeAction::None,
                Paint::Image(name, [0., 0., layout.width, layout.height]),
            )?;
        }
        let title = if menu.kind == MenuKind::Inventory {
            self.text
                .language
                .plain(&serde_json::json!({"translate":"container.crafting"}))?
        } else {
            self.text.language.plain(&menu.title)?
        };
        // InventoryScreen.extractLabels draws only its Crafting title at x97.
        // Other containers retain AbstractContainerScreen's inventory caption.
        let title_x = if menu.kind == MenuKind::Inventory {
            97.
        } else {
            8.
        };
        self.label("menu/title", title, [x + title_x, y + 6., 72., 10.])?;
        if menu.kind != MenuKind::Inventory {
            self.label(
                "menu/inventory_label",
                self.text.language.translate("container.inventory").into(),
                [
                    x + layout.inventory_label[0],
                    y + layout.inventory_label[1],
                    88.,
                    9.,
                ],
            )?;
        }
        if menu.kind == MenuKind::Inventory {
            // Vanilla player recess: inventory.png black area x25-74/y7-77.
            // Parent-owned state.rs renders the real player rig here
            // (Paint::PlayerPreview, mouse-follow rotation); this tree emits
            // no placeholder, only the bounds/id contract below.
            self.add(
                "menu/player_preview".into(),
                "image",
                String::new(),
                [x + 26., y + 8., 51., 70.],
                true,
                None,
                NodeAction::None,
                Paint::PlayerPreview,
            )?;
        }
        for slot in &layout.slots {
            let known = menu.slots.get(slot.index).is_some_and(Option::is_some);
            self.add(
                format!("slot/{}", slot.index),
                "slot",
                String::new(),
                [x + slot.x - 1., y + slot.y - 1., 18., 18.],
                known,
                if known {
                    None
                } else {
                    Some("slot_not_observed")
                },
                NodeAction::Slot(slot.index as i16),
                Paint::Slot(slot.index),
            )?;
        }
        self.ghost_tree(inventory, content, &layout, x, y)?;
        if menu.kind == MenuKind::Anvil {
            if !self.fields.contains_key("anvil/name") {
                let name = menu
                    .slots
                    .first()
                    .and_then(Option::as_ref)
                    .filter(|stack| !stack.empty())
                    .map(|stack| {
                        self.text
                            .language
                            .plain(super::text::item_name_component(content, stack)?.as_ref())
                    })
                    .transpose()?
                    .unwrap_or_default();
                self.field("anvil/name", name, 200, false)?;
            }
            self.edit_node(
                "anvil/name",
                "Item name",
                [x + 59., y + 20., 103., 16.],
                true,
            )?;
        }
        // The player inventory keeps the vanilla frame clean until its recipe
        // book presentation is complete; typed recipe actions remain available.
        if has_recipe {
            self.button(
                "recipe/book",
                "Recipes",
                [x + layout.width - 50., y - 24., 50., 20.],
                NodeAction::RecipeBook,
                true,
                None,
            )?;
            if self.recipe_open {
                self.recipe_panel(inventory, content, x - 150., y)?;
            }
        }
        self.menu_controls(inventory, content, x, y)?;
        Ok(())
    }
    fn recipe_panel(&mut self, inventory: &InventoryStore, content: &Content, x: f32, y: f32) -> Result<()> {
        if !self.fields.contains_key("recipe/search") {
            self.field("recipe/search", String::new(), 256, false)?;
        }
        self.edit_node("recipe/search", "Search recipes", [x, y, 140., 20.], true)?;
        self.recipe_tree(inventory, content, x, y + 24.)
    }
    fn menu_controls(
        &mut self,
        inventory: &InventoryStore,
        content: &Content,
        x: f32,
        y: f32,
    ) -> Result<()> {
        let menu = inventory.menu();
        match menu.kind {
            MenuKind::Crafter => {
                let layout = Layout::menu(menu)?;
                for i in 0..9 {
                    let slot = &layout.slots[i];
                    let disabled = menu.data.get(&(i as i16)).copied();
                    let enabled = disabled.is_some()
                        && inventory.game_mode != 3
                        && menu.slots[i].as_ref().is_some_and(|s| s.empty());
                    self.button(
                        &format!("crafter/toggle/{i}"),
                        if disabled == Some(1) {
                            "Enable slot"
                        } else {
                            "Disable slot"
                        },
                        [x + slot.x - 1., y + slot.y - 1., 18., 18.],
                        NodeAction::SlotToggle(i as i16),
                        enabled,
                        (!enabled).then_some("crafter_slot_toggle_unavailable"),
                    )?;
                }
            }
            MenuKind::Beacon => {
                let levels = menu.data.get(&0).copied();
                for (index, (name, tier)) in [
                    ("minecraft:speed", 1),
                    ("minecraft:haste", 1),
                    ("minecraft:resistance", 2),
                    ("minecraft:jump_boost", 2),
                    ("minecraft:strength", 3),
                ]
                .into_iter()
                .enumerate()
                {
                    let effect = content
                        .registry_entries("minecraft:mob_effect")?
                        .iter()
                        .position(|(value, _)| value == name)
                        .ok_or("beacon_effect_not_observed")?
                        as i32;
                    let enabled = levels.is_some_and(|n| n >= tier);
                    let label = self
                        .text
                        .language
                        .translate(&format!("effect.{}", name.replace(':', ".")))
                        .to_owned();
                    self.button(
                        &format!("beacon/primary/{effect}"),
                        &label,
                        [
                            x + 10. + (index % 2) as f32 * 72.,
                            y + 22. + (index / 2) as f32 * 22.,
                            70.,
                            20.,
                        ],
                        NodeAction::BeaconEffect {
                            primary: true,
                            effect,
                        },
                        enabled,
                        (!enabled).then_some("beacon_level_unavailable"),
                    )?;
                }
                let regeneration = content
                    .registry_entries("minecraft:mob_effect")?
                    .iter()
                    .position(|(name, _)| name == "minecraft:regeneration")
                    .ok_or("beacon_effect_not_observed")? as i32;
                let mut secondary = vec![regeneration];
                if let Some(primary) = self.beacon_selection[0] {
                    secondary.push(primary);
                }
                for (index, effect) in secondary.into_iter().enumerate() {
                    let enabled = levels.is_some_and(|n| n >= 4);
                    self.button(
                        &format!("beacon/secondary/{effect}"),
                        if effect == regeneration {
                            "Regeneration"
                        } else {
                            "Primary power II"
                        },
                        [x + 168., y + 22. + index as f32 * 22., 108., 20.],
                        NodeAction::BeaconEffect {
                            primary: false,
                            effect,
                        },
                        enabled,
                        (!enabled).then_some("beacon_level_unavailable"),
                    )?;
                }
                let enabled = self.beacon_selection[0].is_some()
                    && menu.slots[0].as_ref().is_some_and(|s| !s.empty())
                    && levels.is_some_and(|n| n > 0);
                self.button(
                    "beacon/confirm",
                    "Done",
                    [x + 164., y + 107., 48., 20.],
                    NodeAction::BeaconSubmit,
                    enabled,
                    (!enabled).then_some("beacon_payment_or_power_unavailable"),
                )?;
            }
            MenuKind::Merchant => self.trade_tree(inventory, x, y)?,
            MenuKind::Enchantment | MenuKind::Lectern | MenuKind::Loom | MenuKind::Stonecutter => {
                // Availability is shared with the ordinary inventory executor, not inferred from presentation.
                let buttons = if menu.kind == MenuKind::Loom && !menu.known() {
                    Vec::new()
                } else {
                    inventory.menu_buttons(content)?
                };
                let patterns = if menu.kind == MenuKind::Loom && menu.known() {
                    inventory.loom_patterns(content)?
                } else {
                    Vec::new()
                };
                let stone: Vec<_> = inventory
                    .stonecutter_recipes
                    .iter()
                    .filter(|r| {
                        menu.slots[0]
                            .as_ref()
                            .is_some_and(|s| r.ingredient_items.contains(&s.id))
                    })
                    .collect();
                let skip = if matches!(menu.kind, MenuKind::Loom | MenuKind::Stonecutter) {
                    self.scroll * 4
                } else {
                    0
                };
                for (index, button) in buttons.into_iter().enumerate().skip(skip).take(12) {
                    let (label, bounds, paint) = match menu.kind {
                        MenuKind::Enchantment => (
                            format!(
                                "Enchant: {} levels",
                                menu.data
                                    .get(&(index as i16))
                                    .map(i32::to_string)
                                    .unwrap_or_else(|| "not observed".into())
                            ),
                            [x + 60., y + 14. + index as f32 * 19., 108., 18.],
                            Paint::Button,
                        ),
                        MenuKind::Lectern => (
                            ["Previous", "Next", "Take book"][index].into(),
                            [x + 12. + index as f32 * 56., y + 160., 52., 20.],
                            Paint::Button,
                        ),
                        MenuKind::Loom => (
                            patterns
                                .get(index)
                                .cloned()
                                .ok_or("loom_pattern_not_observed")?,
                            [
                                x + 60. + ((index - skip) % 4) as f32 * 14.,
                                y + 13. + ((index - skip) / 4) as f32 * 14.,
                                14.,
                                14.,
                            ],
                            Paint::Button,
                        ),
                        MenuKind::Stonecutter => {
                            let recipe =
                                stone.get(index).ok_or("stonecutter_recipe_not_observed")?;
                            let variants = super::display::display_variants(
                                &recipe.display,
                                "result",
                                &recipe.display_stacks,
                                content,
                            )?;
                            (
                                String::new(),
                                [
                                    x + 52. + ((index - skip) % 4) as f32 * 16.,
                                    y + 14. + ((index - skip) / 4) as f32 * 18.,
                                    16.,
                                    18.,
                                ],
                                Paint::Variants(std::sync::Arc::new(variants), false),
                            )
                        }
                        _ => unreachable!(),
                    };
                    self.add(
                        format!("menu/button/{}", button.id),
                        "button",
                        label,
                        bounds,
                        button.enabled,
                        button.deny_reason,
                        NodeAction::MenuButton(button.id),
                        paint,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn creative_tree(&mut self, inventory: &InventoryStore, content: &Content) -> Result<()> {
        self.panel();
        let tabs = self.creative_source["tabs"]
            .as_array()
            .ok_or("creative_tabs_required")?;
        let selected = tabs
            .get(self.creative_tab)
            .ok_or("creative_tab_not_observed")?;
        let inventory_tab = selected["type"] == "INVENTORY";
        let search_tab = selected["type"] == "SEARCH";
        let hotbar_tab = selected["type"] == "HOTBAR";
        let descriptors: Vec<_> = tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab["id"] != "minecraft:op_blocks" || self.operator_permission)
            .map(|(index, tab)| {
                (
                    index,
                    tab["id"].as_str().unwrap_or("").to_owned(),
                    tab["title_component"].clone(),
                )
            })
            .collect();
        let columns = 7.max(descriptors.len().div_ceil(2));
        let tab_width = (self.width - 16.) / columns as f32;
        let tab_rows = descriptors.len().div_ceil(columns);
        for (position, (index, id, title)) in descriptors.into_iter().enumerate() {
            self.button(
                &format!("creative/tab/{id}"),
                &self.text.language.plain(&title)?,
                [
                    8. + (position % columns) as f32 * tab_width,
                    8. + (position / columns) as f32 * 22.,
                    tab_width - 2.,
                    20.,
                ],
                NodeAction::CreativeTab(index),
                true,
                None,
            )?;
        }
        if inventory_tab {
            self.menu_tree(inventory, content, 12. + tab_rows as f32 * 22.)?;
            self.button(
                "creative/trash",
                "Destroy item",
                [self.width - 106., self.height - 28., 98., 20.],
                NodeAction::CreativeClear,
                true,
                None,
            )?;
            return Ok(());
        }
        if !self.fields.contains_key("creative/search") {
            self.field("creative/search", String::new(), 256, false)?;
        }
        if search_tab {
            self.edit_node(
                "creative/search",
                "Search items",
                [16., 56., self.width - 32., 20.],
                true,
            )?;
        }
        let query = if search_tab {
            self.fields["creative/search"].text.to_lowercase()
        } else {
            String::new()
        };
        let columns = ((self.width - 32.) / 20.).max(1.) as usize;
        let rows = ((self.height - 126.) / 20.).max(0.) as usize;
        self.creative_preset_tree(content, &query, columns, rows, hotbar_tab)?;
        // Creative palette and player hotbar have separate interaction policies.
        for i in 0..9 {
            let known = inventory
                .player
                .slots
                .get(36 + i)
                .is_some_and(Option::is_some);
            self.add(
                format!("slot/{}", 36 + i),
                "slot",
                String::new(),
                [
                    self.width / 2. - 90. + i as f32 * 20.,
                    self.height - 24.,
                    18.,
                    18.,
                ],
                known,
                (!known).then_some("slot_not_observed"),
                NodeAction::Slot((36 + i) as i16),
                Paint::Slot(36 + i),
            )?;
        }
        Ok(())
    }
    fn editor_tree(&mut self) -> Result<()> {
        self.panel();
        let x = (self.width - 300.) / 2.;
        match self.screen {
            Screen::Sign => {
                for i in 0..4 {
                    self.edit_node(
                        &format!("sign/line/{i}"),
                        &format!("Sign line {}", i + 1),
                        [x, 60. + i as f32 * 24., 300., 20.],
                        true,
                    )?;
                }
            }
            Screen::Book => {
                let writable =
                    matches!(self.editor, Some(EditorState::Book { writable: true, .. }));
                self.add(
                    "book/background".into(),
                    "image",
                    String::new(),
                    [self.width / 2. - 96., 20., 192., 192.],
                    true,
                    None,
                    NodeAction::None,
                    Paint::Image("gui/book".into(), [0., 0., 192., 192.]),
                )?;
                if let Some(page) = self.book_components.get(self.page).cloned() {
                    self.rich_label(
                        &format!("book/page/{}", self.page),
                        page,
                        [self.width / 2. - 58., 50., 116., 128.],
                        true,
                    )?;
                } else {
                    self.edit_node(
                        &format!("book/page/{}", self.page),
                        "Book page",
                        [self.width / 2. - 58., 50., 116., 128.],
                        writable,
                    )?;
                }
                self.label(
                    "book/page_number",
                    format!("Page {}", self.page + 1),
                    [self.width / 2. - 58., 34., 116., 10.],
                )?;
                self.button(
                    "book/previous",
                    "<",
                    [self.width / 2. - 58., 184., 20., 20.],
                    NodeAction::Page(-1),
                    self.page > 0,
                    if self.page > 0 {
                        None
                    } else {
                        Some("first_page")
                    },
                )?;
                self.button(
                    "book/next",
                    ">",
                    [self.width / 2. + 38., 184., 20., 20.],
                    NodeAction::Page(1),
                    writable && self.page < 99
                        || self
                            .fields
                            .contains_key(&format!("book/page/{}", self.page + 1)),
                    if writable && self.page < 99
                        || self
                            .fields
                            .contains_key(&format!("book/page/{}", self.page + 1))
                    {
                        None
                    } else {
                        Some("last_page")
                    },
                )?;
                if writable {
                    self.edit_node("book/title", "Book title", [x, 220., 300., 20.], true)?;
                }
            }
            Screen::Command | Screen::Structure | Screen::Jigsaw => {
                if !self.operator_permission {
                    return Err("permission_denied".into());
                }
                let prefix = match self.screen {
                    Screen::Command => "command/",
                    Screen::Structure => "structure/",
                    _ => "jigsaw/",
                };
                let ids: Vec<_> = self
                    .fields
                    .keys()
                    .filter(|id| id.starts_with(prefix))
                    .filter(|id| match &self.editor {
                        Some(EditorState::Structure(s)) => {
                            super::operator::field_visible(s.mode, id)
                        }
                        _ => true,
                    })
                    .cloned()
                    .collect();
                for (i, id) in ids.into_iter().enumerate() {
                    let row = (i / 2) as f32;
                    let col = (i % 2) as f32;
                    self.label(
                        &format!("{id}/label"),
                        id[prefix.len()..].into(),
                        [x + col * 154., 28. + row * 40., 146., 10.],
                    )?;
                    self.edit_node(
                        &id,
                        "Block value",
                        [x + col * 154., 40. + row * 40., 146., 20.],
                        true,
                    )?;
                }
                let controls: Vec<(&str, String)> = match self.editor.as_ref() {
                    Some(EditorState::Command(c)) => vec![
                        ("mode", format!("Mode {}", c.mode)),
                        ("track_output", format!("Track output {}", c.track_output)),
                        ("conditional", format!("Conditional {}", c.conditional)),
                        ("automatic", format!("Automatic {}", c.automatic)),
                    ],
                    Some(EditorState::Structure(s)) => vec![
                        ("mode", format!("Mode {}", s.mode)),
                        ("mirror", format!("Mirror {}", s.mirror)),
                        ("rotation", format!("Rotation {}", s.rotation)),
                        (
                            "ignore_entities",
                            format!("Ignore entities {}", s.ignore_entities),
                        ),
                        ("strict", format!("Strict {}", s.strict)),
                        ("show_air", format!("Show air {}", s.show_air)),
                        ("show_box", format!("Show bounding box {}", s.show_box)),
                        ("save", "Save".into()),
                        ("load", "Load".into()),
                        ("detect", "Detect size".into()),
                    ],
                    Some(EditorState::Jigsaw(j)) => vec![("joint", format!("Joint {}", j.joint))],
                    _ => return Err("missing_editor_state".into()),
                };
                let controls = controls
                    .into_iter()
                    .filter(|(key, _)| match &self.editor {
                        Some(EditorState::Structure(s)) => {
                            super::operator::control_visible(s.mode, key)
                        }
                        Some(EditorState::Command(c)) if c.entity_id.is_some() => {
                            *key == "track_output"
                        }
                        _ => true,
                    })
                    .collect::<Vec<_>>();
                for (i, (key, label)) in controls.into_iter().enumerate() {
                    self.button(
                        &format!("editor/control/{key}"),
                        &label,
                        [
                            x + (i % 3) as f32 * 102.,
                            self.height - 120. + (i / 3) as f32 * 22.,
                            98.,
                            20.,
                        ],
                        NodeAction::EditorToggle(key),
                        true,
                        None,
                    )?;
                }
                if self.screen == Screen::Jigsaw {
                    self.button(
                        "jigsaw/keep",
                        "Keep jigsaws",
                        [x + 154., self.height - 58., 146., 20.],
                        NodeAction::Setting("keep_jigsaws"),
                        true,
                        None,
                    )?;
                    self.button(
                        "jigsaw/generate",
                        "Generate",
                        [x, self.height - 58., 146., 20.],
                        NodeAction::Generate,
                        true,
                        None,
                    )?;
                }
            }
            _ => {}
        }
        let editable = !matches!(
            self.editor,
            Some(EditorState::Book {
                writable: false,
                ..
            })
        );
        if editable {
            self.button(
                "editor/submit",
                "Done",
                [x, self.height - 30., 146., 20.],
                NodeAction::Submit,
                true,
                None,
            )?;
        }
        self.button(
            "editor/cancel",
            if editable { "Cancel" } else { "Done" },
            [x + 154., self.height - 30., 146., 20.],
            NodeAction::Close,
            true,
            None,
        )?;
        Ok(())
    }
    /// Vanilla HUD geometry in logical UI pixels (vanilla `Gui` sprite grid):
    /// hotbar 182x22, hearts/armor/food/air 9px on an 8px pitch, crosshair
    /// 15px. The window scale factor (parent-owned `state.rs` resize, Auto at
    /// 1280x720) magnifies these to physical pixels; logical sizes here must
    /// stay vanilla so Auto scale does not balloon the strip.
    /// `ui_hotfix_tests::hud_minimum_sizes` pins these.
    pub(crate) const HUD_HOTBAR: [f32; 2] = [182., 22.];
    pub(crate) const HUD_HEART: f32 = 9.;
    pub(crate) const HUD_CROSSHAIR: f32 = 15.;
    pub(crate) const HUD_BUTTON_MIN: [f32; 2] = [200., 20.];
    /// Fail-closed clickability gate shared by tests and the windowed proof driver:
    /// a dialog button must be enabled, fully inside the viewport, and at least
    /// 200x20 logical px (vanilla-scale) to count as playable at 1280x720.
    pub(crate) fn dialog_button_clickable(&self, id: &str) -> bool {
        self.nodes.iter().any(|n| {
            n.id == id
                && n.role == "button"
                && n.enabled
                && n.visible
                && n.bounds[2] >= Self::HUD_BUTTON_MIN[0]
                && n.bounds[3] >= Self::HUD_BUTTON_MIN[1]
                && n.bounds[0] >= 0.
                && n.bounds[1] >= 0.
                && n.bounds[0] + n.bounds[2] <= self.width
                && n.bounds[1] + n.bounds[3] <= self.height
        })
    }
    fn hud_tree(&mut self, inventory: &InventoryStore, content: &Content) -> Result<()> {
        // F3 overlay (vanilla): renders on top even when HUD is hidden.
        if !self.debug_lines.is_empty() {
            let lines: Vec<String> = self.debug_lines.iter().take(16).cloned().collect();
            let height = lines.len() as f32 * 12. + 8.;
            self.add(
                "debug/panel".into(),
                "presentation",
                String::new(),
                [4., 4., 340., height],
                true,
                None,
                NodeAction::None,
                Paint::Solid([0.; 3], 0.5),
            )?;
            for (i, line) in lines.into_iter().enumerate() {
                self.label(
                    &format!("debug/line/{i}"),
                    line,
                    [8., 8. + i as f32 * 12., 332., 12.],
                )?;
            }
        }
        if self.settings.get("hide_hud").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let cx = self.width / 2.;
        self.hud_accessibility_tree(inventory, content)?;
        self.sidebar_tree()?;
        // Vanilla hotbar strip: 182x22 logical px; the window scale factor
        // (not this tree) makes it readable at 1280x720.
        let bottom = self.height - 22.;
        self.add(
            "hud/hotbar".into(),
            "image",
            String::new(),
            [cx - 91., bottom, 182., 22.],
            true,
            None,
            NodeAction::None,
            Paint::Hotbar,
        )?;
        self.add(
            "hud/selection".into(),
            "image",
            String::new(),
            [
                cx - 92. + f32::from(inventory.selected_slot) * 20.,
                bottom - 1.,
                24.,
                23.,
            ],
            true,
            None,
            NodeAction::None,
            Paint::HotbarSelection,
        )?;
        for i in 0..9 {
            // 18x18 logical cells on the vanilla 20px pitch: parent-owned
            // state.rs insets 1px to 16px icons so the 17px count decoration
            // (x+17-width, y+9 + explicit shadow) never clips.
            self.add(
                format!("hud/slot/{i}"),
                "slot",
                String::new(),
                [cx - 89. + i as f32 * 20., bottom + 2., 18., 18.],
                true,
                None,
                NodeAction::None,
                Paint::Slot(36 + i),
            )?;
        }
        // Vanilla crosshair: 15px logical.
        self.sprite(
            "hud/crosshair",
            "gui/sprites/hud/crosshair",
            [cx - 7.5, self.height / 2. - 7.5, 15., 15.],
        )?;
        // Attack-strength indicator next to the crosshair: vanilla draws the
        // hotbar/crosshair attack marker only while it recharges.
        if let Some(attack) = self.hud.attack {
            let attack = attack.clamp(0., 1.);
            if attack < 1. {
                let width = 15. * attack;
                if width > 0. {
                    self.add(
                        "hud/attack".into(),
                        "progressbar",
                        String::new(),
                        [cx - 7.5, self.height / 2. + 8.5, width, 2.],
                        true,
                        None,
                        NodeAction::None,
                        Paint::Solid([1., 1., 1.], 1.),
                    )?;
                }
            }
        }
        // Held-item name above the hotbar, shown briefly after each selection
        // change (vanilla Gui holds the selected item name on screen; the HUD
        // keeps it for 100 ticks here so state-only tests can observe it).
        if self.tick.saturating_sub(self.selected_name_tick) < 100 {
            let slot = 36 + usize::from(inventory.selected_slot);
            if let Some(stack) = inventory.player.slots.get(slot).and_then(Option::as_ref) {
                if !stack.empty() {
                    let name = self
                        .text
                        .language
                        .plain(super::text::item_name_component(content, stack)?.as_ref())?;
                    if !name.is_empty() {
                        self.label(
                            "hud/selected_name",
                            name,
                            [cx - 91., bottom - 25., 182., 10.],
                        )?;
                    }
                }
            }
        }
        let health = self.hud.health;
        let max_health = self.hud.max_health;
        let food = self.hud.food;
        let armor = self.hud.armor;
        let air = self.hud.air;
        let max_air = self.hud.max_air;
        let riding = self.hud.riding_health;
        let riding_max = self.hud.riding_max_health;
        if let (Some(health), Some(max)) = (health, max_health) {
            let hearts = ((max + self.hud.absorption) / 2.).ceil().clamp(0., 100.) as usize;
            // 9px hearts on an 8px pitch, parked above the 22px hotbar.
            for i in 0..hearts.min(60) {
                let x = cx - 91. + (i % 10) as f32 * 8.;
                let y = bottom - 17. - (i / 10) as f32 * 10.;
                self.sprite(
                    &format!("hud/heart/{i}/container"),
                    if self.hud.hardcore {
                        "gui/sprites/hud/heart/container_hardcore"
                    } else {
                        "gui/sprites/hud/heart/container"
                    },
                    [x, y, 9., 9.],
                )?;
                let amount = health - i as f32 * 2.;
                if amount > 0. {
                    let kind = if self.hud.heart_kind.is_empty() {
                        String::new()
                    } else {
                        format!("{}_", self.hud.heart_kind)
                    };
                    let hardcore = if self.hud.hardcore { "hardcore_" } else { "" };
                    let sprite = format!(
                        "gui/sprites/hud/heart/{kind}{hardcore}{}",
                        if amount >= 2. { "full" } else { "half" }
                    );
                    self.sprite(&format!("hud/heart/{i}/value"), &sprite, [x, y, 9., 9.])?;
                }
            }
        }
        for (kind, value, max, right, y) in [
            (
                "food",
                food.map(|x| x as f32),
                Some(20.),
                true,
                bottom - 17.,
            ),
            (
                "armor",
                armor.map(|x| x as f32),
                Some(20.),
                false,
                bottom - 27.,
            ),
            (
                "air",
                air.map(|x| x as f32),
                max_air.map(|x| x as f32),
                true,
                bottom - 27.,
            ),
            ("vehicle", riding, riding_max, true, bottom - 17.),
        ] {
            if let (Some(value), Some(max)) = (value, max) {
                if max <= 0. {
                    continue;
                }
                // Vanilla hides the armor row at 0 and the air row when full.
                if kind == "air" && value >= max {
                    continue;
                }
                if kind == "armor" && value <= 0. {
                    continue;
                }
                let count = if kind == "vehicle" {
                    (max / 2.).ceil().min(30.) as usize
                } else {
                    10
                };
                for i in 0..count {
                    let x = if right {
                        cx + 82. - (i % 10) as f32 * 8.
                    } else {
                        cx - 91. + (i % 10) as f32 * 8.
                    };
                    let y = y - (i / 10) as f32 * 10.;
                    let amount = if kind == "air" {
                        value / max * 20. - i as f32 * 2.
                    } else {
                        value - i as f32 * 2.
                    };
                    let sprite = if kind == "vehicle" {
                        format!(
                            "gui/sprites/hud/heart/vehicle_{}",
                            if amount >= 2. {
                                "full"
                            } else if amount > 0. {
                                "half"
                            } else {
                                "container"
                            }
                        )
                    } else if kind == "air" {
                        format!(
                            "gui/sprites/hud/{}",
                            if amount > 0. { "air" } else { "air_empty" }
                        )
                    } else {
                        format!(
                            "gui/sprites/hud/{kind}_{}{}",
                            if amount >= 2. {
                                "full"
                            } else if amount > 0. {
                                "half"
                            } else {
                                "empty"
                            },
                            if kind == "food" && self.hud.hunger {
                                "_hunger"
                            } else {
                                ""
                            }
                        )
                    };
                    self.sprite(&format!("hud/{kind}/{i}"), &sprite, [x, y, 9., 9.])?;
                }
            }
        }
        self.contextual_tree(inventory, content)?;
        self.experience_level_tree(inventory)?;
        let effects: Vec<_> = self
            .hud
            .effects
            .iter()
            .filter(|e| e.visible)
            .take(32)
            .map(|e| (e.id.clone(), e.amplifier, e.ticks))
            .collect();
        for (i, (id, amp, ticks)) in effects.into_iter().enumerate() {
            let key = id.strip_prefix("minecraft:").unwrap_or(&id);
            self.sprite(
                &format!("hud/effect/{id}"),
                &format!("mob_effect/{key}"),
                [
                    self.width - 28. - (i % 8) as f32 * 26.,
                    4. + (i / 8) as f32 * 26.,
                    24.,
                    24.,
                ],
            )?;
            self.label(
                &format!("hud/effect/{id}/label"),
                format!(
                    "{} {} {}",
                    self.text
                        .language
                        .translate(&format!("effect.minecraft.{key}")),
                    amp + 1,
                    ticks / 20
                ),
                [self.width - 220., 110. + i as f32 * 10., 216., 10.],
            )?;
        }
        let chat: Vec<_> = self
            .chat
            .iter()
            .rev()
            .skip(if self.screen == Screen::Chat {
                self.scroll
            } else {
                0
            })
            .filter(|line| self.screen == Screen::Chat || self.tick.saturating_sub(line.tick) < 200)
            .take(10)
            .map(|line| (line.sequence, line.component.clone()))
            .collect();
        for (i, (seq, text)) in chat.into_iter().enumerate() {
            self.rich_label(
                &format!("chat/line/{seq}"),
                std::sync::Arc::new(text),
                [
                    4.,
                    self.height - 50. - i as f32 * 18.,
                    (self.width - 16.).min(320.),
                    18.,
                ],
                self.screen == Screen::Chat,
            )?;
        }
        if self.hud.player_list_visible {
            let names: Vec<_> = self
                .hud
                .players
                .iter()
                .take(80)
                .map(|p| (p.id.clone(), p.name.clone(), p.latency))
                .collect();
            for (i, (id, name, latency)) in names.into_iter().enumerate() {
                self.label(
                    &format!("tab/player/{id}"),
                    format!("{} {latency}ms", self.text.language.plain(&name)?),
                    [
                        cx - 180. + (i / 20) as f32 * 90.,
                        30. + (i % 20) as f32 * 10.,
                        88.,
                        10.,
                    ],
                )?;
            }
        }
        let bosses: Vec<_> = self
            .bosses
            .iter()
            .take(16)
            .map(|(id, b)| (id.clone(), b.name.clone(), b.progress))
            .collect();
        for (i, (id, name, progress)) in bosses.into_iter().enumerate() {
            self.label(
                &format!("boss/{id}/name"),
                self.text.language.plain(&name)?,
                [cx - 91., 12. + i as f32 * 22., 182., 10.],
            )?;
            self.add(
                format!("boss/{id}/progress"),
                "progressbar",
                String::new(),
                [
                    cx - 91.,
                    22. + i as f32 * 22.,
                    182. * progress.clamp(0., 1.),
                    5.,
                ],
                true,
                None,
                NodeAction::None,
                Paint::Solid([0.8, 0., 0.8], 1.),
            )?;
        }
        if self.tick.saturating_sub(self.title_started)
            < self.title_timing.iter().map(|n| *n as u64).sum()
        {
            if let Some(title) = self.title.clone() {
                self.label(
                    "hud/title",
                    self.text.language.plain(&title)?,
                    [cx - 150., self.height / 2. - 30., 300., 30.],
                )?;
            }
            if let Some(subtitle) = self.subtitle.clone() {
                self.label(
                    "hud/subtitle",
                    self.text.language.plain(&subtitle)?,
                    [cx - 150., self.height / 2., 300., 20.],
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod item8_layout_tests {
    use super::*;
    use std::sync::Arc;

    fn state() -> (Content, InventoryStore, UiState) {
        let mut content = Content::test_cube();
        let tabs: Vec<_> = (0..12)
            .map(|i| json!({
                "id": format!("minecraft:tab_{i}"),
                "title_component": {"text": format!("Tab {i}")},
                "type": if i == 10 { "SEARCH" } else if i == 11 { "INVENTORY" } else { "CATEGORY" },
                "items": []
            }))
            .collect();
        content.creative_tabs_report = Arc::from(json!({"tabs": tabs}).to_string().into_bytes());
        let mut ui = UiState::new_state(&content).unwrap();
        ui.resize(1280, 720, 1.);
        (content, InventoryStore::default(), ui)
    }

    fn node<'a>(ui: &'a UiState, id: &str) -> &'a super::super::state::SemanticNode {
        ui.nodes.iter().find(|node| node.id == id).unwrap()
    }

    fn inside(ui: &UiState, id: &str) {
        let [x, y, w, h] = node(ui, id).bounds;
        assert!(x >= 0. && y >= 0. && x + w <= ui.width && y + h <= ui.height,
            "{id} outside {}x{}: {:?}", ui.width, ui.height, [x, y, w, h]);
    }
    fn reachable(ui: &UiState, id: &str) {
        inside(ui, id);
        let target = node(ui, id);
        let [x, y, w, h] = target.bounds;
        let point = [x + w / 2., y + h / 2.];
        let hit = ui.nodes.iter().rev().find(|n| {
            n.visible && !matches!(n.action, NodeAction::None)
                && super::super::state::contains(n.bounds, point)
        }).unwrap();
        assert!(target.enabled && hit.id == id, "{id} is covered by {}", hit.id);
    }


    fn disjoint(a: [f32; 4], b: [f32; 4]) -> bool {
        a[0] + a[2] <= b[0] || b[0] + b[2] <= a[0]
            || a[1] + a[3] <= b[1] || b[1] + b[3] <= a[1]
    }

    #[test]
    fn creative_tabs_search_and_inventory_are_reachable_at_default_scale() {
        let (content, inventory, mut ui) = state();
        ui.screen = Screen::Creative;
        ui.creative_tab = 10;
        ui.rebuild(&inventory, &content).unwrap();
        for i in 0..12 {
            let id = format!("creative/tab/minecraft:tab_{i}");
            reachable(&ui, &id);
            assert!(matches!(&node(&ui, &id).action, NodeAction::CreativeTab(index) if *index == i));
            assert!(disjoint(node(&ui, &id).bounds, node(&ui, "creative/search").bounds));
        }
        reachable(&ui, "creative/search");
        ui.creative_tab = 11;
        ui.rebuild(&inventory, &content).unwrap();
        reachable(&ui, "creative/trash");
        for i in 0..12 {
            let id = format!("creative/tab/minecraft:tab_{i}");
            reachable(&ui, &id);
            assert!(disjoint(node(&ui, &id).bounds, node(&ui, "menu/background").bounds));
        }
    }

    #[test]
    fn recipe_controls_fit_and_container_is_usable_at_wide_and_narrow_widths() {
        let (content, mut inventory, mut ui) = state();
        let mut menu = inventory.player.clone();
        menu.kind = MenuKind::Crafting;
        menu.slots.resize(menu.kind.slot_count(), None);
        menu.slots.fill(Some(crate::inventory::Stack::default()));
        inventory.active = Some(menu);
        for (id, category) in ["minecraft:crafting_building_blocks", "minecraft:crafting_equipment", "minecraft:crafting_misc"].into_iter().enumerate() {
            inventory.recipes.insert(id as i32, crate::inventory::Recipe {
                id: id as i32,
                display: Arc::new(json!({"result": {"type": "minecraft:empty"}})),
                group: None,
                category: category.into(),
                ingredients: None,
                flags: 0,
                display_stacks: Vec::new(),
                ingredient_items: None,
            });
        }
        ui.screen = Screen::Menu;
        for width in [427., 320.] {
            ui.width = width;
            ui.recipe_open = true;
            ui.rebuild(&inventory, &content).unwrap();
            for id in ["recipe/book", "recipe/search", "recipe/category/", "recipe/filter"] {
                reachable(&ui, id);
            }
            assert!(matches!(&node(&ui, "recipe/category/").action, NodeAction::RecipeCategory(s) if s.is_empty()));
            for category in ["minecraft:crafting_building_blocks", "minecraft:crafting_equipment", "minecraft:crafting_misc"] {
                let id = format!("recipe/category/{category}");
                reachable(&ui, &id);
                assert!(matches!(&node(&ui, &id).action, NodeAction::RecipeCategory(value) if value == category));
                assert!(disjoint(node(&ui, &id).bounds, node(&ui, "recipe/search").bounds));
            }
            if width == 427. {
                assert!(disjoint(node(&ui, "recipe/search").bounds, node(&ui, "menu/background").bounds));
                reachable(&ui, "slot/0");
            }
            ui.recipe_open = false;
            ui.rebuild(&inventory, &content).unwrap();
            reachable(&ui, "slot/0");
            inside(&ui, "recipe/book");
        }
    }
}
