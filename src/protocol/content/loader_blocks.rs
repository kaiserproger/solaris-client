//! Atomic carrier presentation: vanilla physical states, verified plugin models.
use super::*;
use crate::loader::{blocks, items::ItemDefinition};
use crate::protocol::models::RawModel;

pub(super) struct CarrierBlock {
    id: String,
    side: i32,
    original_model: Option<usize>,
}

pub(super) struct PreparedBlocks {
    entries: Vec<Value>,
    carriers: BTreeMap<u32, (String, i32, usize)>,
    models: Vec<RawModel>,
}
pub(super) struct BakedBlocks {
    entries: Vec<Value>,
    carriers: BTreeMap<u32, (String, i32, usize)>,
    models: Vec<BakedModel>,
}
impl PreparedBlocks {
    pub fn new(content: &Content, source: &ResourceSource, entries: &[Value],
        items: &BTreeMap<String, ItemDefinition>, paths: &BTreeMap<String, Arc<[u8]>>,
    ) -> Result<Self> {
        let declarations = blocks::parse(entries)?;
        let mut baker = Baker::new(source);
        let mut carriers = BTreeMap::new();
        for block in declarations {
            if !items.contains_key(&block.drop_item) {
                return Err(format!("Loader block {} has undeclared drop item", block.id));
            }
            let (ns, path) = split_id(&block.model)?;
            if !paths.contains_key(&format!("assets/{ns}/models/{path}.json")) {
                return Err(format!("Loader block {} lacks verified model", block.id));
            }
            super::loader_resources::validate_model(source, &block.model, &mut Vec::new(), &mut BTreeSet::new())?;
            let mut found = BTreeSet::new();
            for (state, info) in content.states.iter().enumerate() {
                if content.names[info.block] != block.carrier { continue; }
                let facing = content.properties[state].get("facing").map(String::as_str);
                let (side, rotation) = match facing {
                    Some("north") => (2, 0), Some("east") => (5, 90),
                    Some("south") => (3, 180), Some("west") => (4, 270),
                    _ => return Err("Loader carrier requires horizontal facing".into()),
                };
                if content.properties[state].len() != 1 || !found.insert(side) {
                    return Err("Loader carrier has ambiguous states".into());
                }
                let model = baker.state(&block.id, &serde_json::json!({}),
                    &serde_json::json!({"variants":{"":{"model":block.model,"y":rotation}}}))?;
                carriers.insert(state as u32, (block.id.clone(), side, model));
            }
            if found.len() != 4 { return Err(format!("Loader carrier {} is missing states", block.carrier)); }
        }
        let models = baker.into_models();
        if models.iter().map(RawModel::maximum_quad_count).sum::<usize>() > 65536 {
            return Err("Loader block geometry exceeds 65536 quads".into());
        }
        Ok(Self { entries: entries.to_vec(), carriers, models })
    }
    pub fn textures(&self, names: &mut BTreeSet<String>) {
        for model in &self.models { model.textures(names); }
    }
    pub fn finish(self, textures: &BTreeMap<String, TextureRegion>, materials: &BTreeMap<String, RenderLayer>) -> Result<BakedBlocks> {
        Ok(BakedBlocks { entries: self.entries, carriers: self.carriers,
            models: self.models.into_iter().map(|m| m.finish(textures, materials)).collect::<Result<_>>()? })
    }
}
impl Content {
    pub fn loader_block(&self, state: u32) -> Option<(&str, i32)> {
        self.loader_carriers.get(&state).map(|block| (block.id.as_str(), block.side))
    }
    pub(super) fn install_loader_blocks(&mut self, blocks: BakedBlocks) {
        self.clear_loader_blocks();
        let base = self.models.len();
        self.loader_model_base = Some(base);
        self.models.extend(blocks.models);
        for (state, (id, side, model)) in blocks.carriers {
            let info = &mut self.states[state as usize];
            self.loader_carriers.insert(state, CarrierBlock { id, side, original_model: info.model });
            info.model = Some(base + model);
        }
        self.loader_blocks = blocks.entries;
    }
    pub(super) fn clear_loader_blocks(&mut self) {
        for (state, block) in std::mem::take(&mut self.loader_carriers) {
            self.states[state as usize].model = block.original_model;
        }
        if let Some(base) = self.loader_model_base.take() { self.models.truncate(base); }
        self.loader_blocks.clear();
    }
}
