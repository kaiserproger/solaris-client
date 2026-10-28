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
        if doc.schema != FACTS_SCHEMA || doc.game_version != "26.1.2" {
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
        for name in doc.entities.keys() {
            if !content.entity_names.values().any(|candidate| candidate == name) {
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
            simple_particles: Vec::new(),
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
            Some("net.minecraft.world.entity.monster.Slime") => {
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

    #[test]
    fn embedded_entity_facts_parse_and_carry_no_geometry() {
        let doc = Models::parse().unwrap();
        assert_eq!(doc.entities.len(), 157);
        assert_eq!(doc.motion.len(), doc.entities.len());
        assert_eq!(doc.families.len(), 129);
        assert_eq!(doc.default_skins.len(), 18);
        let zombie = &doc.entities["minecraft:zombie"];
        assert_eq!(zombie.protocol_id, 150);
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
