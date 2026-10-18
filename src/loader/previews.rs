//! Closed schema-2 blueprint previews. Runtime preview messages are not this codec.
use crate::{
    api::{MeshGeometry, RenderLayer, Vertex},
    protocol::{
        Result,
        content::Content,
        models::{BakedModel, TintKind},
    },
};
use glam::{Mat4, Vec3};
use serde::Deserialize;
use serde_json::Value;
use std::{collections::BTreeMap, sync::Arc};

pub(crate) const MAX_PREVIEWS: usize = 64;
const MAX_BLOCKS: usize = 65_536;
const MAX_QUADS: usize = 65_536;
pub(crate) type PreviewCatalog = BTreeMap<String, Arc<WorldPreview>>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct WorldPreview {
    pub id: String,
    pub blueprint_id: String,
    pub content_hash: String,
    pub rotation: u8,
    pub size_x: u8,
    pub size_y: u8,
    pub size_z: u8,
    pub blocks: Vec<PreviewBlock>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PreviewBlock {
    pub x: u8,
    pub y: u8,
    pub z: u8,
    pub block_id: String,
}
fn resource_id(id: &str) -> Result<()> {
    let (ns, path) = id
        .split_once(':')
        .ok_or("world preview requires namespaced identifier")?;
    if id.len() > 128
        || ns.is_empty()
        || path.is_empty()
        || !ns
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
        || !path
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_./-".contains(&b))
    {
        return Err("invalid world preview identifier".into());
    }
    Ok(())
}
/// Admission works on the already byte-bounded schema-2 index, never guessed assets.
pub(crate) fn parse(owner: &str, entries: &[Value]) -> Result<PreviewCatalog> {
    if entries.len() > MAX_PREVIEWS {
        return Err("world preview registry limit".into());
    }
    let mut catalog = BTreeMap::new();
    for entry in entries {
        // Check the collection limit before serde allocates a typed block vector.
        if entry["blocks"]
            .as_array()
            .is_none_or(|blocks| blocks.len() > MAX_BLOCKS)
        {
            return Err("world preview block count bound".into());
        }
        let preview = WorldPreview::deserialize(entry).map_err(|e| e.to_string())?;
        resource_id(&preview.id)?;
        if preview.id.split_once(':').map(|v| v.0) != Some(owner) {
            return Err("world preview id outside bundle owner".into());
        }
        resource_id(&preview.blueprint_id)?;
        if preview.content_hash.len() != 64
            || !preview
                .content_hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err("world preview hash requires 64 lowercase hex characters".into());
        }
        if preview.rotation > 3
            || [preview.size_x, preview.size_y, preview.size_z]
                .into_iter()
                .any(|axis| axis > 64)
        {
            return Err("world preview rotation/axis bound".into());
        }
        for block in &preview.blocks {
            resource_id(&block.block_id)?;
            if block.x >= preview.size_x || block.y >= preview.size_y || block.z >= preview.size_z {
                return Err("world preview block outside declared box".into());
            }
        }
        let id = preview.id.clone();
        if catalog.insert(id, Arc::new(preview)).is_some() {
            return Err("duplicate world preview id".into());
        }
    }
    Ok(catalog)
}
pub(crate) fn validate_references(screens: &[Value], catalog: &PreviewCatalog) -> Result<()> {
    for screen in screens {
        if let Some(widgets) = screen["widgets"].as_array() {
            for widget in widgets {
                if let Some(id) = widget.get("preview_id") {
                    if widget["type"] != "world_marker"
                        || id.as_str().is_none_or(|id| !catalog.contains_key(id))
                    {
                        return Err("Loader screen references undeclared world preview".into());
                    }
                }
            }
        }
    }
    Ok(())
}
impl WorldPreview {
    fn model<'a>(&self, content: &'a Content, block: &PreviewBlock) -> Result<&'a BakedModel> {
        let state = content.resolve_block_state(&block.block_id, &BTreeMap::new())?;
        if content.info(state)?.fluid.is_some() {
            return Err(format!(
                "unsupported world preview fluid material {}",
                block.block_id
            ));
        }
        content.model_for(state)?.ok_or_else(|| {
            format!(
                "world preview material has no baked model: {}",
                block.block_id
            )
        })
    }
    /// Required before ACK: unavailable/contextual materials never become selectable.
    pub(crate) fn validate_content(&self, content: &Content) -> Result<()> {
        let mut quads = 0usize;
        let mut models = BTreeMap::new();
        for block in &self.blocks {
            let model = match models.get(block.block_id.as_str()) {
                Some(model) => *model,
                None => {
                    let model = self.model(content, block)?;
                    models.insert(block.block_id.as_str(), model);
                    model
                }
            };
            quads = quads
                .checked_add(model.maximum_quad_count())
                .ok_or("world preview geometry overflow")?;
            if quads > MAX_QUADS {
                return Err("world preview geometry limit".into());
            }
            let mut unsupported = false;
            model.for_each_quad(
                [i32::from(block.x), i32::from(block.y), i32::from(block.z)],
                |q| {
                    unsupported |= !matches!(q.tint, TintKind::None | TintKind::Constant(_))
                        || q.positions.iter().flatten().any(|p| !p.is_finite());
                },
            );
            if unsupported {
                return Err(format!(
                    "unsupported world preview contextual tint or nonfinite model: {}",
                    block.block_id
                ));
            }
        }
        if quads == 0 && !self.blocks.is_empty() {
            return Err("world preview has no renderable geometry".into());
        }
        Ok(())
    }
    pub(crate) fn rotated_size(&self) -> [u8; 3] {
        if self.rotation & 1 == 0 {
            [self.size_x, self.size_y, self.size_z]
        } else {
            [self.size_z, self.size_y, self.size_x]
        }
    }
    pub(crate) fn rotate_point(&self, p: Vec3) -> Vec3 {
        match self.rotation {
            0 => p,
            1 => Vec3::new(f32::from(self.size_z) - p.z, p.y, p.x),
            2 => Vec3::new(
                f32::from(self.size_x) - p.x,
                p.y,
                f32::from(self.size_z) - p.z,
            ),
            3 => Vec3::new(p.z, p.y, f32::from(self.size_x) - p.x),
            _ => unreachable!("validated preview rotation"),
        }
    }
    /// Textured, baked default-state block geometry, not material-colored placeholder cubes.
    pub(crate) fn build_geometry(&self, content: &Content) -> Result<MeshGeometry> {
        self.validate_content(content)?;
        let mut out = MeshGeometry::default();
        let rotation =
            Mat4::from_rotation_x(30f32.to_radians()) * Mat4::from_rotation_y(-45f32.to_radians());
        let mut low = Vec3::splat(f32::INFINITY);
        let mut high = Vec3::splat(f32::NEG_INFINITY);
        let mut quads = 0;
        let mut models = BTreeMap::new();
        for block in &self.blocks {
            let model = match models.get(block.block_id.as_str()) {
                Some(model) => *model,
                None => {
                    let model = self.model(content, block)?;
                    models.insert(block.block_id.as_str(), model);
                    model
                }
            };
            let local = Vec3::new(f32::from(block.x), f32::from(block.y), f32::from(block.z));
            model.for_each_quad(
                [i32::from(block.x), i32::from(block.y), i32::from(block.z)],
                |q| {
                    quads += 1;
                    let color = match q.tint {
                        TintKind::Constant(c) => c,
                        _ => [1.; 3],
                    };
                    let base = out.vertices.len() as u32;
                    for i in 0..4 {
                        let point = rotation.transform_point3(
                            self.rotate_point(local + Vec3::from_array(q.positions[i])),
                        );
                        low = low.min(point);
                        high = high.max(point);
                        out.vertices.push(Vertex {
                            position: point.to_array(),
                            color,
                            uv: q.uv[i],
                            overlay_uv: [-1.; 2],
                            overlay_tint: [1.; 3],
                            alpha: q.alpha,
                            texture_layer: q.texture_layer,
                            light: [-1.; 2],
                            normal: [0.; 3],
                        });
                    }
                    out.indices[RenderLayer::Translucent as usize].extend([
                        base,
                        base + 1,
                        base + 2,
                        base,
                        base + 2,
                        base + 3,
                    ]);
                },
            );
        }
        if quads == 0 && !self.blocks.is_empty() {
            return Err("world preview has no renderable geometry".into());
        }
        if !out.vertices.is_empty() {
            let scale = 1. / (high.x - low.x).max(high.y - low.y).max(1.);
            let transform = Mat4::from_scale(Vec3::new(scale, -scale, 1.))
                * Mat4::from_translation(-(low + high) / 2.);
            for vertex in &mut out.vertices {
                vertex.position = transform
                    .transform_point3(Vec3::from_array(vertex.position))
                    .to_array();
            }
        }
        Ok(out)
    }
    pub(crate) fn append_cached(
        mesh: &MeshGeometry,
        [x, y, w, h]: [f32; 4],
        parent: Mat4,
        out: &mut MeshGeometry,
    ) {
        let scale = (w - 16.).min(h - 16.).max(1.);
        let transform = parent
            * Mat4::from_translation(Vec3::new(x + w / 2., y + h / 2., 0.))
            * Mat4::from_scale(Vec3::new(scale, scale, 1.));
        let base = out.vertices.len() as u32;
        for vertex in &mesh.vertices {
            let mut vertex = *vertex;
            vertex.position = transform
                .transform_point3(Vec3::from_array(vertex.position))
                .to_array();
            out.vertices.push(vertex);
        }
        out.indices[RenderLayer::Translucent as usize].extend(
            mesh.indices[RenderLayer::Translucent as usize]
                .iter()
                .map(|index| base + index),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn entry() -> Value {
        json!({"id":"example:gate","blueprint_id":"example:gate","content_hash":"a".repeat(64),"rotation":1,"size_x":2,"size_y":1,"size_z":3,"blocks":[{"x":1,"y":0,"z":2,"block_id":"minecraft:stone"}]})
    }
    #[test]
    fn preview_rejects_invalid_boxes_hashes_and_references() {
        let mut v = entry();
        let catalog = parse("example", &[v.clone()]).unwrap();
        let p = &catalog["example:gate"];
        assert_eq!(p.rotated_size(), [3, 1, 2]);
        assert_eq!(p.rotate_point(Vec3::new(1., 0., 2.)), Vec3::new(1., 0., 1.));
        v["blocks"][0]["x"] = json!(2);
        assert!(parse("example", &[v]).is_err());
        v = entry();
        v["content_hash"] = json!("A".repeat(64));
        assert!(parse("example", &[v]).is_err());
        assert!(parse("other", &[entry()]).is_err());
        assert!(parse("example", &[entry(), entry()]).is_err());
        assert!(
            validate_references(
                &[json!({"widgets":[{"type":"world_marker","preview_id":"example:missing"}]})],
                &catalog
            )
            .is_err()
        );
        assert!(
            validate_references(
                &[json!({"widgets":[{"type":"world_marker","preview_id":"example:gate"}]})],
                &catalog
            )
            .is_ok()
        );
    }
    #[test]
    fn preview_zero_axes_require_empty_geometry_and_collections_are_bounded() {
        let mut v = entry();
        v["size_x"] = json!(0);
        assert!(parse("example", &[v.clone()]).is_err());
        v["blocks"] = json!([]);
        assert!(parse("example", &[v]).is_ok());
        let mut v = entry();
        v["rotation"] = json!(4);
        assert!(parse("example", &[v]).is_err());
        let mut v = entry();
        v["size_z"] = json!(65);
        assert!(parse("example", &[v]).is_err());
        let mut v = entry();
        v["blocks"] = json!(vec![entry()["blocks"][0].clone(); MAX_BLOCKS + 1]);
        assert!(parse("example", &[v]).is_err());
        let mut v = entry();
        v["future"] = json!(true);
        assert!(parse("example", &[v]).is_err());
        assert!(parse("example", &vec![entry(); MAX_PREVIEWS + 1]).is_err());
    }
}
