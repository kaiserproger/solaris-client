//! Solaris-authored entity geometry (`assets/solaris/models/entity/*.json`).
//!
//! Model files are embedded at build time (`build.rs`) and baked into [`Rig`]s on first use.
//! Geometry is authored by the Solaris project and is UV-compatible with the vanilla entity
//! texture layouts, which the player supplies at runtime. Nothing here reads Mojang data.
//!
//! Coordinate conventions (identical to the renderer's model space):
//! * lengths are pixels (1/16 block); +Y points down; the entity's feet are at `y = 24`;
//!   negative Z is the entity's front;
//! * a bone `pivot` is relative to its parent bone's pivot, a cube `origin` is relative to
//!   the owning bone's pivot;
//! * bone `rotation` is in degrees, applied Z then Y then X about the pivot;
//! * cubes use the classic box unwrap: with `uv = [u, v]` and size `w x h x d` the strip
//!   `(u, v+d) .. (u+2d+2w, v+d+h)` carries right/front/left/back and `(u+d, v) ..
//!   (u+d+2w, v+d)` carries top/bottom.
use super::resources::{Bone, BoneRole, Rig};
use crate::protocol::Result;
use glam::Vec3;
use serde::Deserialize;
use std::{collections::BTreeMap, sync::LazyLock};

include!(concat!(env!("OUT_DIR"), "/solaris_models_embedded.rs"));

pub(crate) const SCHEMA: u32 = 1;
pub(crate) const PLACEHOLDER_BONE: &str = "solaris_placeholder";
const MAX_BONES: usize = 256;
const MAX_CUBES: usize = 2048;
const MAX_DEPTH: usize = 16;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ModelFile {
    pub schema: u32,
    pub id: String,
    pub texture: TextureSpec,
    pub bones: Vec<BoneSpec>,
    #[serde(default)]
    pub roles: Roles,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TextureSpec {
    /// Path below `assets/minecraft/textures/`, including `.png`.
    pub path: String,
    pub size: [u32; 2],
}
/// Bone names driven by the generic locomotion/look animation. Quadrupeds list their
/// diagonal gait pairs explicitly: `right_leg` and `left_leg` swing in opposite phase.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct Roles {
    pub head: Vec<String>,
    pub right_arm: Vec<String>,
    pub left_arm: Vec<String>,
    pub right_leg: Vec<String>,
    pub left_leg: Vec<String>,
    pub wing: Vec<String>,
    pub crossed_arms: Vec<String>,
    pub nose: Vec<String>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BoneSpec {
    pub name: String,
    #[serde(default)]
    pub pivot: [f32; 3],
    /// Degrees.
    #[serde(default)]
    pub rotation: [f32; 3],
    #[serde(default = "one3")]
    pub scale: [f32; 3],
    #[serde(default)]
    pub cubes: Vec<CubeSpec>,
    #[serde(default)]
    pub bones: Vec<BoneSpec>,
}
fn one3() -> [f32; 3] {
    [1.; 3]
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CubeSpec {
    pub origin: [f32; 3],
    pub size: [f32; 3],
    #[serde(default)]
    pub uv: [f32; 2],
    #[serde(default)]
    pub mirror: bool,
    #[serde(default)]
    pub inflate: f32,
    /// Optional per-face texture rectangles `[u0, v0, u1, v1]` in pixels replacing the box
    /// unwrap of that face (for flat planes and non-box layouts). Swapped corners flip.
    #[serde(default)]
    pub faces: BTreeMap<Face, [f32; 4]>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Face {
    Up,
    Down,
    /// Faces the entity's front (-Z).
    Front,
    Back,
    /// Model -X side.
    Right,
    /// Model +X side.
    Left,
}

impl ModelFile {
    pub(crate) fn parse(source: &str) -> Result<Self> {
        let model: Self = serde_json::from_str(source).map_err(|e| e.to_string())?;
        if model.schema != SCHEMA {
            return Err(format!(
                "solaris model {} schema {}",
                model.id, model.schema
            ));
        }
        Ok(model)
    }
    /// Every rectangle the cubes sample, in pixels: `(bone, [u0, v0, u1, v1])`.
    pub(crate) fn uv_rects(&self) -> Vec<(String, [f32; 4])> {
        fn walk(bone: &BoneSpec, out: &mut Vec<(String, [f32; 4])>) {
            for cube in &bone.cubes {
                for (_, rect) in face_rects(cube) {
                    out.push((bone.name.clone(), rect));
                }
            }
            for child in &bone.bones {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        for bone in &self.bones {
            walk(bone, &mut out);
        }
        out
    }
    pub(crate) fn bone_names(&self) -> Vec<&str> {
        fn walk<'a>(bone: &'a BoneSpec, out: &mut Vec<&'a str>) {
            out.push(&bone.name);
            for child in &bone.bones {
                walk(child, out);
            }
        }
        let mut out = Vec::new();
        for bone in &self.bones {
            walk(bone, &mut out);
        }
        out
    }
    pub(crate) fn bake(&self) -> Result<Rig> {
        let [tw, th] = self.texture.size;
        if tw == 0 || th == 0 || tw > 4096 || th > 4096 {
            return Err(format!("solaris model {} texture size", self.id));
        }
        let mut bones = vec![Bone {
            parent: None,
            pivot: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
            role: BoneRole::Fixed,
            name: "root".into(),
            quads: Vec::new(),
            normals: Vec::new(),
        }];
        let mut cubes = 0;
        for bone in &self.bones {
            self.flatten(bone, 0, 1, &mut bones, &mut cubes)?;
        }
        let part_by_name = super::resources::part_lookup(&bones);
        for (role, names) in [
            (BoneRole::Head, &self.roles.head),
            (BoneRole::RightArm, &self.roles.right_arm),
            (BoneRole::LeftArm, &self.roles.left_arm),
            (BoneRole::RightLeg, &self.roles.right_leg),
            (BoneRole::LeftLeg, &self.roles.left_leg),
            (BoneRole::Wing, &self.roles.wing),
            (BoneRole::CrossedArms, &self.roles.crossed_arms),
            (BoneRole::Nose, &self.roles.nose),
        ] {
            for name in names {
                let index = part_by_name
                    .get(name)
                    .ok_or_else(|| format!("solaris model {} role bone {name} missing", self.id))?;
                bones[*index].role = role;
            }
        }
        Ok(Rig {
            bones,
            part_by_name,
        })
    }
    fn flatten(
        &self,
        spec: &BoneSpec,
        parent: usize,
        depth: usize,
        bones: &mut Vec<Bone>,
        cubes: &mut usize,
    ) -> Result<()> {
        if depth > MAX_DEPTH || bones.len() >= MAX_BONES {
            return Err(format!("solaris model {} bone bounds", self.id));
        }
        *cubes += spec.cubes.len();
        if *cubes > MAX_CUBES {
            return Err(format!("solaris model {} cube bounds", self.id));
        }
        let [tw, th] = self.texture.size.map(|v| v as f32);
        let mut quads = Vec::with_capacity(spec.cubes.len() * 6);
        let mut normals = Vec::with_capacity(spec.cubes.len() * 6);
        for cube in &spec.cubes {
            for (quad, normal) in cube_quads(cube, tw, th) {
                quads.push(quad);
                normals.push(normal);
            }
        }
        if spec
            .pivot
            .iter()
            .chain(&spec.rotation)
            .chain(&spec.scale)
            .chain(quads.iter().flatten().flatten())
            .any(|v| !v.is_finite())
        {
            return Err(format!(
                "solaris model {} nonfinite bone {}",
                self.id, spec.name
            ));
        }
        let index = bones.len();
        bones.push(Bone {
            parent: Some(parent),
            pivot: Vec3::from_array(spec.pivot) / 16.,
            rotation: Vec3::from_array(spec.rotation) * std::f32::consts::PI / 180.,
            scale: Vec3::from_array(spec.scale),
            role: BoneRole::Fixed,
            name: spec.name.clone(),
            quads,
            normals,
        });
        for child in &spec.bones {
            self.flatten(child, index, depth + 1, bones, cubes)?;
        }
        Ok(())
    }
}

/// Texture rectangle (pixels) of every face of a cube before mirroring.
fn face_rects(cube: &CubeSpec) -> [(Face, [f32; 4]); 6] {
    let [u, v] = cube.uv;
    let [w, h, d] = cube.size;
    let default = |face: Face| match face {
        Face::Up => [u + d, v, u + d + w, v + d],
        Face::Down => [u + d + w, v, u + d + 2. * w, v + d],
        Face::Right => [u, v + d, u + d, v + d + h],
        Face::Front => [u + d, v + d, u + d + w, v + d + h],
        Face::Left => [u + d + w, v + d, u + 2. * d + w, v + d + h],
        Face::Back => [u + 2. * d + w, v + d, u + 2. * d + 2. * w, v + d + h],
    };
    [
        Face::Up,
        Face::Down,
        Face::Right,
        Face::Front,
        Face::Left,
        Face::Back,
    ]
    .map(|face| {
        (
            face,
            cube.faces
                .get(&face)
                .copied()
                .unwrap_or_else(|| default(face)),
        )
    })
}

/// Six textured quads in blocks relative to the bone pivot.
///
/// Corner order, UV assignment and winding follow the renderer's cutout pipeline: every quad
/// is counter-clockwise seen from outside, so `cross(v1 - v0, v2 - v0)` equals its normal.
fn cube_quads(cube: &CubeSpec, tw: f32, th: f32) -> Vec<([[f32; 5]; 4], [f32; 3])> {
    let [ox, oy, oz] = cube.origin;
    let [w, h, d] = cube.size;
    let i = cube.inflate;
    let (x0, y0, z0) = (ox - i, oy - i, oz - i);
    let (x1, y1, z1) = (ox + w + i, oy + h + i, oz + d + i);
    let rects = face_rects(cube);
    let rect = |face: Face| rects.iter().find(|(f, _)| *f == face).unwrap().1;
    // (face, normal, corners, uv corner selector) — uv corner: (use u1?, use v1?)
    type Corner = (f32, f32, f32, bool, bool);
    let faces: [(Face, [f32; 3], [Corner; 4]); 6] = [
        (
            Face::Up,
            [0., -1., 0.],
            [
                (x1, y0, z1, true, false),
                (x0, y0, z1, false, false),
                (x0, y0, z0, false, true),
                (x1, y0, z0, true, true),
            ],
        ),
        (
            Face::Down,
            [0., 1., 0.],
            [
                (x1, y1, z0, true, true),
                (x0, y1, z0, false, true),
                (x0, y1, z1, false, false),
                (x1, y1, z1, true, false),
            ],
        ),
        (
            Face::Right,
            [-1., 0., 0.],
            [
                (x0, y0, z0, true, false),
                (x0, y0, z1, false, false),
                (x0, y1, z1, false, true),
                (x0, y1, z0, true, true),
            ],
        ),
        (
            Face::Front,
            [0., 0., -1.],
            [
                (x1, y0, z0, true, false),
                (x0, y0, z0, false, false),
                (x0, y1, z0, false, true),
                (x1, y1, z0, true, true),
            ],
        ),
        (
            Face::Left,
            [1., 0., 0.],
            [
                (x1, y0, z1, true, false),
                (x1, y0, z0, false, false),
                (x1, y1, z0, false, true),
                (x1, y1, z1, true, true),
            ],
        ),
        (
            Face::Back,
            [0., 0., 1.],
            [
                (x0, y0, z1, true, false),
                (x1, y0, z1, false, false),
                (x1, y1, z1, false, true),
                (x0, y1, z1, true, true),
            ],
        ),
    ];
    let centre_x = (x0 + x1) * 0.5;
    faces
        .into_iter()
        .map(|(face, normal, corners)| {
            let [ru0, rv0, ru1, rv1] = rect(face);
            let mut quad = corners.map(|(x, y, z, use_u1, use_v1)| {
                let px = if cube.mirror { 2. * centre_x - x } else { x };
                [
                    px / 16.,
                    y / 16.,
                    z / 16.,
                    if use_u1 { ru1 } else { ru0 } / tw,
                    if use_v1 { rv1 } else { rv0 } / th,
                ]
            });
            let mut normal = normal;
            if cube.mirror {
                // Reflection reverses winding; restore counter-clockwise order.
                quad.reverse();
                normal[0] = -normal[0];
            }
            (quad, normal)
        })
        .collect()
}

/// Maps a renderer rig key to the id of the Solaris model that supplies its geometry.
///
/// Keys look like `minecraft:cow#renderer_main`, `minecraft:cow_baby#main`,
/// `minecraft:oak_chest_boat#renderer/model` or
/// `minecraft:zombie#layer3/babyModelSet/head`; the qualifier after `#` never selects
/// geometry except for the armor layer slots.
pub(crate) fn model_id(key: &str) -> String {
    let key = key.strip_prefix("minecraft:").unwrap_or(key);
    let (name, qualifier) = key.split_once('#').unwrap_or((key, ""));
    let slot = |q: &str| -> Option<&'static str> {
        Some(match q.rsplit('/').next()? {
            "head" | "helmet" => "head",
            "chest" | "chestplate" => "chest",
            "legs" | "leggings" => "legs",
            "feet" | "boots" => "feet",
            _ => return None,
        })
    };
    let baby_set = qualifier.contains("babyModelSet") || name == "armor_stand_small";
    if let Some(slot) = slot(qualifier).filter(|_| {
        qualifier.contains("/modelSet/")
            || qualifier.contains("/babyModelSet/")
            || name.starts_with("armor_stand")
    }) {
        return format!("armor_{}{slot}", if baby_set { "baby_" } else { "" });
    }
    // The conduit block entity is drawn from several sub-models selected by the qualifier.
    if name == "conduit" && !qualifier.is_empty() {
        return format!("conduit_{qualifier}");
    }
    if qualifier.contains("waterPatch") {
        return "boat_water_patch".into();
    }
    for suffix in ["chest_boat", "chest_raft", "boat", "raft", "minecart"] {
        if name.ends_with(&format!("_{suffix}")) || name == suffix {
            return suffix.into();
        }
    }
    // Same projectile mesh, different renderer family.
    if name == "breeze_wind_charge" {
        return "wind_charge".into();
    }
    name.replace('/', "_")
}

pub(crate) struct Catalog {
    rigs: BTreeMap<String, Rig>,
    textures: BTreeMap<String, String>,
}
impl Catalog {
    /// A malformed or duplicate file is reported once and skipped: its entity then renders the
    /// magenta placeholder instead of taking the client down.
    fn build() -> Self {
        Self::from_sources(EMBEDDED)
    }
    fn from_sources(sources: &[(&str, &str)]) -> Self {
        let mut rigs = BTreeMap::new();
        let mut textures = BTreeMap::new();
        for (file, source) in sources {
            let baked = ModelFile::parse(source).and_then(|model| {
                let rig = model.bake()?;
                Ok((model, rig))
            });
            let (model, rig) = match baked {
                Ok(baked) => baked,
                Err(error) => {
                    eprintln!("solaris entity model {file} rejected: {error}");
                    continue;
                }
            };
            if rigs.contains_key(&model.id) {
                eprintln!(
                    "solaris entity model {file} rejected: duplicate id {}",
                    model.id
                );
                continue;
            }
            textures.insert(model.id.clone(), model.texture.path.clone());
            rigs.insert(model.id, rig);
        }
        Self { rigs, textures }
    }
}
static CATALOG: LazyLock<Catalog> = LazyLock::new(Catalog::build);
fn catalog() -> &'static Catalog {
    &CATALOG
}
/// Authored rig for a renderer key, if Solaris ships one.
pub(crate) fn authored(key: &str) -> Option<&'static Rig> {
    catalog().rigs.get(&model_id(key))
}
/// Default vanilla texture (below `textures/`) of the authored model for `key`.
pub(crate) fn default_texture(key: &str) -> Option<&'static str> {
    catalog().textures.get(&model_id(key)).map(String::as_str)
}
/// Digest input proving which authored files this binary carries.
pub(crate) fn embedded_digest() -> String {
    use sha2::{Digest, Sha256};
    let mut sha = Sha256::new();
    for (name, source) in EMBEDDED {
        sha.update(name.as_bytes());
        sha.update(source.as_bytes());
    }
    format!("{:x}", sha.finalize())
}

/// Visible stand-in for a missing model: a magenta-tinted box with the entity's bounding
/// dimensions, standing on the entity's feet.
pub(crate) fn placeholder(width: f32, height: f32) -> Rig {
    let (w, h) = (
        (width * 16.).clamp(2., 256.),
        (height * 16.).clamp(2., 256.),
    );
    let cube = CubeSpec {
        origin: [-w * 0.5, 24. - h, -w * 0.5],
        size: [w, h, w],
        uv: [0., 0.],
        mirror: false,
        inflate: 0.,
        faces: Face::ALL
            .into_iter()
            .map(|face| (face, [0., 0., 1., 1.]))
            .collect(),
    };
    let (quads, normals) = cube_quads(&cube, 1., 1.).into_iter().unzip();
    let root = Bone {
        parent: None,
        pivot: Vec3::ZERO,
        rotation: Vec3::ZERO,
        scale: Vec3::ONE,
        role: BoneRole::Fixed,
        name: PLACEHOLDER_BONE.into(),
        quads,
        normals,
    };
    Rig {
        part_by_name: BTreeMap::from([(PLACEHOLDER_BONE.into(), 0)]),
        bones: vec![root],
    }
}
impl Face {
    const ALL: [Face; 6] = [
        Face::Up,
        Face::Down,
        Face::Front,
        Face::Back,
        Face::Right,
        Face::Left,
    ];
}
impl Rig {
    pub(crate) fn is_placeholder(&self) -> bool {
        self.bones.len() == 1 && self.bones[0].name == PLACEHOLDER_BONE
    }
}

#[cfg(test)]
mod tests;
