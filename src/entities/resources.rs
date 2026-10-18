use super::render_living::{AnimationDefinition, MotionFacts};
use crate::protocol::{Result, content::Content};
use glam::{Mat4, Vec3};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

#[derive(Deserialize)]
struct Document {
    schema: u32,
    game_version: String,
    client_sha256: String,
    roots: BTreeMap<String, Part>,
    entities: BTreeMap<String, EntitySchema>,
    families: BTreeMap<String, Family>,
    simple_particles: Vec<String>,
    default_skins: Vec<DefaultSkin>,
    poses: BTreeMap<u8, String>,
    attributes: BTreeMap<String, AttributeBounds>,
    #[serde(default)]
    builtin_variants: BTreeMap<String, BTreeMap<String, Value>>,
}
#[derive(Deserialize)]
struct StateDocument {
    schema: u32,
    game_version: String,
    client_sha256: String,
    entities: BTreeMap<String, EntitySchema>,
    poses: BTreeMap<u8, String>,
    attributes: BTreeMap<String, AttributeBounds>,
    #[serde(default)]
    builtin_variants: BTreeMap<String, BTreeMap<String, Value>>,
}
#[derive(Deserialize)]
struct PresentationDocument {
    schema: u32,
    game_version: String,
    client_sha256: String,
    roots: BTreeMap<String, Part>,
    families: BTreeMap<String, Value>,
    animations: BTreeMap<String, AnimationDefinition>,
}
#[derive(Deserialize)]
struct MotionDocument {
    schema: u32,
    game_version: String,
    client_sha256: String,
    entities: BTreeMap<String, MotionFacts>,
}
#[derive(Deserialize)]
struct Part {
    name: String,
    pivot: [f32; 3],
    rotation: [f32; 3],
    scale: [f32; 3],
    quads: Vec<[[f32; 5]; 4]>,
    normals: Vec<[f32; 3]>,
    children: Vec<Part>,
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
    pub(crate) presentation: BTreeMap<String, Value>,
    pub(crate) animations: BTreeMap<String, AnimationDefinition>,
    pub(crate) motion: BTreeMap<String, MotionFacts>,
}
impl Models {
    pub(crate) fn load_state(path: &Path, content: &Content) -> Result<Self> {
        let bytes = crate::protocol::content::bounded_bytes(path, 64 << 20)?;
        let provenance: Value = serde_json::from_slice(&crate::protocol::content::bounded_bytes(
            &path.with_file_name("provenance.json"),
            1 << 20,
        )?)
        .map_err(|e| e.to_string())?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if provenance["models_sha256"].as_str() != Some(&digest) {
            return Err("entity model cache digest mismatch".into());
        }
        // Deserialize protocol/dimensions metadata only. Unknown rig/texture fields
        // are skipped by serde without allocating their render geometry.
        let doc: StateDocument = serde_json::from_slice(&bytes)
            .map_err(|e| format!("entity state cache schema: {e}"))?;
        if doc.schema != 2
            || doc.game_version != "26.1.2"
            || provenance["client_sha256"].as_str() != Some(&doc.client_sha256)
            || doc.entities.len() > 1024
            || doc.poses.len() != 18
        {
            return Err("entity state source/target mismatch".into());
        }
        for (name, schema) in &doc.entities {
            if content.entity_names.get(&schema.protocol_id) != Some(name)
                || !schema.width.is_finite()
                || schema.width < 0.
                || !schema.height.is_finite()
                || schema.height < 0.
                || !schema.eye_height.is_finite()
            {
                return Err(format!(
                    "entity state registry/dimensions mismatch for {name}"
                ));
            }
        }
        let motion_path = std::env::var_os("SOLARIS_CLIENT_ENTITY_MOTION_FACTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| path.with_file_name("motion-facts.json"));
        let motion_bytes = crate::protocol::content::bounded_bytes(&motion_path, 8 << 20)?;
        let motion_digest = format!("{:x}", Sha256::digest(&motion_bytes));
        if provenance["motion_sha256"].as_str() != Some(&motion_digest) {
            return Err("entity motion cache digest mismatch".into());
        }
        let motion: MotionDocument = serde_json::from_slice(&motion_bytes)
            .map_err(|e| format!("entity motion cache schema: {e}"))?;
        if motion.schema != 1
            || motion.game_version != doc.game_version
            || motion.client_sha256 != doc.client_sha256
            || motion.entities.len() != doc.entities.len()
            || motion.entities.iter().any(|(kind, facts)| {
                doc.entities
                    .get(kind)
                    .is_none_or(|schema| schema.class != facts.class)
            })
        {
            return Err("entity motion source or holder coverage mismatch".into());
        }
        Ok(Self {
            rigs: BTreeMap::new(),
            families: BTreeMap::new(),
            simple_particles: Vec::new(),
            default_skins: Vec::new(),
            schemas: doc.entities,
            poses: doc.poses,
            builtin_variants: doc.builtin_variants,
            attribute_ids: doc
                .attributes
                .iter()
                .map(|(name, value)| (value.id, name.clone()))
                .collect(),
            attributes: doc.attributes,
            digest,
            presentation: BTreeMap::new(),
            animations: BTreeMap::new(),
            motion: motion.entities,
        })
    }
    pub fn load(path: &Path, content: &Content) -> Result<Self> {
        let bytes = crate::protocol::content::bounded_bytes(path, 64 << 20)?;
        let provenance_path = path.with_file_name("provenance.json");
        let provenance: Value = serde_json::from_slice(&crate::protocol::content::bounded_bytes(
            &provenance_path,
            1 << 20,
        )?)
        .map_err(|e| e.to_string())?;
        let digest = format!("{:x}", Sha256::digest(&bytes));
        if provenance["models_sha256"].as_str() != Some(&digest) {
            return Err("entity model cache digest mismatch".into());
        }
        let mut doc: Document = serde_json::from_slice(&bytes)
            .map_err(|e| format!("entity model cache schema: {e}"))?;
        if doc.schema != 2
            || doc.game_version != "26.1.2"
            || provenance["client_sha256"].as_str() != Some(&doc.client_sha256)
        {
            return Err("entity model source/target mismatch".into());
        }
        if doc.roots.len() > 1024 || doc.entities.len() > 1024 {
            return Err("entity model catalog bound".into());
        }
        if doc.default_skins.len() != 18 || doc.poses.len() != 18 {
            return Err("source skin/pose catalog mismatch".into());
        }
        for (name, schema) in &doc.entities {
            if content.entity_names.get(&schema.protocol_id) != Some(name) {
                return Err(format!("entity registry/cache mismatch for {name}"));
            }
            if !schema.width.is_finite()
                || !schema.height.is_finite()
                || schema.width < 0.
                || schema.height < 0.
                || !schema.eye_height.is_finite()
            {
                return Err("entity dimensions invalid".into());
            }
        }
        let presentation_path = std::env::var_os("SOLARIS_CLIENT_ENTITY_PRESENTATION")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| path.with_file_name("presentation.json"));
        let presentation_bytes =
            crate::protocol::content::bounded_bytes(&presentation_path, 64 << 20)?;
        let presentation_digest = format!("{:x}", Sha256::digest(&presentation_bytes));
        if provenance["presentation_sha256"].as_str() != Some(&presentation_digest) {
            return Err("entity presentation cache digest mismatch".into());
        }
        let presentation: PresentationDocument = serde_json::from_slice(&presentation_bytes)
            .map_err(|error| format!("entity presentation cache schema: {error}"))?;
        if presentation.schema != 2
            || presentation.game_version != doc.game_version
            || presentation.client_sha256 != doc.client_sha256
            || presentation.roots.len() > 1024
            || presentation.families.len() != doc.entities.len()
            || presentation
                .families
                .keys()
                .any(|kind| !doc.entities.contains_key(kind))
        {
            return Err("entity presentation source or holder coverage mismatch".into());
        }
        for (name, root) in presentation.roots {
            if doc.roots.insert(name, root).is_some() {
                return Err("duplicate entity presentation rig identifier".into());
            }
        }
        let motion_path = std::env::var_os("SOLARIS_CLIENT_ENTITY_MOTION_FACTS")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| path.with_file_name("motion-facts.json"));
        let motion_bytes = crate::protocol::content::bounded_bytes(&motion_path, 8 << 20)?;
        let motion_digest = format!("{:x}", Sha256::digest(&motion_bytes));
        if provenance["motion_sha256"].as_str() != Some(&motion_digest) {
            return Err("entity motion cache digest mismatch".into());
        }
        let motion: MotionDocument = serde_json::from_slice(&motion_bytes)
            .map_err(|error| format!("entity motion cache schema: {error}"))?;
        if motion.schema != 1
            || motion.game_version != doc.game_version
            || motion.client_sha256 != doc.client_sha256
            || motion.entities.len() != doc.entities.len()
            || motion.entities.iter().any(|(kind, facts)| {
                doc.entities
                    .get(kind)
                    .is_none_or(|schema| schema.class != facts.class)
            })
        {
            return Err("entity motion source or holder coverage mismatch".into());
        }
        let mut resource_digest = Sha256::new();
        resource_digest.update(&bytes);
        resource_digest.update(&presentation_bytes);
        resource_digest.update(&motion_bytes);
        let digest = format!("{:x}", resource_digest.finalize());
        let mut rigs = BTreeMap::new();
        let mut total_quads = 0;
        for (name, root) in doc.roots {
            let mut bones = Vec::new();
            flatten(root, None, 0, &mut bones, &mut total_quads)?;
            let part_by_name = part_lookup(&bones);
            rigs.insert(
                name,
                Rig {
                    bones,
                    part_by_name,
                },
            );
        }
        Ok(Self {
            rigs,
            schemas: doc.entities,
            families: doc.families,
            builtin_variants: doc.builtin_variants,
            simple_particles: doc.simple_particles,
            default_skins: doc.default_skins,
            poses: doc.poses,
            attribute_ids: doc
                .attributes
                .iter()
                .map(|(name, value)| (value.id, name.clone()))
                .collect(),
            attributes: doc.attributes,
            digest,
            presentation: presentation.families,
            animations: presentation.animations,
            motion: motion.entities,
        })
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
fn flatten(
    part: Part,
    parent: Option<usize>,
    depth: usize,
    bones: &mut Vec<Bone>,
    total: &mut usize,
) -> Result<()> {
    if depth > 32 || bones.len() >= 1024 {
        return Err("entity rig hierarchy bound".into());
    }
    *total += part.quads.len();
    if *total > 200_000 {
        return Err("entity rig geometry bound".into());
    }
    if part
        .pivot
        .iter()
        .chain(part.rotation.iter())
        .chain(part.scale.iter())
        .any(|n| !n.is_finite())
        || part
            .quads
            .iter()
            .flatten()
            .flatten()
            .any(|n| !n.is_finite())
        || part.normals.len() != part.quads.len()
        || part.normals.iter().any(|normal| {
            normal.iter().any(|value| !value.is_finite())
                || (normal.iter().map(|value| value * value).sum::<f32>() - 1.0).abs() > 1e-5
        })
    {
        return Err("nonfinite entity rig geometry".into());
    }
    let role = match part.name.as_str() {
        "head" | "head_main" => BoneRole::Head,
        "right_arm" => BoneRole::RightArm,
        "left_arm" => BoneRole::LeftArm,
        "right_leg" | "right_hind_leg" | "left_front_leg" => BoneRole::RightLeg,
        "left_leg" | "left_hind_leg" | "right_front_leg" => BoneRole::LeftLeg,
        "left_wing" | "right_wing" => BoneRole::Wing,
        _ => BoneRole::Fixed,
    };
    let index = bones.len();
    bones.push(Bone {
        parent,
        pivot: Vec3::from_array(part.pivot),
        rotation: Vec3::from_array(part.rotation),
        scale: Vec3::from_array(part.scale),
        role,
        name: part.name,
        quads: part.quads,
        normals: part.normals,
    });
    for child in part.children {
        flatten(child, Some(index), depth + 1, bones, total)?;
    }
    Ok(())
}
fn part_lookup(bones: &[Bone]) -> BTreeMap<String, usize> {
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
