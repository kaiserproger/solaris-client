use super::state::{NodeAction, Paint, UiState};
use crate::protocol::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DesktopAction {
    PickWindow(u64),
    CancelPicker,
    DismissMenu,
    Control,
    ChangeWindow,
    StopCapture,
    RevokeViewer([u8; 16]),
    ViewPermission { request: u32, allow: bool },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DesktopDialog {
    pub title: String,
    pub body: Vec<String>,
    pub entries: Vec<(String, DesktopAction)>,
    pub selected: usize,
}

fn layout(view: [f32; 2], header: f32, footer: f32, heights: &[f32], selected: usize) -> ([f32; 4], usize, usize) {
    let width = (view[0] - 24.).clamp(1., 420.);
    let budget = (view[1] - header - footer - 24.).max(1.);
    let selected = selected.min(heights.len().saturating_sub(1));
    let mut start = selected;
    let mut end = (selected + 1).min(heights.len());
    let mut used = heights.get(selected).copied().unwrap_or(0.) + 4.;
    while start > 0 && used + heights[start - 1] + 4. <= budget {
        start -= 1;
        used += heights[start] + 4.;
    }
    while end < heights.len() && used + heights[end] + 4. <= budget {
        used += heights[end] + 4.;
        end += 1;
    }
    let height = header + used + footer;
    ([(view[0] - width) * 0.5, (view[1] - height) * 0.5, width, height], start, end)
}

impl UiState {
    pub(crate) fn set_desktop_dialog(&mut self, dialog: Option<DesktopDialog>) {
        if self.desktop_dialog != dialog {
            self.focus = dialog.as_ref().map(|d| format!("desktop/choice/{}", d.selected));
            self.desktop_dialog = dialog;
            self.desktop_action = None;
            self.invalidate();
        }
    }
    pub(crate) fn take_desktop_action(&mut self) -> Option<DesktopAction> {
        self.desktop_action.take()
    }
    pub(crate) fn activate_desktop(&mut self, action: DesktopAction) -> Result<()> {
        if !self.desktop_dialog.as_ref().is_some_and(|dialog| dialog.entries.iter().any(|(_, candidate)| *candidate == action)) {
            return Err("stale_desktop_dialog_action".into());
        }
        self.desktop_action = Some(action);
        Ok(())
    }
    pub(crate) fn desktop_dialog_tree(&mut self) -> Result<()> {
        let Some(dialog) = self.desktop_dialog.clone() else { return Ok(()); };
        let width = (self.width - 24.).clamp(1., 420.);
        let text_width = (width - 24.).max(1.);
        let measure = |text: &str, width| self.text.layout(&serde_json::json!({"text":text}), width).map(|layout| layout.height.max(9.));
        let title_height = measure(&dialog.title, text_width)?;
        let body_heights = dialog.body.iter().map(|line| measure(line, text_width)).collect::<Result<Vec<_>>>()?;
        let button_heights = dialog.entries.iter().map(|(label, _)| measure(label, (text_width - 8.).max(1.)).map(|height| (height + 10.).max(20.))).collect::<Result<Vec<_>>>()?;
        let hint = "Up/Down: select   Enter: confirm   Esc: cancel";
        let hint_height = measure(hint, text_width)?;
        let header = 12. + title_height + 8. + body_heights.iter().map(|height| height + 4.).sum::<f32>() + 4.;
        let footer = hint_height + 12.;
        let (rect, start, end) = layout([self.width, self.height], header, footer, &button_heights, dialog.selected);
        let [x, y, width, height] = rect;
        self.panel();
        self.add("desktop/dialog".into(), "dialog", dialog.title.clone(), rect, true, None, NodeAction::None, Paint::Frame)?;
        self.label("desktop/title", dialog.title, [x + 12., y + 12., text_width, title_height])?;
        let mut top = y + 12. + title_height + 8.;
        for (index, (line, height)) in dialog.body.iter().zip(body_heights).enumerate() {
            self.label(&format!("desktop/body/{index}"), line.clone(), [x + 12., top, text_width, height])?;
            top += height + 4.;
        }
        top = y + header;
        for (index, (label, action)) in dialog.entries.iter().enumerate().take(end).skip(start) {
            self.button(&format!("desktop/choice/{index}"), label, [x + 12., top, width - 24., button_heights[index]], NodeAction::Desktop(action.clone()), true, None)?;
            top += button_heights[index] + 4.;
        }
        self.label("desktop/hint", hint.into(), [x + 12., y + height - footer, text_width, hint_height])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn desktop_modal_centers_and_keeps_selected_row_visible_without_overlap() {
        let heights = (0..40).map(|index| if index % 3 == 0 { 38. } else { 20. }).collect::<Vec<_>>();
        for view in [[320., 240.], [427., 240.], [854., 480.], [1920., 1080.]] {
            let (rect, start, end) = layout(view, 68., 28., &heights, 35);
            assert!((rect[0] + rect[2] * 0.5 - view[0] * 0.5).abs() < 0.01);
            assert!((rect[1] + rect[3] * 0.5 - view[1] * 0.5).abs() < 0.01);
            assert!(rect[1] >= 0. && rect[1] + rect[3] <= view[1]);
            assert!((start..end).contains(&35));
            let last_button_bottom = 68. + heights[start..end].iter().map(|height| height + 4.).sum::<f32>() - 4.;
            assert!(last_button_bottom < rect[3] - 28.);
        }
    }
}
