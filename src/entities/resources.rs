use super::render_living::MotionFacts;
use super::solaris_models;
use crate::protocol::{content::Content, Result};
use glam::{Mat4, Vec3};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Solaris-owned entity metadata: protocol ids, dimensions, metadata accessors, texture/variant
/// selection, head/body turn limits and armor shell keys. Pure data, no geometry.
const ENTITY_FACTS: &str = include_str!("../../assets/solaris/entities/entity-facts.json");
const FACTS_SCHEMA: u32 = 1;

#[derive(Deserialize)]
struct Document {
    schema: u32,
    game_version: String,
    entities: BTreeMap<String, EntitySchema>,
    families: BTreeMap<String, Family>,
    armor_models: BTreeMap<String, ArmorModels>,
    simple_particles: Vec<String>,
    default_skins: Vec<DefaultSkin>,
    poses: BTreeMap<u8, String>,
    attributes: BTreeMap<String, AttributeBounds>,
    builtin_variants: BTreeMap<String, BTreeMap<String, Value>>,
    motion: BTreeMap<String, MotionFacts>,
}
#[derive(Deserialize, Debug)]
pub struct DefaultSkin {
    pub texture: String,
    pub model: super::PlayerModel,
}
#[derive(Clone, Deserialize, Debug)]
pub struct Dimensions {
    pub width: f32,
    pub height: f32,
    pub eye_height: f32,
    pub fixed: bool,
    pub attachments: BTreeMap<String, Vec<[f64; 3]>>,
}
#[derive(Debug)]
pub enum DimensionConstant {
    Dimensions(Dimensions),
    Poses(BTreeMap<String, Dimensions>),
}
impl<'de> Deserialize<'de> for DimensionConstant {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        // serde's untagged buffer loses serde_json's arbitrary-precision number
        // representation. Dispatch the two object shapes before typed decoding.
        let value = Value::deserialize(deserializer)?;
        if value.get("width").is_some() {
            serde_json::from_value(value)
                .map(Self::Dimensions)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::Poses)
                .map_err(serde::de::Error::custom)
        }
    }
}
#[derive(Deserialize, Debug)]
pub struct DefaultAttribute {
    pub id: u32,
    pub value: f64,
}
#[derive(Deserialize, Debug)]
pub struct AttributeBounds {
    pub id: u32,
    pub min: Option<f64>,
    pub max: Option<f64>,
}
#[derive(Deserialize, Debug)]
pub struct EntitySchema {
    pub protocol_id: u32,
    pub width: f32,
    pub fire_immune: bool,
    pub class: String,
    pub eye_height: f32,
    pub dimension_constants: BTreeMap<String, DimensionConstant>,
    pub enum_ids: BTreeMap<String, BTreeMap<i32, String>>,
    pub enum_numbers: BTreeMap<String, BTreeMap<i32, BTreeMap<String, f32>>>,
    pub hierarchy: Vec<String>,
    pub method_owners: BTreeMap<String, String>,
    pub dimensions: Dimensions,
    pub default_attributes: BTreeMap<String, DefaultAttribute>,
    pub height: f32,
    pub accessors: BTreeMap<String, [u32; 2]>,
}
#[derive(Deserialize, Debug)]
pub struct Family {
    pub rig: String,
    pub texture: Option<String>,
    pub baby_rig: Option<String>,
    pub textures: Vec<String>,
    pub renderer_class: Option<String>,
    pub materials: BTreeMap<String, Value>,
}
#[derive(Clone, Copy, PartialEq)]
pub enum BoneRole {
    Fixed,
    Head,
    RightArm,
    LeftArm,
    RightLeg,
    LeftLeg,
    Wing,
    /// Villager-family `arms` bone: the main-hand item rests across it (`CrossedArmsItemLayer`).
    CrossedArms,
    /// Witch nose: a held potion hangs from it.
    Nose,
}
pub struct Bone {
    pub parent: Option<usize>,
    pub pivot: Vec3,
    pub rotation: Vec3,
    pub scale: Vec3,
    pub role: BoneRole,
    pub name: String,
    pub quads: Vec<[[f32; 5]; 4]>,
    pub normals: Vec<[f32; 3]>,
}
pub struct Rig {
    pub bones: Vec<Bone>,
    pub part_by_name: BTreeMap<String, usize>,
}
pub struct Models {
    pub rigs: BTreeMap<String, Rig>,
    pub schemas: BTreeMap<String, EntitySchema>,
    pub families: BTreeMap<String, Family>,
    pub builtin_variants: BTreeMap<String, BTreeMap<String, Value>>,
    pub simple_particles: Vec<String>,
    pub default_skins: Vec<DefaultSkin>,
    pub poses: BTreeMap<u8, String>,
    pub attributes: BTreeMap<String, AttributeBounds>,
    pub attribute_ids: BTreeMap<u32, String>,
    pub digest: String,
    pub(crate) armor_models: BTreeMap<String, ArmorModels>,
    pub(crate) motion: BTreeMap<String, MotionFacts>,
}
/// Renderer rig keys of the per-slot armor shells of a humanoid family.
#[derive(Deserialize, Debug)]
pub(crate) struct ArmorModels {
    pub adult: BTreeMap<String, String>,
    pub baby: BTreeMap<String, String>,
}
impl Models {
    fn parse() -> Result<Document> {
        let doc: Document = serde_json::from_str(ENTITY_FACTS)
            .map_err(|e| format!("solaris entity facts schema: {e}"))?;
        if doc.schema != FACTS_SCHEMA || doc.game_version != "26.3" {
            return Err("solaris entity facts source/target mismatch".into());
        }
        if doc.entities.len() > 1024 || doc.poses.len() != 18 {
            return Err("solaris entity facts catalog mismatch".into());
        }
        for (name, schema) in &doc.entities {
            if !schema.width.is_finite()
                || schema.width < 0.
                || !schema.height.is_finite()
                || schema.height < 0.
                || !schema.eye_height.is_finite()
            {
                return Err(format!("entity dimensions invalid for {name}"));
            }
        }
        if doc.motion.len() != doc.entities.len()
            || doc.motion.iter().any(|(kind, facts)| {
                doc.entities
                    .get(kind)
                    .is_none_or(|schema| schema.class != facts.class)
            })
        {
            return Err("entity motion holder coverage mismatch".into());
        }
        Ok(doc)
    }
    fn check_registry(doc: &Document, content: &Content) -> Result<()> {
        for (name, schema) in &doc.entities {
            if content.entity_names.get(&schema.protocol_id) != Some(name) {
                return Err(format!("entity registry/facts mismatch for {name}"));
            }
        }
        Ok(())
    }
    fn digest() -> String {
        let mut digest = Sha256::new();
        digest.update(ENTITY_FACTS.as_bytes());
        digest.update(solaris_models::embedded_digest());
        format!("{:x}", digest.finalize())
    }
    fn attribute_ids(attributes: &BTreeMap<String, AttributeBounds>) -> BTreeMap<u32, String> {
        attributes
            .iter()
            .map(|(name, value)| (value.id, name.clone()))
            .collect()
    }
    /// Protocol/dimension/motion metadata only: no families or render resources.
    pub(crate) fn load_state(content: &Content) -> Result<Self> {
        let doc = Self::parse()?;
        Self::check_registry(&doc, content)?;
        Ok(Self {
            rigs: BTreeMap::new(),
            families: BTreeMap::new(),
            simple_particles: doc.simple_particles,
            default_skins: Vec::new(),
            schemas: doc.entities,
            poses: doc.poses,
            builtin_variants: doc.builtin_variants,
            attribute_ids: Self::attribute_ids(&doc.attributes),
            attributes: doc.attributes,
            digest: Self::digest(),
            armor_models: BTreeMap::new(),
            motion: doc.motion,
        })
    }
    pub fn load(content: &Content) -> Result<Self> {
        let doc = Self::parse()?;
        if doc.default_skins.len() != 18 {
            return Err("source skin catalog mismatch".into());
        }
        Self::check_registry(&doc, content)?;
        // Geometry is Solaris-authored (`solaris_models`); the facts carry no meshes. A family
        // whose rig has no authored model gets a visible entity-sized placeholder box.
        let mut rigs = BTreeMap::new();
        for (kind, family) in &doc.families {
            let (width, height) = doc
                .entities
                .get(kind)
                .map_or((0.6, 1.8), |schema| (schema.width, schema.height));
            for key in std::iter::once(&family.rig).chain(family.baby_rig.as_ref()) {
                if solaris_models::authored(key).is_none() {
                    rigs.entry(key.clone())
                        .or_insert_with(|| solaris_models::placeholder(width, height));
                }
            }
        }
        Ok(Self {
            rigs,
            schemas: doc.entities,
            families: doc.families,
            builtin_variants: doc.builtin_variants,
            simple_particles: doc.simple_particles,
            default_skins: doc.default_skins,
            poses: doc.poses,
            attribute_ids: Self::attribute_ids(&doc.attributes),
            attributes: doc.attributes,
            digest: Self::digest(),
            armor_models: doc.armor_models,
            motion: doc.motion,
        })
    }
    /// Rig key of one armor slot shell (`head`/`chest`/`legs`/`feet`) of a humanoid family.
    pub(crate) fn armor_rig(&self, kind: &str, baby: bool, part: &str) -> Option<&str> {
        let armor = self.armor_models.get(kind)?;
        let set = if baby { &armor.baby } else { &armor.adult };
        set.get(part).map(String::as_str)
    }
    /// Geometry for a renderer rig key: explicit entry, else Solaris-authored model, else a
    /// placeholder box. Never fails; a missing model must not stop the frame.
    pub(crate) fn rig(&self, key: &str) -> &Rig {
        static FALLBACK: std::sync::LazyLock<Rig> =
            std::sync::LazyLock::new(|| solaris_models::placeholder(0.75, 0.75));
        self.rigs
            .get(key)
            .or_else(|| solaris_models::authored(key))
            .unwrap_or(&FALLBACK)
    }
    /// Whether a real (non-placeholder) model exists for the key.
    pub(crate) fn has_authored_rig(&self, key: &str) -> bool {
        self.rigs.get(key).is_some_and(|rig| !rig.is_placeholder())
            || (!self.rigs.contains_key(key) && solaris_models::authored(key).is_some())
    }
    pub fn accessor(&self, kind: &str, name: &str) -> Option<u8> {
        self.schemas
            .get(kind)?
            .accessors
            .get(name)
            .and_then(|v| u8::try_from(v[0]).ok())
    }
    pub fn default_skin(&self, uuid: Option<&str>) -> Result<&DefaultSkin> {
        let index = if let Some(uuid) = uuid {
            let value = uuid::Uuid::parse_str(uuid)
                .map_err(|_| "invalid profile UUID")?
                .as_u128();
            let h = (value as u64) ^ ((value >> 64) as u64);
            (((h >> 32) as u32 ^ h as u32) as i32).rem_euclid(18) as usize
        } else {
            6
        };
        Ok(&self.default_skins[index])
    }
    pub(crate) fn dimensions<'a>(
        &'a self,
        entity: &super::Entity,
    ) -> Result<(&'a Dimensions, f32)> {
        let schema = self
            .schemas
            .get(&entity.type_name)
            .ok_or("entity dimensions source not activated")?;
        let number = |key: &str| {
            self.accessor(&entity.type_name, key)
                .and_then(|id| entity.metadata_value(id))
                .and_then(super::Metadata::int)
        };
        let boolean = |key: &str| {
            self.accessor(&entity.type_name, key)
                .and_then(|id| entity.metadata_value(id))
                .and_then(super::Metadata::boolean)
                .unwrap_or(false)
        };
        let constant = |name: &str| match schema.dimension_constants.get(name) {
            Some(DimensionConstant::Dimensions(value)) => Some(value),
            _ => None,
        };
        let pose = self
            .poses
            .get(&(number("DATA_POSE").unwrap_or(0) as u8))
            .ok_or("unknown entity pose")?;
        let living = schema
            .hierarchy
            .iter()
            .any(|class| class == "net.minecraft.world.entity.LivingEntity");
        let mut attribute_scale = if living {
            let definition = schema
                .default_attributes
                .get("minecraft:scale")
                .ok_or("source scale attribute missing")?;
            entity
                .attributes
                .get(&definition.id)
                .copied()
                .unwrap_or(definition.value) as f32
        } else {
            1.
        };
        if !attribute_scale.is_finite() || attribute_scale <= 0. {
            return Err("invalid entity scale".into());
        }
        attribute_scale = match schema
            .method_owners
            .get("sanitizeScale")
            .map(String::as_str)
        {
            Some("net.minecraft.world.entity.animal.happyghast.HappyGhast") => {
                attribute_scale.min(1.)
            }
            Some("net.minecraft.world.entity.monster.Shulker") => attribute_scale.min(3.),
            Some("net.minecraft.world.entity.boss.enderdragon.EnderDragon") => 1.,
            _ => attribute_scale,
        };
        if living && pose == "SLEEPING" {
            return Ok((
                constant("SLEEPING_DIMENSIONS").ok_or("source sleeping dimensions missing")?,
                1.,
            ));
        }
        if entity.type_name == "minecraft:armor_stand" {
            let flags = number("DATA_CLIENT_FLAGS").unwrap_or(0);
            return Ok((
                if flags & 16 != 0 {
                    constant("MARKER_DIMENSIONS").ok_or("source marker dimensions missing")?
                } else if flags & 1 != 0 {
                    constant("BABY_DIMENSIONS")
                        .ok_or("source small armor stand dimensions missing")?
                } else {
                    &schema.dimensions
                },
                attribute_scale,
            ));
        }
        if let Some(DimensionConstant::Poses(poses)) = schema.dimension_constants.get("POSES") {
            return Ok((
                poses
                    .get(pose)
                    .or_else(|| constant("STANDING_DIMENSIONS"))
                    .ok_or("source avatar dimensions missing")?,
                attribute_scale,
            ));
        }
        let camel = schema
            .hierarchy
            .iter()
            .any(|class| class == "net.minecraft.world.entity.animal.camel.Camel");
        let age_scale = if boolean("DATA_BABY_ID") {
            match schema.method_owners.get("getAgeScale").map(String::as_str) {
                Some(
                    "net.minecraft.world.entity.animal.armadillo.Armadillo"
                    | "net.minecraft.world.entity.animal.camel.Camel",
                ) => 0.6,
                Some("net.minecraft.world.entity.animal.dolphin.Dolphin") => 0.65,
                Some("net.minecraft.world.entity.animal.goat.Goat") => 0.55,
                Some("net.minecraft.world.entity.animal.happyghast.HappyGhast") => 0.2375,
                Some("net.minecraft.world.entity.animal.turtle.Turtle") => 0.3,
                _ => 0.5,
            }
        } else {
            1.
        };
        if camel && pose == "SITTING" {
            return Ok((
                constant("SITTING_DIMENSIONS").ok_or("source camel sitting dimensions missing")?,
                age_scale * attribute_scale,
            ));
        }
        if pose == "LONG_JUMPING" && schema.class == "net.minecraft.world.entity.animal.goat.Goat" {
            let d =
                constant("LONG_JUMPING_DIMENSIONS").ok_or("source goat jump dimensions missing")?;
            return Ok((
                d,
                if d.fixed {
                    1.
                } else {
                    age_scale * attribute_scale
                },
            ));
        }
        if schema.class == "net.minecraft.world.entity.animal.sniffer.Sniffer"
            && schema
                .enum_ids
                .get("State")
                .and_then(|ids| ids.get(&number("DATA_STATE").unwrap_or(0)))
                .is_some_and(|v| v == "DIGGING")
        {
            let d = constant("DIGGING_DIMENSIONS")
                .ok_or("source sniffer digging dimensions missing")?;
            return Ok((
                d,
                if d.fixed {
                    1.
                } else {
                    age_scale * attribute_scale
                },
            ));
        }
        if let Some(baby) = constant("BABY_DIMENSIONS") {
            return Ok((
                if boolean("DATA_BABY_ID") {
                    baby
                } else {
                    &schema.dimensions
                },
                attribute_scale,
            ));
        }
        let owner = schema
            .method_owners
            .get("getDefaultDimensions")
            .map(String::as_str);
        if owner == Some("net.minecraft.world.entity.animal.equine.AbstractChestedHorse")
            && !boolean("DATA_BABY_ID")
        {
            return Ok((&schema.dimensions, attribute_scale));
        }
        let family_scale = match owner {
            Some("net.minecraft.world.entity.animal.fish.Pufferfish") => {
                match number("PUFF_STATE").unwrap_or(0) {
                    0 => 0.5,
                    1 => 0.7,
                    _ => 1.,
                }
            }
            Some("net.minecraft.world.entity.animal.fish.Salmon") => *schema
                .enum_numbers
                .get("Variant")
                .and_then(|variants| variants.get(&number("DATA_TYPE").unwrap_or(1)))
                .and_then(|fields| fields.get("boundingBoxScale"))
                .ok_or("source salmon numeric variant missing")?,
            Some("net.minecraft.world.entity.monster.cubemob.AbstractCubeMob") => {
                number("ID_SIZE").unwrap_or(1) as f32
            }
            Some("net.minecraft.world.entity.monster.Phantom") => {
                1. + 0.15 * number("ID_SIZE").unwrap_or(0) as f32
            }
            _ => 1.,
        };
        Ok((
            &schema.dimensions,
            if schema.dimensions.fixed {
                1.
            } else {
                age_scale * attribute_scale * family_scale
            },
        ))
    }
}
pub(super) fn part_lookup(bones: &[Bone]) -> BTreeMap<String, usize> {
    if bones.is_empty() {
        return BTreeMap::new();
    }
    let mut links = vec![(usize::MAX, usize::MAX); bones.len()];
    for child in (1..bones.len()).rev() {
        let parent = bones[child]
            .parent
            .expect("flattened non-root bone has parent");
        links[child].1 = links[parent].0;
        links[parent].0 = child;
    }
    fn insert_children(
        parent: usize,
        bones: &[Bone],
        links: &[(usize, usize)],
        names: &mut BTreeMap<String, usize>,
    ) {
        let mut child = links[parent].0;
        while child != usize::MAX {
            names.entry(bones[child].name.clone()).or_insert(child);
            child = links[child].1;
        }
        child = links[parent].0;
        while child != usize::MAX {
            insert_children(child, bones, links, names);
            child = links[child].1;
        }
    }
    let mut names = BTreeMap::new();
    names.insert(bones[0].name.clone(), 0);
    insert_children(0, bones, &links, &mut names);
    names
}
pub fn transform(pivot: Vec3, rotation: Vec3, scale: Vec3) -> Mat4 {
    Mat4::from_translation(pivot)
        * Mat4::from_rotation_z(rotation.z)
        * Mat4::from_rotation_y(rotation.y)
        * Mat4::from_rotation_x(rotation.x)
        * Mat4::from_scale(scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_received_fixture(kind: &str, direction: i32, metadata: &[u8]) -> crate::api::DynamicGeometry {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let variant = serde_json::json!({"width":1,"height":1,"asset_id":"test:canvas"});
        content.set_registries(BTreeMap::from([("minecraft:painting_variant".into(),
            vec![("test:canvas".into(), variant.clone())])]));
        let region = crate::protocol::content::TextureRegion {
            layer: 7, uv: [0., 0., 1., 1.], width: 16, height: 16,
        };
        for texture in ["test:painting/canvas", "minecraft:painting/back", "minecraft:test/frame",
            "minecraft:entity/minecart/minecart"] {
            content.insert_test_texture(texture, region);
        }
        // Owned box fixture, not copied vanilla geometry.
        let faces: serde_json::Map<String, Value> = ["north", "south", "east", "west", "up", "down"]
            .into_iter().map(|face| (face.into(), serde_json::json!({"texture":"#frame"}))).collect();
        let frame = serde_json::to_vec(&serde_json::json!({
            "textures":{"frame":"test/frame"},
            "elements":[{"from":[2,2,14],"to":[14,14,16],"faces":faces}]
        })).unwrap();
        for name in ["item_frame", "glow_item_frame"] {
            content.insert_test_resource(&format!("assets/minecraft/models/block/{name}.json"), &frame);
        }
        content.item_names.insert(1, "test:token".into());
        content.insert_test_item_components(1, BTreeMap::new());
        content.insert_test_resource("assets/test/models/token.json", &frame);
        content.insert_test_resource("assets/test/items/token.json",
            br#"{"model":{"type":"minecraft:model","model":"test:token"}}"#);
        let models = std::sync::Arc::new(Models::load(&content).unwrap());
        let mut store = super::super::Store::default();
        store.registries.insert("minecraft:painting_variant".into(),
            vec![("test:canvas".into(), variant)]);
        store.activate_models(models);
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0;16]);
        spawn.var(doc.entities[kind].protocol_id as i32);
        for coordinate in [4_f64, 4., 3.] {
            spawn.0.extend_from_slice(&coordinate.to_be_bytes());
        }
        spawn.0.extend_from_slice(&[0;4]);
        spawn.var(direction);
        store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap();
        if !metadata.is_empty() {
            store.apply(cb::SET_ENTITY_DATA, &mut Reader::new(metadata), &content, 1).unwrap();
        }
        let mut out = crate::api::DynamicGeometry::default();
        let stats = store.append_geometry(&content, [4.,4.,3.], [4.,5.,0.], [0.;2], 32.,
            1, 0., &|point| {
                // A painting's segment cells are available, but not its generic eye sample.
                Ok((kind != "minecraft:painting" || point.iter().all(|v| v.fract() == 0.))
                    .then_some([15.;2]))
            }, &mut out).unwrap();
        assert_eq!(stats.submitted, 1, "{kind}: {:?}", stats.unrendered);
        assert!(!out.vertices.is_empty(), "{kind}");
        out
    }

    #[test]
    fn entity_render_rc1_empty_item_frame_has_body_at_packet_anchor() {
        let out = render_received_fixture("minecraft:item_frame", 2, &[]);
        assert!(out.vertices.iter().all(|v| (0. ..=1.).contains(&v.position[2])));
        assert!(out.vertices.iter().any(|v| v.position[2] >= 0.9));
    }

    #[test]
    fn entity_render_rc1_empty_glow_item_frame_has_body() {
        render_received_fixture("minecraft:glow_item_frame", 5, &[]);
    }

    #[test]
    fn entity_render_rc1_painting_emits_mesh_from_received_spawn() {
        let out = render_received_fixture("minecraft:painting", 2, &[]);
        assert!(out.vertices.iter().all(|v| (0.9374..=1.0001).contains(&v.position[2])));
    }

    #[test]
    fn entity_render_rc1_minecart_mesh_is_above_rail_not_buried() {
        let out = render_received_fixture("minecraft:minecart", 0, &[]);
        let bottom = out.vertices.iter().map(|v| v.position[1]).fold(f32::INFINITY, f32::min);
        let top = out.vertices.iter().map(|v| v.position[1]).fold(f32::NEG_INFINITY, f32::max);
        assert!(bottom >= -0.001, "minecart bottom below received rail position: {bottom}");
        assert!(top > 0.5, "minecart top buried: {top}");
    }

    #[test]
    fn entity_render_rc1_frame_metadata_direction_overrides_spawn() {
        let out = render_received_fixture("minecraft:item_frame", 2, &[1,8,12,5,255]);
        assert!(out.vertices.iter().all(|v| v.position[0] >= -0.0001 && v.position[0] <= 0.1251));
    }

    #[test]
    fn entity_render_rc1_frame_emits_received_held_item_too() {
        let out = render_received_fixture("minecraft:item_frame", 2, &[1,9,7,1,1,0,0,10,1,2,255]);
        assert_eq!(out.vertices.len(), 48, "owned frame box plus owned held-item box");
    }

    #[test]
    fn wire_263_raid_omen_particle_list_metadata_packet() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        content.particle_names.insert(123, "minecraft:raid_omen".into());
        let models = std::sync::Arc::new(Models::load_state(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(models);
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0; 16]);
        spawn.var(120); // 26.3 slime
        spawn.0.extend_from_slice(&[0; 29]);
        assert!(store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap());
        // Entity 1, particle-list accessor 10, serializer 17, one particle 123,
        // followed by a byte metadata value to verify the particle consumes no options.
        let packet = [1, 10, 17, 1, 123, 8, 0, 42, 255];
        let mut reader = Reader::new(&packet);
        assert!(store.apply(cb::SET_ENTITY_DATA, &mut reader, &content, 2).unwrap());
        reader.done().unwrap();
        match &store.get(1).unwrap().metadata[&10] {
            super::super::metadata::Metadata::Particles(particles) => {
                assert_eq!(particles, &[serde_json::json!({
                    "type_id": 123, "type": "minecraft:raid_omen", "data": null
                })]);
            }
            other => panic!("unexpected metadata: {other:?}"),
        }
        assert_eq!(store.get(1).unwrap().metadata[&8].int(), Some(42));
    }

    #[test]
    fn wire_263_all_reported_simple_particles_decode_without_options() {
        use crate::protocol::{Reader, Writer};
        // The embedded facts are the sorted 26.3 SimpleParticleType registry report.
        let doc = Models::parse().unwrap();
        assert_eq!(doc.simple_particles.len(), 106);
        assert!(doc.simple_particles.windows(2).all(|pair| pair[0] < pair[1]));
        for name in [
            "minecraft:raid_omen", "minecraft:sulfur_bubbles", "minecraft:noxious_gas",
            "minecraft:noxious_gas_cloud", "minecraft:red_poplar_leaves",
            "minecraft:orange_poplar_leaves", "minecraft:yellow_poplar_leaves",
            "minecraft:sulfur_cube_goo",
        ] {
            assert!(doc.simple_particles.iter().any(|particle| particle == name), "{name}");
        }
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        let models = Models::load_state(&content).unwrap();
        for (id, name) in doc.simple_particles.iter().enumerate() {
            content.particle_names.insert(id as u32, name.clone());
            let mut packet = Writer::default();
            packet.0.extend_from_slice(&[8, 17, 1]);
            packet.var(id as i32);
            packet.0.extend_from_slice(&[9, 0, 42, 255]);
            let mut reader = Reader::new(&packet.0);
            let values = super::super::metadata::decode(&mut reader, &content, &models)
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            reader.done().unwrap();
            match &values[&8] {
                super::super::metadata::Metadata::Particles(particles) => {
                    assert_eq!(particles, &[serde_json::json!({
                        "type_id": id, "type": name, "data": null
                    })], "{name}");
                }
                other => panic!("{name}: unexpected metadata: {other:?}"),
            }
            assert_eq!(values[&9].int(), Some(42), "{name}");
        }
    }

    #[test]
    fn embedded_entity_facts_parse_and_carry_no_geometry() {
        let doc = Models::parse().unwrap();
        assert_eq!(doc.entities.len(), 161);
        assert_eq!(doc.motion.len(), doc.entities.len());
        assert_eq!(doc.families.len(), 129);
        assert_eq!(doc.default_skins.len(), 18);
        let zombie = &doc.entities["minecraft:zombie"];
        assert_eq!(zombie.protocol_id, 154);
        assert!(zombie.dimension_constants.contains_key("BABY_DIMENSIONS"));
        assert!(
            ENTITY_FACTS.len() < 1 << 20,
            "entity facts must stay a data table"
        );
        for geometry in ["\"roots\"", "\"quads\"", "\"polygons\"", "\"animations\""] {
            assert!(
                !ENTITY_FACTS.contains(geometry),
                "{geometry} in entity facts"
            );
        }
    }

    #[test]
    fn wire_263_slime_metadata_uses_regenerated_accessor_schema() {
        use crate::protocol::{Reader, Writer, packet_ids::play::clientbound as cb};
        let doc = Models::parse().unwrap();
        assert_eq!(doc.game_version, "26.3");
        assert_eq!(doc.entities["minecraft:slime"].accessors["ID_SIZE"], [18, 1]);
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        content.item_names.insert(0, "test:body".into());
        content.insert_test_item_components(0, BTreeMap::new());
        let models = std::sync::Arc::new(Models::load_state(&content).unwrap());
        let mut store = super::super::Store::default();
        store.activate_models(std::sync::Arc::clone(&models));
        let mut spawn = Writer::default();
        spawn.var(1);
        spawn.0.extend_from_slice(&[0; 16]); // UUID
        spawn.var(120); // reports/registries.json: minecraft:slime
        spawn.0.extend_from_slice(&[0; 29]); // xyz, zero LP Vec3, rotations, spawn data
        assert!(store.apply(cb::ADD_ENTITY, &mut Reader::new(&spawn.0), &content, 1).unwrap());
        // entity=1, accessor=18, INT serializer=1, size=4, end=255.
        assert!(store.apply(cb::SET_ENTITY_DATA, &mut Reader::new(&[1, 18, 1, 4, 255]), &content, 2).unwrap());
        assert_eq!(store.get(1).unwrap().metadata[&18].int(), Some(4));
        assert_eq!(models.dimensions(store.get(1).unwrap()).unwrap().1, 4.);
        // 26.3 SulfurCube's vertical walk animation is hasBodyItem(), not the
        // removed FlyingAnimal interface or a fabricated constant.
        let sulfur = &models.motion["minecraft:sulfur_cube"];
        assert!(!sulfur.walk_uses_y(store.get(1).unwrap()));
        let body = crate::inventory::read_stack(&mut Reader::new(&[1, 0, 0, 0]), &content).unwrap();
        store.get_mut(1).unwrap().equipment[6] = Some(body);
        assert!(sulfur.walk_uses_y(store.get(1).unwrap()));
        store.get_mut(1).unwrap().equipment[6] = None;
        assert!(!sulfur.walk_uses_y(store.get(1).unwrap()));
    }

    #[test]
    fn regression_263_cushion_and_painting_metadata() {
        use crate::protocol::{Reader, Writer};
        let doc = Models::parse().unwrap();
        assert_eq!(doc.entities["minecraft:cushion"].accessors["DATA_COLOR"], [8, 43]);
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id, name.clone())).collect();
        content.set_registries(BTreeMap::from([("minecraft:painting_variant".into(),
            (0..3).map(|id| (format!("test:painting_{id}"), serde_json::json!({}))).collect())]));
        let models = Models::load_state(&content).unwrap();
        let mut reader = Reader::new(&[8, 43, 14, 255]);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&8].serializer_id(), 43);
        assert_eq!(values[&8].int(), Some(14));
        let mut reader = Reader::new(&[9, 34, 1, 255]);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&9].serializer_id(), 34);
        assert_eq!(values[&9].int(), Some(0), "holder reference 1 names registry entry 0");
        let last = content.registry_entries("minecraft:painting_variant").unwrap().len() as i32;
        let mut packet = Writer::default();
        packet.0.extend_from_slice(&[9, 34]);
        packet.var(last);
        packet.0.push(255);
        let mut reader = Reader::new(&packet.0);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&9].int(), Some(last - 1));
        let mut packet = Writer::default();
        packet.0.extend_from_slice(&[9, 34, 0, 2, 3]); // inline holder, width, height
        packet.string("test:painting");
        packet.0.extend_from_slice(&[1, 8, 0, 1, b'T', 1, 8, 0, 1, b'A', 10, 0, 5, 255]);
        let mut reader = Reader::new(&packet.0);
        let values = super::super::metadata::decode(&mut reader, &content, &models).unwrap();
        reader.done().unwrap();
        assert_eq!(values[&9].serializer_id(), 34);
        assert_eq!(values[&9].int(), None);
        assert_eq!(values[&10].int(), Some(5), "inline holder consumes exactly its fields");
        let value = serde_json::to_value(&values[&9]).unwrap();
        assert_eq!(value["value"]["width"], 2);
        assert_eq!(value["value"]["height"], 3);
        assert_eq!(value["value"]["asset_id"], "test:painting");
        assert_eq!(value["value"]["title"], "T");
        assert_eq!(value["value"]["author"], "A");
    }

    #[test]
    fn entity_schema_rejects_same_names_at_stale_protocol_ids() {
        let doc = Models::parse().unwrap();
        let mut content = Content::test_cube();
        content.entity_names = doc.entities.iter()
            .map(|(name, schema)| (schema.protocol_id + 1, name.clone())).collect();
        assert!(Models::check_registry(&doc, &content).is_err());
    }

    #[test]
    fn armor_models_resolve_per_slot_for_adult_and_baby() {
        let doc = Models::parse().unwrap();
        let zombie = &doc.armor_models["minecraft:zombie"];
        for part in ["head", "chest", "legs", "feet"] {
            assert!(zombie.adult.contains_key(part), "adult {part}");
            assert!(zombie.baby.contains_key(part), "baby {part}");
        }
    }

    #[test]
    fn dimension_constants_preserve_decimal_dimensions_and_attachments() {
        let dimensions = r#"{"width":0.2,"height":0.2,"eye_height":0.2,"fixed":true,"attachments":{"NAME_TAG":[[0.0,0.20000000298023224,0.0]]}}"#;
        let value: DimensionConstant = serde_json::from_str(dimensions).unwrap();
        let DimensionConstant::Dimensions(value) = value else {
            panic!("dimension variant")
        };
        assert_eq!(value.width, 0.2);
        assert_eq!(
            value.attachments["NAME_TAG"][0],
            [0., 0.20000000298023224, 0.]
        );
        let poses: DimensionConstant =
            serde_json::from_str(&format!(r#"{{"SLEEPING":{dimensions}}}"#)).unwrap();
        let DimensionConstant::Poses(poses) = poses else {
            panic!("pose variant")
        };
        assert_eq!(poses["SLEEPING"].eye_height, 0.2);
        assert!(poses["SLEEPING"].fixed);
    }
}
