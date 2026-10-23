//! Connection-scoped verified bundle resources. Plugin atlas pages are appended, so
//! previously baked terrain, entities, fonts and dynamic texture handles never move.
use super::*;
use crate::loader::items::{self, ItemDefinition};

pub(super) struct BaseResources {
    rgba: Arc<Vec<u8>>,
    mips: Arc<Vec<AtlasMip>>,
    layers: u32,
    textures: BTreeMap<String, TextureRegion>,
    materials: BTreeMap<String, RenderLayer>,
    animation_count: usize,
}

impl ResourceSource {
    fn with_loader(&self, paths: &BTreeMap<String, Arc<[u8]>>) -> Result<Self> {
        if paths.len() > 128 {
            return Err("Loader activated resource count exceeds 128".into());
        }
        let mut total = 0usize;
        for (path, bytes) in paths {
            safe_relative(path)?;
            let rest = path
                .strip_prefix("assets/")
                .ok_or("Loader resource must be under assets/")?;
            let (ns, resource) = rest
                .split_once('/')
                .ok_or("Loader resource namespace missing")?;
            split_id(&format!("{ns}:{resource}"))?;
            if ns == "minecraft" {
                return Err(format!(
                    "Loader resource cannot override built-in namespace {ns}"
                ));
            }
            if ns == "solaris" {
                for root in &self.roots {
                    if root_resource_bytes(root, path, self.limits.max_source_bytes)?.is_some() {
                        return Err(format!("Loader resource overrides built-in resource {path}"));
                    }
                }
                if self.indexed.contains_key(path) {
                    return Err(format!("Loader resource overrides built-in resource {path}"));
                }
            }
            if bytes.len() as u64 > self.limits.max_source_bytes {
                return Err(format!("Loader resource byte bound {path}"));
            }
            if path.ends_with(".png") {
                validate_png_header(bytes, self.limits, path)?;
            }
            if path.ends_with(".png.mcmeta") {
                if bytes.len() > 1 << 20 {
                    return Err(format!("Loader texture metadata JSON bound {path}"));
                }
                let metadata: Value = serde_json::from_slice(bytes)
                    .map_err(|error| format!("Loader texture metadata {path}: {error}"))?;
                let metadata = metadata
                    .as_object()
                    .ok_or("Loader texture metadata must be an object")?;
                if metadata.contains_key("animation") {
                    return Err(format!("unsupported Loader animated texture {path}"));
                }
            }
            total = total
                .checked_add(bytes.len())
                .filter(|n| *n <= 64 << 20)
                .ok_or("Loader activated resource byte bound")?;
        }
        let mut source = self.clone();
        source.loader = paths.clone();
        Ok(source)
    }
}

impl Content {
    pub(super) fn loader_retained_bytes(&self) -> usize {
        self.loader_base.as_ref().map_or(0, |base| {
            let rgba = if Arc::ptr_eq(&base.rgba, &self.rgba) {
                0
            } else {
                base.rgba.len()
            };
            let mips = if Arc::ptr_eq(&base.mips, &self.mips) {
                0
            } else {
                base.mips.iter().map(|mip| mip.rgba.len()).sum()
            };
            rgba + mips
        })
    }
    pub fn loader_items(&self) -> &BTreeMap<String, ItemDefinition> {
        &self.loader_items
    }

    /// `paths` must contain the exact canonical paths and bytes verified by Loader.
    /// Validates carriers, every admitted graph, model and sprite before publication.
    /// Caller installs this atlas and invalidates item caches before acknowledging activation.
    pub fn activate_loader_resources(
        &mut self,
        paths: &BTreeMap<String, Arc<[u8]>>,
        entries: &[Value],
        block_entries: &[Value],
    ) -> Result<()> {
        let declarations = items::parse_items(entries)?;
        let source = self.source.with_loader(paths)?;
        for item in declarations.values() {
            if !self.item_names.values().any(|name| name == &item.base_item) {
                return Err(format!(
                    "Loader item {} has unknown base_item {}",
                    item.id, item.base_item
                ));
            }
            if self.item_names.values().any(|name| name == &item.id) {
                return Err(format!(
                    "Loader item {} collides with vanilla registry",
                    item.id
                ));
            }
            let (ns, path) = split_id(&item.id)?;
            if !paths.contains_key(&format!("assets/{ns}/items/{path}.json")) {
                return Err(format!(
                    "Loader item {} lacks verified item definition",
                    item.id
                ));
            }
        }
        let mut baker = Baker::new(&source);
        let blocks = super::loader_blocks::PreparedBlocks::new(self, &source, block_entries, &declarations, paths)?;
        let mut raw = Vec::new();
        let mut sprite_names = BTreeSet::new();
        let mut visited = BTreeSet::new();
        let mut baked_ids = BTreeSet::new();
        let mut quad_count = 0usize;
        for item in declarations.values() {
            let graph = source.json("items", &item.id)?;
            for model_id in
                items::model_ids(&graph).map_err(|e| format!("Loader item {}: {e}", item.id))?
            {
                if !baked_ids.insert(model_id.clone()) {
                    continue;
                }
                if baked_ids.len() > 256 {
                    return Err("Loader item model count exceeds 256".into());
                }
                validate_model(&source, &model_id, &mut Vec::new(), &mut visited)?;
                let model = baker
                    .model(&model_id)
                    .map_err(|e| format!("Loader item {} model {model_id}: {e}", item.id))?;
                quad_count = quad_count
                    .checked_add(model.maximum_quad_count())
                    .filter(|count| *count <= 65536)
                    .ok_or("Loader item geometry exceeds 65536 quads")?;
                model.textures(&mut sprite_names);
                raw.push((item.id.clone(), model));
            }
        }
        blocks.textures(&mut sprite_names);
        let base_textures = self
            .loader_base
            .as_ref()
            .map_or(&self.textures, |base| &base.textures);
        // Include all verified textures, not just currently referenced ones. These are shared
        // by item resources and ordinary resource consumers such as plugin previews.
        for path in paths.keys() {
            if let Some((ns, path)) = path.strip_prefix("assets/").and_then(|p| p.split_once('/')) {
                if let Some(texture) = path
                    .strip_prefix("textures/")
                    .and_then(|p| p.strip_suffix(".png"))
                {
                    sprite_names.insert(format!("{ns}:{texture}"));
                }
            }
        }
        sprite_names.retain(|name| !base_textures.contains_key(name));
        // State-only clients still decode sprites and bake all model geometry; they do not
        // acquire atlas pages, a GPU, or vanilla presentation resources.
        if self.width == 0 {
            let mut textures = BTreeMap::new();
            let mut materials = BTreeMap::new();
            let mut decoded_bytes = 0usize;
            for name in &sprite_names {
                let image = decode_texture(&source, name)?;
                decoded_bytes = decoded_bytes
                    .checked_add(image.rgba.len())
                    .filter(|count| *count <= source.limits.max_decoded_bytes)
                    .ok_or("Loader texture decoded byte budget exceeded")?;
                let metadata = source.texture_metadata(name)?;
                let [width, height] = sprite_dimensions([image.width, image.height], &metadata)?;
                textures.insert(
                    name.clone(),
                    TextureRegion {
                        uv: [0., 0., 1., 1.],
                        layer: 0,
                        width,
                        height,
                    },
                );
                materials.insert(name.clone(), RenderLayer::Translucent);
            }
            for (id, model) in raw {
                model
                    .finish(&textures, &materials)
                    .map_err(|e| format!("Loader item {id}: {e}"))?;
            }
            let blocks = blocks.finish(&textures, &materials)?;
            self.install_loader_blocks(blocks);
            self.source = source;
            self.loader_items = declarations;
            return Ok(());
        }
        if self.width != 2048 || self.height != 2048 {
            return Err("Loader item atlas requires the native 2048-page atlas".into());
        }
        let base_layers = self
            .loader_base
            .as_ref()
            .map_or(self.texture_layers, |base| base.layers);
        let base_rgba = self
            .loader_base
            .as_ref()
            .map_or(&self.rgba, |base| &base.rgba);
        let base_mips = self
            .loader_base
            .as_ref()
            .map_or(&self.mips, |base| &base.mips);
        let base_materials = self
            .loader_base
            .as_ref()
            .map_or(&self.texture_materials, |base| &base.materials);
        let mut textures = base_textures.clone();
        let mut materials = base_materials.clone();
        let mut atlas = if sprite_names.is_empty() {
            Atlas::default()
        } else {
            Atlas::build(&source, &sprite_names)?
        };
        // The connection keeps the original dynamic profile/map page. The separate atlas
        // builder reserves another one, which is never addressed by these static sprites.
        // Drop that unused page before accounting/copying rather than doubling the reserve.
        if atlas.layers > 0 {
            atlas.layers = atlas.dynamic_first_layer;
            atlas
                .rgba
                .truncate(atlas.layers as usize * atlas.side as usize * atlas.side as usize * 4);
            for mip in &mut atlas.mips {
                Arc::make_mut(&mut mip.rgba)
                    .truncate(atlas.layers as usize * mip.width as usize * mip.height as usize * 4);
            }
        }
        for (name, region) in &atlas.textures {
            if name == "solaris:white" || name == "minecraft:missingno" {
                continue;
            }
            let mut region = *region;
            region.layer = region
                .layer
                .checked_add(base_layers)
                .ok_or("Loader atlas layer overflow")?;
            textures.insert(name.clone(), region);
            materials.insert(name.clone(), atlas.materials[name]);
        }
        for (id, model) in raw {
            model
                .finish(&textures, &materials)
                .map_err(|e| format!("Loader item {id}: {e}"))?;
        }
        let blocks = blocks.finish(&textures, &materials)?;
        let atlas_bytes = base_rgba.len()
            + base_mips.iter().map(|m| m.rgba.len()).sum::<usize>()
            + atlas.rgba.len()
            + atlas.mips.iter().map(|m| m.rgba.len()).sum::<usize>();
        if atlas_bytes > source.limits.max_atlas_bytes
            || self
                .resource_bytes_used()
                .checked_add(atlas_bytes)
                .is_none_or(|n| n > source.limits.max_reload_bytes)
        {
            return Err("Loader atlas active+staging memory budget exceeded".into());
        }
        let layers = base_layers
            .checked_add(atlas.layers)
            .ok_or("Loader atlas layer overflow")?;
        let rgba = if atlas.rgba.is_empty() {
            Arc::clone(base_rgba)
        } else {
            let mut rgba = Vec::with_capacity(base_rgba.len() + atlas.rgba.len());
            rgba.extend_from_slice(base_rgba);
            rgba.append(&mut atlas.rgba);
            Arc::new(rgba)
        };
        let mips = if atlas.mips.is_empty() {
            Arc::clone(base_mips)
        } else {
            let mut mips = Vec::with_capacity(base_mips.len());
            for (base, extra) in base_mips.iter().zip(&atlas.mips) {
                let mut rgba = Vec::with_capacity(base.rgba.len() + extra.rgba.len());
                rgba.extend_from_slice(&base.rgba);
                rgba.extend_from_slice(&extra.rgba);
                mips.push(AtlasMip {
                    level: base.level,
                    width: base.width,
                    height: base.height,
                    rgba: Arc::new(rgba),
                });
            }
            Arc::new(mips)
        };
        for animation in &mut atlas.animations {
            animation.layer += base_layers;
        }
        // No fallible work after this point: the source, atlas and declarations publish together.
        self.install_loader_blocks(blocks);
        if self.loader_base.is_none() {
            self.loader_base = Some(BaseResources {
                rgba: Arc::clone(&self.rgba),
                mips: Arc::clone(&self.mips),
                layers: self.texture_layers,
                textures: std::mem::take(&mut self.textures),
                materials: std::mem::take(&mut self.texture_materials),
                animation_count: self.animations.len(),
            });
        }
        self.animations
            .truncate(self.loader_base.as_ref().unwrap().animation_count);
        self.animations.extend(atlas.animations);
        self.rgba = rgba;
        self.mips = mips;
        self.texture_layers = layers;
        self.textures = textures;
        self.texture_materials = materials;
        self.source = source;
        self.loader_items = declarations;
        Ok(())
    }

    /// Restores local pack resources without rebuilding them. The caller must invalidate item
    /// caches/install the restored atlas before another connection can present a frame.
    pub fn clear_loader_resources(&mut self) -> Result<()> {
        if let Some(base) = self.loader_base.take() {
            self.rgba = base.rgba;
            self.mips = base.mips;
            self.texture_layers = base.layers;
            self.textures = base.textures;
            self.texture_materials = base.materials;
            self.animations.truncate(base.animation_count);
        }
        self.source.loader.clear();
        self.clear_loader_blocks();
        self.loader_items.clear();
        Ok(())
    }
}

pub(super) fn validate_model(
    source: &ResourceSource,
    id: &str,
    stack: &mut Vec<String>,
    visited: &mut BTreeSet<String>,
) -> Result<()> {
    let id = normalize_id(id)?;
    if stack.len() >= 64 || stack.contains(&id) {
        return Err(format!("Loader model parent cycle/depth at {id}"));
    }
    if visited.contains(&id) {
        return Ok(());
    }
    if visited.len() + stack.len() >= 256 {
        return Err("Loader model dependency count exceeds 256".into());
    }
    let model = source.json("models", &id)?;
    let object = model.as_object().ok_or("Loader model must be an object")?;
    for key in object.keys() {
        if ![
            "parent",
            "textures",
            "elements",
            "display",
            "ambientocclusion",
            "gui_light",
        ]
        .contains(&key.as_str())
        {
            return Err(format!("unsupported Loader model field {key} in {id}"));
        }
    }
    if model.get("elements").is_some_and(|v| !v.is_array()) {
        return Err(format!("Loader model elements must be an array in {id}"));
    }
    if let Some(elements) = model["elements"].as_array() {
        for element in elements {
            check_fields(element, &["from", "to", "rotation", "shade", "faces"], &id)?;
            if element.get("shade").is_some_and(|v| !v.is_boolean()) {
                return Err(format!("Loader element shade must be boolean in {id}"));
            }
            if let Some(rotation) = element.get("rotation") {
                check_fields(
                    rotation,
                    &["origin", "axis", "angle", "angles", "rescale"],
                    &id,
                )?;
                if rotation.get("rescale").is_some_and(|v| !v.is_boolean()) {
                    return Err(format!("Loader rotation rescale must be boolean in {id}"));
                }
            }
            let faces = element["faces"]
                .as_object()
                .ok_or("Loader model faces must be an object")?;
            for face in faces.values() {
                check_fields(face, &["uv", "texture", "rotation", "cullface"], &id)?;
            }
        }
    }
    if let Some(display) = model.get("display") {
        for transform in display
            .as_object()
            .ok_or("Loader model display must be an object")?
            .values()
        {
            check_fields(transform, &["rotation", "translation", "scale"], &id)?;
        }
    }
    if let Some(textures) = model.get("textures") {
        for texture in textures
            .as_object()
            .ok_or("Loader model textures must be an object")?
            .values()
        {
            if !texture.is_string() {
                return Err(format!("unsupported Loader texture descriptor in {id}"));
            }
        }
    }
    if model
        .get("ambientocclusion")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(format!(
            "Loader model ambientocclusion must be boolean in {id}"
        ));
    }
    if model
        .get("gui_light")
        .is_some_and(|v| !matches!(v.as_str(), Some("front" | "side")))
    {
        return Err(format!("Loader model gui_light invalid in {id}"));
    }
    if let Some(parent) = model.get("parent") {
        let parent = parent
            .as_str()
            .ok_or("Loader model parent must be a resource identifier")?;
        if !matches!(parent, "builtin/generated" | "minecraft:builtin/generated") {
            if matches!(parent, "builtin/entity" | "minecraft:builtin/entity") {
                return Err(format!("unsupported Loader special model {id}"));
            }
            stack.push(id.clone());
            validate_model(source, parent, stack, visited)?;
            stack.pop();
        }
    }
    visited.insert(id);
    Ok(())
}

fn check_fields(value: &Value, allowed: &[&str], id: &str) -> Result<()> {
    let fields = value
        .as_object()
        .ok_or_else(|| format!("Loader model {id} descriptor must be an object"))?;
    for field in fields.keys() {
        if !allowed.contains(&field.as_str()) {
            return Err(format!("unsupported Loader model field {field} in {id}"));
        }
    }
    Ok(())
}

pub(super) fn validate_png_header(bytes: &[u8], limits: ResourceLimits, name: &str) -> Result<()> {
    if bytes.len() < 33
        || &bytes[..8] != b"\x89PNG\r\n\x1a\n"
        || bytes[8..12] != 13u32.to_be_bytes()
        || &bytes[12..16] != b"IHDR"
    {
        return Err(format!("Loader PNG IHDR missing or malformed {name}"));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    let decoded = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4));
    if width == 0
        || height == 0
        || width > limits.max_texture_dimension
        || height > limits.max_texture_dimension
        || decoded.is_none_or(|bytes| bytes > (32 << 20) || bytes > limits.max_decoded_bytes as u64)
    {
        return Err(format!(
            "Loader PNG dimensions/decoded byte bound {name} {width}x{height}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn monitor_carriers_rotate_and_restore_without_touching_vanilla_identity() {
        let (mut content, paths, items) = resources();
        let block = content.names.len();
        content.names.push("minecraft:black_glazed_terracotta".into());
        let mut states = Vec::new();
        for (facing, side) in [("north", 2), ("east", 5), ("south", 3), ("west", 4)] {
            let mut info = content.states[1].clone();
            info.block = block;
            states.push((content.states.len() as u32, side, info.model));
            content.states.push(info);
            content.properties.push(BTreeMap::from([("facing".into(), facing.into())]));
        }
        let blocks = [json!({"id":"example:monitor","name":"Monitor","model":"example:item/sword",
            "carrier":"minecraft:black_glazed_terracotta","drop_item":"example:sword"})];
        content.activate_loader_resources(&paths, &items, &blocks).unwrap();
        for &(state, side, _) in &states {
            assert_eq!(content.loader_block(state), Some(("example:monitor", side)));
            assert_eq!(content.name(state), Some("minecraft:black_glazed_terracotta"));
            assert_eq!(content.model_for(state).unwrap().unwrap().quads[0].face, side as u8);
        }
        content.clear_loader_resources().unwrap();
        for (state, _, model) in states {
            assert!(content.loader_block(state).is_none());
            assert_eq!(content.states[state as usize].model, model);
        }
    }

    fn resources() -> (Content, BTreeMap<String, Arc<[u8]>>, Vec<Value>) {
        let mut content = Content::test_cube();
        content.item_names.insert(1, "minecraft:paper".into());
        content.width = 2048;
        content.height = 2048;
        content.rgba = Arc::new(vec![0; 2048 * 2048 * 4]);
        content.mips = Arc::new(
            (1..=4)
                .map(|level| {
                    let side = 2048 >> level;
                    AtlasMip {
                        level,
                        width: side,
                        height: side,
                        rgba: Arc::new(vec![0; (side * side * 4) as usize]),
                    }
                })
                .collect(),
        );
        content.textures.insert(
            "minecraft:kept".into(),
            TextureRegion {
                uv: [0.1, 0.2, 0.3, 0.4],
                layer: 0,
                width: 16,
                height: 16,
            },
        );
        content
            .texture_materials
            .insert("minecraft:kept".into(), RenderLayer::Opaque);
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 16, 16);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&vec![255; 16 * 16 * 4]).unwrap();
        }
        let paths = BTreeMap::from([
            ("assets/example/items/sword.json".into(), Arc::from(serde_json::to_vec(&json!({"model":{"type":"minecraft:model","model":"example:item/sword"}})).unwrap())),
            ("assets/example/models/item/sword.json".into(), Arc::from(serde_json::to_vec(&json!({"textures":{"blade":"example:item/blade"},"elements":[{"from":[0,0,0],"to":[16,16,16],"faces":{"north":{"texture":"#blade"}}}]})).unwrap())),
            ("assets/example/textures/item/blade.png".into(), Arc::from(png)),
        ]);
        let entries =
            vec![json!({"id":"example:sword","base_item":"minecraft:paper","name":"Sword"})];
        (content, paths, entries)
    }

    #[test]
    fn activation_preserves_base_handles_and_failed_replacement_is_atomic() {
        let (mut content, mut paths, entries) = resources();
        let base = Arc::clone(&content.rgba);
        let kept = content.texture("minecraft:kept").unwrap();
        content.activate_loader_resources(&paths, &entries, &[]).unwrap();
        let texture = content.texture("example:item/blade").unwrap();
        assert!(texture.layer >= 1);
        let quad = &content.bake_model("example:item/sword").unwrap().quads[0];
        assert_eq!(quad.texture_layer, texture.layer);
        assert_eq!(
            quad.uv,
            [
                [texture.uv[0], texture.uv[1]],
                [texture.uv[0], texture.uv[3]],
                [texture.uv[2], texture.uv[3]],
                [texture.uv[2], texture.uv[1]],
            ]
        );
        let still_kept = content.texture("minecraft:kept").unwrap();
        assert_eq!((still_kept.uv, still_kept.layer), (kept.uv, kept.layer));
        let active = Arc::clone(&content.rgba);
        paths.insert(
            "assets/example/models/item/sword.json".into(),
            Arc::from(br#"{"parent":"example:item/sword"}"#.as_slice()),
        );
        assert!(
            content
                .activate_loader_resources(&paths, &entries, &[])
                .unwrap_err()
                .contains("cycle")
        );
        assert!(Arc::ptr_eq(&active, &content.rgba));
        assert_eq!(
            content.bake_model("example:item/sword").unwrap().quads[0].texture_layer,
            texture.layer
        );
        content.clear_loader_resources().unwrap();
        assert!(Arc::ptr_eq(&base, &content.rgba));
        assert!(content.loader_items().is_empty());
        assert!(content.texture("example:item/blade").is_none());
        assert!(
            content
                .resource_bytes("assets/example/items/sword.json")
                .is_err()
        );
    }

    #[test]
    fn replacing_connection_resources_does_not_accumulate_atlas_pages() {
        let (mut content, paths, entries) = resources();
        content.activate_loader_resources(&paths, &entries, &[]).unwrap();
        let layers = content.texture_layers;
        content.activate_loader_resources(&paths, &entries, &[]).unwrap();
        assert_eq!(content.texture_layers, layers);
        content.clear_loader_resources().unwrap();
        assert_eq!(content.texture_layers, 1);
        content.activate_loader_resources(&paths, &entries, &[]).unwrap();
        assert_eq!(content.texture_layers, layers);
    }

    #[test]
    fn animated_overlay_texture_cannot_silently_activate_as_static() {
        let (mut content, mut paths, entries) = resources();
        let base = Arc::clone(&content.rgba);
        paths.insert(
            "assets/example/textures/item/blade.png.mcmeta".into(),
            Arc::from(br#"{"animation":{"frametime":2,"frames":[0,1]}}"#.as_slice()),
        );
        assert!(
            content
                .activate_loader_resources(&paths, &entries, &[])
                .unwrap_err()
                .contains("unsupported Loader animated texture")
        );
        assert!(Arc::ptr_eq(&base, &content.rgba));
        assert!(content.loader_items().is_empty());
    }

    #[test]
    fn bad_carrier_missing_sprite_and_unsupported_model_never_publish() {
        let (mut content, paths, mut entries) = resources();
        let base = Arc::clone(&content.rgba);
        entries[0]["base_item"] = json!("minecraft:nonexistent");
        assert!(
            content
                .activate_loader_resources(&paths, &entries, &[])
                .unwrap_err()
                .contains("base_item")
        );
        entries[0]["base_item"] = json!("minecraft:paper");
        let mut invalid = paths.clone();
        invalid.remove("assets/example/textures/item/blade.png");
        assert!(
            content
                .activate_loader_resources(&invalid, &entries, &[])
                .unwrap_err()
                .contains("resource")
        );
        invalid = paths;
        invalid.insert(
            "assets/example/models/item/sword.json".into(),
            Arc::from(br#"{"overrides":[]}"#.as_slice()),
        );
        assert!(
            content
                .activate_loader_resources(&invalid, &entries, &[])
                .unwrap_err()
                .contains("overrides")
        );
        assert!(Arc::ptr_eq(&base, &content.rgba));
        assert!(content.loader_items().is_empty());
        assert!(
            content
                .resource_bytes("assets/example/items/sword.json")
                .is_err()
        );
    }

    #[test]
    fn png_allocation_bombs_are_rejected_from_the_header_before_activation() {
        let (mut content, mut paths, entries) = resources();
        let base = Arc::clone(&content.rgba);
        let path = "assets/example/textures/item/blade.png";
        let original = paths[path].to_vec();
        for dimensions in [(u32::MAX, u32::MAX), (4000, 4000), (0, 16)] {
            let mut png = original.clone();
            png[16..20].copy_from_slice(&dimensions.0.to_be_bytes());
            png[20..24].copy_from_slice(&dimensions.1.to_be_bytes());
            paths.insert(path.into(), Arc::from(png));
            assert!(
                content
                    .activate_loader_resources(&paths, &entries, &[])
                    .unwrap_err()
                    .contains("PNG dimensions")
            );
            assert!(Arc::ptr_eq(&base, &content.rgba));
            assert!(content.loader_items().is_empty());
        }
    }
}
