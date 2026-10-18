use super::*;
use crate::ui::shaping::TextLayout;
use std::collections::BTreeMap;
#[derive(Default)]
pub(crate) struct Cache {
    signs: BTreeMap<[i32; 3], SignLayouts>,
    errors: BTreeMap<[i32; 3], ErrorLayouts>,
}
pub(super) struct SignLayouts {
    revision: u64,
    font: usize,
    filtering: bool,
    pub front: Vec<TextLayout>,
    pub back: Vec<TextLayout>,
}
struct ErrorLayouts {
    revision: u64,
    font: usize,
    layouts: Vec<TextLayout>,
}
impl Cache {
    pub(crate) fn clear(&mut self) {
        self.signs.clear();
        self.errors.clear();
    }
    pub(crate) fn remove(&mut self, position: [i32; 3]) {
        self.signs.remove(&position);
        self.errors.remove(&position);
    }
    pub(super) fn errors(&mut self, e: &BlockEntity, text: &TextRenderer) -> Result<&[TextLayout]> {
        let font = text as *const TextRenderer as usize;
        let dirty = self
            .errors
            .get(&e.position)
            .is_none_or(|v| v.revision != e.revision || v.font != font);
        if dirty {
            let errors = e.data_value()["errors"].as_array();
            let mut layouts = Vec::with_capacity(errors.map_or(0, Vec::len));
            if let Some(errors) = errors {
                for error in errors {
                    // Source TestInstanceRenderer calls Component.getString: styles do not survive.
                    let plain = Value::String(text.language.plain(&error["text"])?);
                    layouts.push(text.layout(&plain, f32::MAX)?);
                }
            }
            self.errors.insert(
                e.position,
                ErrorLayouts {
                    revision: e.revision,
                    font,
                    layouts,
                },
            );
        }
        self.errors
            .get(&e.position)
            .map(|v| v.layouts.as_slice())
            .ok_or("source error marker layout cache missing".into())
    }
    pub(super) fn sign(
        &mut self,
        e: &BlockEntity,
        text: &TextRenderer,
        filtering: bool,
        hanging: bool,
    ) -> Result<&SignLayouts> {
        let font = text as *const TextRenderer as usize;
        let dirty = self
            .signs
            .get(&e.position)
            .is_none_or(|v| v.revision != e.revision || v.font != font || v.filtering != filtering);
        if dirty {
            let width = if hanging { 60. } else { 90. };
            let shape = |side: &str| -> Result<Vec<TextLayout>> {
                let data = &e.data_value()[side];
                let filtered = &data["filtered_messages"];
                let messages = if filtering && filtered.is_array() {
                    filtered
                } else {
                    &data["messages"]
                };
                let mut layouts = Vec::with_capacity(4);
                if let Some(messages) = messages.as_array().filter(|messages| {
                    messages.len() == 4 && messages.iter().all(|message| !message.is_null())
                }) {
                    for component in messages {
                        let mut layout = text.layout(component, width)?;
                        layout.glyphs.retain(|g| g.position[1] == 0.);
                        layout.line_widths.truncate(1);
                        layouts.push(layout);
                    }
                }
                Ok(layouts)
            };
            let front = shape("front_text")?;
            let back = shape("back_text")?;
            self.signs.insert(
                e.position,
                SignLayouts {
                    revision: e.revision,
                    font,
                    filtering,
                    front,
                    back,
                },
            );
        }
        self.signs
            .get(&e.position)
            .ok_or("source sign layout cache missing".into())
    }
}
