//! Source HUD scoreboard presentation, cached independently from per-tick player state.
use super::{
    hud::ScoreboardState,
    render::TextRenderer,
    shaping::TextLayout,
    state::{NodeAction, Paint, UiState},
};
use crate::protocol::Result;
use std::sync::Arc;

pub(crate) struct SidebarText {
    label: String,
    layout: Arc<TextLayout>,
    component: Arc<serde_json::Value>,
    width: i32,
}
pub(crate) struct SidebarRow {
    name_id: String,
    score_id: String,
    name: SidebarText,
    score: SidebarText,
}
#[derive(Default)]
pub(crate) struct Sidebar {
    title: Option<SidebarText>,
    rows: Vec<SidebarRow>,
    width: i32,
}
impl SidebarText {
    fn new(text: &TextRenderer, component: Arc<serde_json::Value>) -> Result<Self> {
        let layout = Arc::new(text.layout(&component, f32::MAX)?);
        Ok(Self {
            label: text.language.plain(&component)?,
            width: layout.width.ceil() as i32,
            layout,
            component,
        })
    }
}
impl Sidebar {
    pub(crate) fn build(
        board: &ScoreboardState,
        holder: Option<&str>,
        text: &TextRenderer,
    ) -> Result<Self> {
        let Some((name, objective)) = board.sidebar_objective(holder) else {
            return Ok(Self::default());
        };
        let title = SidebarText::new(text, Arc::clone(&objective.display))?;
        let separator = SidebarText::new(text, Arc::new(serde_json::json!({"text":": "})))?.width;
        let mut scores: Vec<_> = board
            .scores
            .get(name)
            .into_iter()
            .flat_map(|scores| scores.iter())
            .filter(|(holder, _)| !holder.starts_with('#'))
            .collect();
        // Gui.SCORE_DISPLAY_ORDER: decreasing score then Java CASE_INSENSITIVE_ORDER.
        scores.sort_by(|(a, sa), (b, sb)| {
            sb.value
                .cmp(&sa.value)
                .then_with(|| super::hud_case::compare(a, b))
        });
        let mut width = title.width;
        let mut rows = Vec::with_capacity(scores.len().min(15));
        for (holder, score) in scores.into_iter().take(15) {
            let name =
                SidebarText::new(text, board.formatted_name(holder, score.display.as_ref()))?;
            let score = SidebarText::new(text, board.score_component(objective, score, true))?;
            width = width.max(
                name.width
                    + if score.width > 0 {
                        separator + score.width
                    } else {
                        0
                    },
            );
            rows.push(SidebarRow {
                name_id: format!("hud/sidebar/holder/{holder}/name"),
                score_id: format!("hud/sidebar/holder/{holder}/score"),
                name,
                score,
            });
        }
        Ok(Self {
            title: Some(title),
            rows,
            width,
        })
    }
}
impl UiState {
    pub(crate) fn compare_player_names(a: &str, b: &str) -> std::cmp::Ordering {
        super::hud_case::compare(a, b)
    }
    pub(crate) fn experience_level_tree(
        &mut self,
        inventory: &crate::inventory::InventoryStore,
    ) -> Result<()> {
        let Some(level) = self
            .hud
            .level
            .filter(|&level| level > 0 && matches!(inventory.game_mode, 0 | 2))
        else {
            return Ok(());
        };
        if self.hud_level.as_ref().map(|(cached, _)| *cached) != Some(level) {
            let component = Arc::new(serde_json::json!({
                "translate":"gui.experience.level",
                "with":[level],
            }));
            self.hud_level = Some((level, Arc::new(SidebarText::new(&self.text, component)?)));
        }
        let text = Arc::clone(&self.hud_level.as_ref().expect("experience text prepared").1);
        self.add(
            "hud/xp/level".into(),
            "text",
            text.label.clone(),
            [
                ((self.width as i32 - text.width) / 2) as f32,
                (self.height as i32 - 62) as f32,
                text.width as f32,
                18.,
            ],
            true,
            None,
            NodeAction::None,
            Paint::OutlinedText(Arc::clone(&text.layout), 0xff80ff20, 0xff000000),
        )?;
        self.nodes
            .last_mut()
            .expect("experience node added")
            .component = Some(Arc::clone(&text.component));
        Ok(())
    }
    pub(crate) fn sidebar_tree(&mut self) -> Result<()> {
        if self.hud_sidebar.is_none() {
            self.hud_sidebar = Some(Arc::new(Sidebar::build(
                &self.network_hud.scoreboard,
                self.hud.scoreboard_name.as_deref(),
                &self.text,
            )?));
        }
        let sidebar = Arc::clone(self.hud_sidebar.as_ref().expect("sidebar prepared"));
        let Some(title) = &sidebar.title else {
            return Ok(());
        };
        let count = sidebar.rows.len() as i32;
        let right = self.width as i32 - 1;
        let left = self.width as i32 - sidebar.width - 3;
        let bottom = self.height as i32 / 2 + count * 9 / 3;
        let top = bottom - count * 9;
        self.add(
            "hud/sidebar/title/background".into(),
            "presentation",
            String::new(),
            [
                (left - 2) as f32,
                (top - 10) as f32,
                (right - left + 2) as f32,
                9.,
            ],
            true,
            None,
            NodeAction::None,
            Paint::Solid([0.; 3], self.text_background_opacity(0.4)),
        )?;
        self.add(
            "hud/sidebar/rows/background".into(),
            "presentation",
            String::new(),
            [
                (left - 2) as f32,
                (top - 1) as f32,
                (right - left + 2) as f32,
                (bottom - top + 1) as f32,
            ],
            true,
            None,
            NodeAction::None,
            Paint::Solid([0.; 3], self.text_background_opacity(0.3)),
        )?;
        self.sidebar_text(
            "hud/sidebar/title",
            title,
            left + sidebar.width / 2 - title.width / 2,
            top - 9,
        )?;
        for (i, row) in sidebar.rows.iter().enumerate() {
            let y = top + i as i32 * 9;
            self.sidebar_text(&row.name_id, &row.name, left, y)?;
            self.sidebar_text(&row.score_id, &row.score, right - row.score.width, y)?;
        }
        Ok(())
    }
    fn sidebar_text(&mut self, id: &str, text: &SidebarText, x: i32, y: i32) -> Result<()> {
        self.add(
            id.into(),
            "text",
            text.label.clone(),
            [x as f32, y as f32, text.width as f32, 9.],
            true,
            None,
            NodeAction::None,
            Paint::RichText(Arc::clone(&text.layout)),
        )?;
        self.nodes.last_mut().expect("sidebar node added").component =
            Some(Arc::clone(&text.component));
        Ok(())
    }
}
