//! Authored byte fixtures for the 26.3 packet families that had stale routing.
use super::*;

#[test]
fn wire_263_mount_screen_and_merchant_offers_reach_their_owners() {
    let content = Content::test_cube();
    let mut store = InventoryStore::default();
    // Container ID, column count, big-endian entity ID.
    assert!(store.apply(42, &mut Reader::new(&[1, 2, 0, 0, 0, 42]), &content).unwrap());
    assert_eq!(store.menu().entity_id, Some(42));
    assert_eq!(store.menu().horse_columns, Some(2));
    // Container ID, empty offers, villager level/xp, show-progress/can-restock.
    assert!(store.apply(53, &mut Reader::new(&[1, 0, 1, 0, 1, 0]), &content).unwrap());
    // A mount menu is not silently replaced by merchant data for the same ID.
    assert_eq!(store.menu().kind, MenuKind::Horse);
}

#[test]
fn wire_263_experience_and_recipe_updates_are_not_entity_or_advancement_packets() {
    let content = Content::test_cube();
    let mut store = InventoryStore::default();
    // float experience fraction, level, total experience.
    assert!(store.apply(105, &mut Reader::new(&[0x3f, 0, 0, 0, 7, 12]), &content).unwrap());
    assert_eq!(store.experience_level, 7);
    // RecipeBookAdd: entry count, replace. UpdateRecipes: property count, stonecutter count.
    assert!(store.apply(75, &mut Reader::new(&[0, 1]), &content).unwrap());
    assert!(store.apply(136, &mut Reader::new(&[0, 0]), &content).unwrap());
    assert!(store.recipes.is_empty());
    assert!(store.recipe_property_sets.is_empty());
    assert!(!store.apply(103, &mut Reader::new(&[]), &content).unwrap());
    assert!(!store.apply(133, &mut Reader::new(&[]), &content).unwrap());
    assert!(store.apply(75, &mut Reader::new(&[0, 1, 0]), &content).is_err());
    assert!(store.apply(136, &mut Reader::new(&[0, 0, 0]), &content).is_err());
}
