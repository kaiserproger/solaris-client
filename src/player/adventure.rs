//! Source BlockInWorld state and saveWithFullMetadata NBT for adventure predicates.
use super::*;
use crate::inventory::{AdventureBlock, Stack, Tag, nbt};

pub(super) fn matches(
    item: Option<&Stack>,
    component: &str,
    hit: &BlockHit,
    world: &World,
    entities: &EntityStore,
    content: &Content,
) -> Result<bool> {
    let Some(item) = item else { return Ok(false) };
    let Some(predicate) = item.component(component) else {
        return Ok(false);
    };
    let needs_nbt = predicate
        .get("predicates")
        .and_then(Value::as_array)
        .is_some_and(|predicates| predicates.iter().any(|p| p.get("nbt").is_some()));
    let nbt = if needs_nbt {
        full_metadata(hit.position, world, entities, content)?
    } else {
        None
    };
    item.matches_adventure(
        component,
        &AdventureBlock {
            state: hit.state,
            nbt: nbt.as_ref(),
        },
        content,
    )
}

pub(super) fn full_metadata(
    position: [i32; 3],
    world: &World,
    entities: &EntityStore,
    content: &Content,
) -> Result<Option<Tag>> {
    let chunk_entity = world
        .chunk([position[0].div_euclid(16), position[2].div_euclid(16)])
        .and_then(|chunk| {
            chunk
                .block_entities
                .iter()
                .find(|be| be.position == position)
        });
    let (raw, name) = if let Some(raw) = entities.block_entity_nbt(position) {
        (
            raw,
            entities
                .block_entity_name(position)
                .ok_or("missing_block_entity_type")?,
        )
    } else if let Some(be) = chunk_entity {
        (
            be.raw_nbt.as_ref(),
            content
                .block_entity_names
                .get(&be.kind)
                .ok_or("unknown_block_entity_type")?
                .as_str(),
        )
    } else {
        return Ok(None);
    };
    let mut reader = Reader::new(raw);
    let value = nbt::read(&mut reader)?;
    reader.done()?;
    let Tag::Map(mut fields) = value else {
        return Err("block_entity_nbt_not_compound".into());
    };
    // The packet omits saveWithFullMetadata's owner/type/position fields. Reconstruct
    // exactly these typed tags, overriding any untrusted payload copies.
    for (key, value) in [
        ("id", Tag::String(name.to_owned())),
        ("x", Tag::Int(position[0])),
        ("y", Tag::Int(position[1])),
        ("z", Tag::Int(position[2])),
    ] {
        let key = Tag::String(key.into());
        if let Some((_, existing)) = fields.iter_mut().find(|(k, _)| *k == key) {
            *existing = value;
        } else {
            fields.push((key, value));
        }
    }
    Ok(Some(Tag::Map(fields)))
}
