//! Source-specific nonliving presentation. Shared mesh emission and item baking have one owner.
mod display_snapshot;
mod items;
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
