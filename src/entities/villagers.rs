//! Villager clothing is selected from synchronized VillagerData, not player profiles.
//! Policy reference: local 26.1.2 VillagerProfessionLayer (including baby textures/hat metadata).
use super::{presentation as p, resources::Rig, Entity, Metadata, Models, Store};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    protocol::{
        content::{Content, TextureRegion},
        Result,
    },
};
use glam::{Mat4, Vec3};

#[derive(Debug)]
pub(super) struct Layers {
    textures: [Option<TextureRegion>; 3],
    type_hat_visible: bool,
}

fn texture_path(family: &str, layer: &str, id: &str) -> String {
    let (namespace, path) = id.split_once(':').unwrap_or(("minecraft", id));
    format!("{namespace}:entity/{family}/{layer}/{path}")
}

fn selected_paths<'a>(
    family: &str,
    data: Option<&Metadata>,
    baby: bool,
    resolve: impl Fn(&str, u32) -> Result<&'a str>,
) -> Result<[Option<String>; 3]> {
    let (kind, profession, level) = match data {
        Some(Metadata::Villager([kind, profession, level])) => (
            resolve("minecraft:villager_type", *kind)?,
            resolve("minecraft:villager_profession", *profession)?,
            *level,
        ),
        // Constructor defaults need not be included in a pairing metadata packet.
        None => ("minecraft:plains", "minecraft:none", 1),
        _ => return Err("invalid VillagerData metadata".into()),
    };
    let mut paths = [
        Some(texture_path(
            family,
            if baby { "baby" } else { "type" },
            kind,
        )),
        None,
        None,
    ];
    if !baby && profession != "minecraft:none" {
        paths[1] = Some(texture_path(family, "profession", profession));
        if profession != "minecraft:nitwit" {
            let tier =
                ["stone", "iron", "gold", "emerald", "diamond"][(level.clamp(1, 5) - 1) as usize];
            paths[2] = Some(texture_path(family, "profession_level", tier));
        }
    }
    Ok(paths)
}

fn hat(content: &Content, texture: &str) -> Result<String> {
    let (namespace, path) = texture.split_once(':').unwrap_or(("minecraft", texture));
    let metadata = match content
        .resource_bytes(&format!("assets/{namespace}/textures/{path}.png.mcmeta"))
    {
        Ok(bytes) => {
            serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())?
        }
        Err(error) if error.starts_with("missing mandatory resource") => return Ok("none".into()),
        Err(error) => return Err(error),
    };
    Ok(metadata["villager"]["hat"]
        .as_str()
        .unwrap_or("none")
        .to_owned())
}

pub(super) fn layers(
    _store: &Store,
    models: &Models,
    e: &Entity,
    content: &Content,
) -> Result<Option<Layers>> {
    let family = match e.type_name.as_str() {
        "minecraft:villager" => "villager",
        "minecraft:zombie_villager" => "zombie_villager",
        _ => return Ok(None),
    };
    let baby = p::boolean(models, e, "DATA_BABY_ID");
    let data = p::value(models, e, "DATA_VILLAGER_DATA");
    let paths = selected_paths(family, data, baby, |registry, id| {
        content.registry_name(
            registry,
            i32::try_from(id).map_err(|_| "villager registry id overflow")?,
        )
    })?;
    // Hat policy uses the adult biome texture even for baby clothing, as vanilla does.
    let type_texture = if baby {
        paths[0].as_ref().unwrap().replace("/baby/", "/type/")
    } else {
        paths[0].as_ref().unwrap().clone()
    };
    let profession_texture = match data {
        Some(Metadata::Villager([_, profession, _])) => texture_path(
            family,
            "profession",
            content.registry_name("minecraft:villager_profession", *profession as i32)?,
        ),
        _ => texture_path(family, "profession", "minecraft:none"),
    };
    let profession_hat = hat(content, &profession_texture)?;
    let type_hat_visible = profession_hat == "none"
        || (profession_hat == "partial" && hat(content, &type_texture)? != "full");
    let mut textures = [None; 3];
    for (target, path) in textures.iter_mut().zip(&paths) {
        *target = path
            .as_ref()
            .map(|path| p::texture(content, path))
            .transpose()?;
    }
    Ok(Some(Layers {
        textures,
        type_hat_visible,
    }))
}

pub(super) fn append(
    layers: Option<&Layers>,
    rig: &Rig,
    transforms: &[Mat4],
    light: [f32; 2],
    out: &mut DynamicGeometry,
) -> Result<()> {
    let Some(layers) = layers else {
        return Ok(());
    };
    for (layer_index, texture) in layers.textures.iter().enumerate() {
        let Some(texture) = texture else {
            continue;
        };
        if layer_index != 0 || layers.type_hat_visible {
            p::emit_rig(
                rig,
                transforms,
                *texture,
                [1.; 3],
                EntityMaterial::EntityCutout,
                1.,
                light,
                out,
            )?;
            continue;
        }
        // Reuse the animated parent transforms. Only the biome hat subtree is hidden;
        // the profession hat still uses the full model, matching the no-hat layer model.
        for (index, bone) in rig.bones.iter().enumerate() {
            let mut ancestor = Some(index);
            let mut hidden = false;
            while let Some(i) = ancestor {
                if rig.bones[i].name == "hat" {
                    hidden = true;
                    break;
                }
                ancestor = rig.bones[i].parent;
            }
            if hidden {
                continue;
            }
            let matrix = transforms[index];
            let normal_matrix = matrix.inverse().transpose();
            for (quad, normal) in bone.quads.iter().zip(&bone.normals) {
                let positions = quad.map(|v| {
                    matrix
                        .transform_point3(Vec3::new(v[0], v[1], v[2]))
                        .to_array()
                });
                let uv = quad.map(|v| {
                    [
                        texture.uv[0] + v[3] * (texture.uv[2] - texture.uv[0]),
                        texture.uv[1] + v[4] * (texture.uv[3] - texture.uv[1]),
                    ]
                });
                p::emit(
                    positions,
                    uv,
                    [1.; 3],
                    texture.layer,
                    EntityMaterial::EntityCutout,
                    1.,
                    light,
                    out,
                )?;
                let normal = normal_matrix
                    .transform_vector3(Vec3::from_array(*normal))
                    .normalize_or_zero()
                    .to_array();
                let first = out.vertices.len() - 4;
                for vertex in &mut out.vertices[first..] {
                    vertex.normal = normal;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn resolve(registry: &str, id: u32) -> Result<&'static str> {
        match (registry, id) {
            ("minecraft:villager_type", 42) => Ok("minecraft:snow"),
            ("minecraft:villager_profession", 7) => Ok("minecraft:librarian"),
            ("minecraft:villager_profession", 8) => Ok("minecraft:none"),
            ("minecraft:villager_profession", 9) => Ok("minecraft:nitwit"),
            _ => Err("unknown fixture registry entry".into()),
        }
    }
    #[test]
    fn villager_data_selects_biome_profession_and_level_layers() {
        let data = Metadata::Villager([42, 7, 3]);
        assert_eq!(
            selected_paths("villager", Some(&data), false, resolve).unwrap(),
            [
                Some("minecraft:entity/villager/type/snow".into()),
                Some("minecraft:entity/villager/profession/librarian".into()),
                Some("minecraft:entity/villager/profession_level/gold".into()),
            ]
        );
        for (level, tier) in [
            (0, "stone"),
            (1, "stone"),
            (2, "iron"),
            (4, "emerald"),
            (5, "diamond"),
            (99, "diamond"),
        ] {
            let data = Metadata::Villager([42, 7, level]);
            assert_eq!(
                selected_paths("zombie_villager", Some(&data), false, resolve).unwrap()[2],
                Some(format!(
                    "minecraft:entity/zombie_villager/profession_level/{tier}"
                ))
            );
        }
    }
    #[test]
    fn villager_baby_and_unemployed_layers_follow_vanilla_policy() {
        let baby = selected_paths(
            "villager",
            Some(&Metadata::Villager([42, 7, 5])),
            true,
            resolve,
        )
        .unwrap();
        assert_eq!(
            baby,
            [
                Some("minecraft:entity/villager/baby/snow".into()),
                None,
                None
            ]
        );
        let none = selected_paths(
            "villager",
            Some(&Metadata::Villager([42, 8, 5])),
            false,
            resolve,
        )
        .unwrap();
        assert!(none[1].is_none() && none[2].is_none());
        let nitwit = selected_paths(
            "villager",
            Some(&Metadata::Villager([42, 9, 5])),
            false,
            resolve,
        )
        .unwrap();
        assert_eq!(
            nitwit[1],
            Some("minecraft:entity/villager/profession/nitwit".into())
        );
        assert!(nitwit[2].is_none());
        assert!(selected_paths("villager", Some(&Metadata::Int(3)), false, resolve).is_err());
    }
    #[test]
    fn villager_layers_reuse_pose_and_hide_only_the_biome_hat() {
        use super::super::resources::{Bone, BoneRole};
        let bone = |name: &str, parent| Bone {
            name: name.into(),
            parent,
            pivot: Vec3::ZERO,
            rotation: Vec3::ZERO,
            scale: Vec3::ONE,
            role: BoneRole::Fixed,
            quads: vec![[
                [0., 0., 0., 0., 0.],
                [1., 0., 0., 1., 0.],
                [1., 1., 0., 1., 1.],
                [0., 1., 0., 0., 1.],
            ]],
            normals: vec![[0., 0., 1.]],
        };
        let rig = Rig {
            bones: vec![
                bone("head", None),
                bone("hat", Some(0)),
                bone("hat_rim", Some(1)),
            ],
            part_by_name: Default::default(),
        };
        let layers = Layers {
            textures: std::array::from_fn(|index| {
                Some(TextureRegion {
                    uv: [0., 0., 1., 1.],
                    layer: index as u32 + 10,
                    width: 64,
                    height: 64,
                })
            }),
            type_hat_visible: false,
        };
        let transforms = [Mat4::from_translation(Vec3::new(3., 4., 5.)); 3];
        let mut out = DynamicGeometry::default();
        append(Some(&layers), &rig, &transforms, [15., 7.], &mut out).unwrap();
        assert_eq!(out.vertices.len(), 4 + 12 + 12);
        assert_eq!(
            out.vertices
                .iter()
                .filter(|v| v.texture_layer == 10)
                .count(),
            4
        );
        assert_eq!(
            out.vertices
                .iter()
                .filter(|v| v.texture_layer == 11)
                .count(),
            12
        );
        assert_eq!(
            out.vertices
                .iter()
                .filter(|v| v.texture_layer == 12)
                .count(),
            12
        );
        assert!(out
            .vertices
            .iter()
            .all(|v| v.light == [15., 7.] && v.position[2] == 5.));
    }
}
