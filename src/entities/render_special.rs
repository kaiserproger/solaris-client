//! Source-specific nonliving presentation. Shared mesh emission and item baking have one owner.
mod display_snapshot;
mod items;
mod frame;
mod kernels;
mod painting;
mod projectiles;
mod timeline;
use super::{Entity, Handle, Metadata, Models, Store, presentation as p, resources};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    inventory::Stack,
    protocol::{
        Result,
        content::{Content, TextureRegion},
        models::BakedModel,
    },
    ui::{
        items::{ItemContext, ItemModels},
        shaping::TextLayout,
    },
};
pub(crate) use display_snapshot::DisplaySnapshot;
use glam::{Mat4, Vec3};
use serde_json::Value;
use std::collections::BTreeMap;
pub(crate) use timeline::{State, TickEffects};
fn source_sin(angle: f32) -> f32 {
    crate::player::math::sin(f64::from(angle))
}
fn source_cos(angle: f32) -> f32 {
    crate::player::math::cos(f64::from(angle))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Handled {
    Unhandled,
    Rendered,
    NoGeometry,
    PendingLighting,
    PendingWorld,
}
struct CachedText {
    revision: u64,
    layout: TextLayout,
}
struct ObservedItem {
    source: Value,
    stack: Option<Stack>,
}
#[derive(Default)]
pub(crate) struct Cache {
    models: BTreeMap<String, BakedModel>,
    text: BTreeMap<Handle, CachedText>,
    observed_items: BTreeMap<usize, ObservedItem>,
}
impl Cache {
    pub fn remove(&mut self, handle: Handle) {
        self.text.remove(&handle);
    }
    fn model<'a>(&'a mut self, content: &Content, name: &str) -> Result<&'a BakedModel> {
        if !self.models.contains_key(name) {
            if self.models.len() >= 512 {
                return Err("special entity cold model cache bound".into());
            }
            self.models.insert(name.into(), content.bake_model(name)?);
        }
        Ok(&self.models[name])
    }
}
fn translation(position: [f64; 3], origin: [f64; 3]) -> Mat4 {
    Mat4::from_translation(Vec3::from_array(std::array::from_fn(|i| {
        (position[i] - origin[i]) as f32
    })))
}
fn textured_quad(
    root: Mat4,
    points: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    texture: TextureRegion,
    color: [f32; 3],
    alpha: f32,
    layer: EntityMaterial,
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    p::emit(
        points.map(|point| root.transform_point3(Vec3::from_array(point)).to_array()),
        uv.map(|uv| {
            [
                texture.uv[0] + uv[0] * (texture.uv[2] - texture.uv[0]),
                texture.uv[1] + uv[1] * (texture.uv[3] - texture.uv[1]),
            ]
        }),
        color,
        texture.layer,
        layer,
        alpha,
        light,
        out,
    )
}
pub(crate) use items::append_observed_item;

#[expect(clippy::too_many_arguments, reason = "Shares the scene extraction inputs without a second render-state owner")]
pub(crate) fn append(
    store: &mut Store,
    slot: usize,
    models: &Models,
    content: &Content,
    origin: [f64; 3],
    tick: u64,
    partial: f32,
    light_source: &dyn Fn([f64; 3]) -> Result<Option<[f32; 2]>>,
    out: &mut DynamicGeometry,
) -> Result<Handled> {
    if !matches!(store.slots[slot].type_name.as_str(),
        "minecraft:painting" | "minecraft:item_frame" | "minecraft:glow_item_frame") {
        return Ok(Handled::Unhandled);
    }
    let vertex_start = out.vertices.len();
    let index_starts = out.indices.each_ref().map(Vec::len);
    let result = (|| {
        if store.slots[slot].type_name == "minecraft:painting" {
            return painting::append(&store.slots[slot], store, models,
                content, origin, tick, partial, light_source, out);
        }
        let entity = &store.slots[slot];
        let (dimensions, scale) = models.dimensions(entity)?;
        let Some(light) = p::entity_light(entity, models, tick, partial,
            dimensions, scale, light_source)? else {
            return Ok(Handled::PendingLighting);
        };
        frame::append(entity, models, &mut store.special_render_cache,
            &mut store.items, content, origin, tick, partial, light, out)
    })();
    if result.is_err() {
        out.vertices.truncate(vertex_start);
        for (indices, start) in out.indices.iter_mut().zip(index_starts) {
            indices.truncate(start);
        }
    }
    result
}
