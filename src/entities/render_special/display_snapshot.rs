use super::{Entity, Metadata, Models, Stack, Value, p};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct DisplaySnapshot {
    pub block_state: u32,
    pub item: Option<Stack>,
    pub item_context: i32,
    pub text: Arc<Value>,
    pub text_revision: u64,
    pub line_width: i32,
    pub style_flags: i32,
}
impl Default for DisplaySnapshot {
    fn default() -> Self {
        Self {
            block_state: 0,
            item: None,
            item_context: 0,
            text: Arc::new(serde_json::json!({"text":""})),
            text_revision: 0,
            line_width: 200,
            style_flags: 0,
        }
    }
}
impl DisplaySnapshot {
    pub fn commit(previous: Option<Box<Self>>, models: &Models, entity: &Entity) -> Box<Self> {
        let mut snapshot = previous.unwrap_or_default();
        snapshot.block_state = match p::value(models, entity, "DATA_BLOCK_STATE_ID") {
            Some(Metadata::BlockState(state)) => *state,
            _ => 0,
        };
        snapshot.item = match p::value(models, entity, "DATA_ITEM_STACK_ID") {
            Some(Metadata::Item(stack)) => Some(stack.clone()),
            _ => None,
        };
        snapshot.item_context = p::number(models, entity, "DATA_ITEM_DISPLAY_ID").unwrap_or(0);
        let width = p::number(models, entity, "DATA_LINE_WIDTH_ID").unwrap_or(200);
        let text = match p::value(models, entity, "DATA_TEXT_ID") {
            Some(Metadata::Component(text)) => Some(text),
            _ => None,
        };
        if snapshot.line_width != width || text.is_some_and(|text| snapshot.text.as_ref() != text) {
            snapshot.text_revision = snapshot.text_revision.wrapping_add(1);
            snapshot.line_width = width;
            if let Some(text) = text {
                snapshot.text = Arc::new(text.clone());
            }
        }
        snapshot.style_flags = p::number(models, entity, "DATA_STYLE_FLAGS_ID").unwrap_or(0);
        snapshot
    }
}
