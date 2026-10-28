use super::*;

#[test]
fn block_transformers_select_tool_predictions() {
    assert_eq!(
        transformer_action(Some(&serde_json::json!("minecraft:axe"))),
        Some("BlockTransformerAxe")
    );
    assert_eq!(
        transformer_action(Some(&serde_json::json!("minecraft:hoe"))),
        Some("BlockTransformerHoe")
    );
    assert_eq!(
        transformer_action(Some(&serde_json::json!("minecraft:shovel"))),
        Some("BlockTransformerShovel")
    );
}
