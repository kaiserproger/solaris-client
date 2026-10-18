//! Ordinary client-side interaction predictions are a projection over received facts.
//! Each received field reconciles its overlay; no local change is an acknowledgement.
use super::{Entity, Handle, Metadata, Store};
use crate::{inventory::Stack, protocol::Result};
impl Entity {
    pub(crate) fn leash_value(&self) -> Option<Handle> {
        self.predicted_leash.unwrap_or(self.leash)
    }
    pub fn metadata_value(&self, index: u8) -> Option<&Metadata> {
        self.predicted_metadata
            .get(&index)
            .or_else(|| self.metadata.get(&index))
    }
    pub fn equipment_value(&self, slot: usize) -> Option<&Stack> {
        self.predicted_equipment
            .get(slot)
            .and_then(Option::as_ref)
            .map_or_else(
                || self.equipment.get(slot).and_then(Option::as_ref),
                Option::as_ref,
            )
    }
}
impl Store {
    pub(super) fn clear_leash_prediction(&mut self, id: i32) {
        let holder = self
            .get_mut(id)
            .and_then(|entity| entity.predicted_leash.take())
            .flatten();
        if let Some(holder) = holder {
            if let Some(links) = self.predicted_leash_holders.get_mut(&holder) {
                links.remove(&id);
                if links.is_empty() {
                    self.predicted_leash_holders.remove(&holder);
                }
            }
        }
    }

    pub(crate) fn predict_leash(&mut self, handle: Handle, holder: Option<Handle>) -> Result<()> {
        if self.resolve(handle)?.handle != handle {
            return Err("dragon child leash prediction".into());
        }
        if let Some(holder) = holder {
            if self.resolve(holder)?.handle != holder {
                return Err("dragon child leash holder prediction".into());
            }
        }
        self.clear_leash_prediction(handle.network_id);
        if let Some(holder) = holder {
            self.predicted_leash_holders
                .entry(holder)
                .or_default()
                .insert(handle.network_id);
        }
        self.revision += 1;
        let revision = self.revision;
        let entity = self.get_mut(handle.network_id).unwrap();
        entity.predicted_leash = Some(holder);
        entity.appearance_revision = revision;
        Ok(())
    }

    pub fn predict_equipment(
        &mut self,
        handle: Handle,
        slot: u8,
        stack: Option<Stack>,
    ) -> Result<()> {
        let e = self.resolve(handle)?;
        if e.handle != handle {
            return Err("dragon child equipment prediction".into());
        }
        let slot = usize::from(slot);
        if slot >= 8 {
            return Err("entity equipment slot bound".into());
        }
        self.revision += 1;
        let revision = self.revision;
        let e = self.get_mut(handle.network_id).unwrap();
        e.predicted_equipment[slot] = Some(stack);
        e.appearance_revision = revision;
        e.binding = None;
        Ok(())
    }
    pub fn predict_metadata(&mut self, handle: Handle, key: &str, value: Metadata) -> Result<()> {
        let e = self.resolve(handle)?;
        if e.handle != handle {
            return Err("dragon child metadata prediction".into());
        }
        let models = self.models_ref().ok_or("entity models not activated")?;
        let schema = models
            .schemas
            .get(&e.type_name)
            .ok_or("entity schema missing")?;
        let definition = schema
            .accessors
            .get(key)
            .ok_or("unknown source entity accessor")?;
        if definition[1] != value.serializer_id() {
            return Err("predicted entity metadata serializer mismatch".into());
        }
        let index = u8::try_from(definition[0]).map_err(|_| "entity accessor index bound")?;
        self.revision += 1;
        let models = self.models.clone().ok_or("entity models not activated")?;
        let revision = self.revision;
        let e = self.get_mut(handle.network_id).unwrap();
        e.predicted_metadata.insert(index, value);
        let mut changed = [false; 256];
        changed[usize::from(index)] = true;
        super::source_state::on_metadata(&models, e, &changed);
        e.appearance_revision = revision;
        e.binding = None;
        Ok(())
    }
}
