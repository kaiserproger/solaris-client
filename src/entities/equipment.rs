//! Received humanoid armor uses activated, source-baked per-slot rigs, not item icons.
//! Contracts: local 26.1.2 ArmorStandRenderer, HumanoidArmorLayer,
//! ArmorStandArmorModel and EquipmentLayerRenderer javap (ignored QA receipts).
use super::{Entity, Models, presentation as p};
use crate::{
    api::{DynamicGeometry, EntityMaterial},
    inventory::Stack,
    protocol::{
        Result,
        content::{Content, TextureRegion},
    },
};
use glam::Mat4;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
struct Asset {
    layers: BTreeMap<String, Vec<LayerDefinition>>,
}
#[derive(Deserialize)]
struct LayerDefinition {
    texture: String,
    dyeable: Option<Dyeable>,
}
#[derive(Deserialize)]
struct Dyeable {
    color_when_undyed: Option<i32>,
}
struct Layer {
    texture: TextureRegion,
    // None is an untinted layer; Some(None) draws only when the item is dyed.
    dyeable: Option<Option<i32>>,
}
impl Layer {
    fn color(&self, dyed: i32) -> Option<[f32; 3]> {
        let rgb = match self.dyeable {
            None => -1,
            Some(default) => {
                if dyed != 0 {
                    dyed
                } else {
                    default?
                }
            }
        };
        Some(std::array::from_fn(|i| {
            ((rgb >> (16 - 8 * i)) & 255) as f32 / 255.
        }))
    }
}

#[derive(Default)]
pub(super) struct Cache {
    assets: BTreeMap<String, Result<BTreeMap<String, Vec<Layer>>>>,
    transforms: Vec<Mat4>,
}
impl Cache {
    fn activate(&mut self, content: &Content, asset: &str) {
        if self.assets.contains_key(asset) {
            return;
        }
        if self.assets.len() >= 4096 {
            self.assets.clear();
        }
        self.assets.insert(asset.into(), load_asset(content, asset));
    }

    pub(super) fn append(
        &mut self,
        e: &Entity,
        slot: usize,
        stack: &Stack,
        models: &Models,
        content: &Content,
        root: Mat4,
        tick: u64,
        partial: f32,
        light: [f32; 2],
        out: &mut DynamicGeometry,
    ) -> Result<bool> {
        let Some(asset) = armor_asset(stack, slot) else {
            return Ok(false);
        };
        let stand = e.type_name == "minecraft:armor_stand";
        let baby = !stand && p::boolean(models, e, "DATA_BABY_ID");
        let key = if stand {
            let small = p::number(models, e, "DATA_CLIENT_FLAGS").unwrap_or(0) & 1 != 0;
            armor_rig(slot, small)
        } else {
            humanoid_rig(models, e, slot, baby)
        };
        let Some(key) = key else {
            return Ok(false);
        };
        let rig = models
            .rigs
            .get(key)
            .ok_or_else(|| format!("armor rig not activated: {key}"))?;
        self.activate(content, asset);
        let layers = self.assets[asset].as_ref().map_err(Clone::clone)?;
        let kind = if baby {
            "humanoid_baby"
        } else if slot == 3 {
            "humanoid_leggings"
        } else {
            "humanoid"
        };
        let Some(layers) = layers.get(kind) else {
            // Equipment without this layer type is also omitted by HumanoidArmorLayer.
            return Ok(true);
        };
        p::evaluate_rig(rig, e, models, tick, partial, root, &mut self.transforms);
        let dyed = stack
            .component("minecraft:dyed_color")
            .and_then(serde_json::Value::as_i64)
            .map(|rgb| rgb as i32 | (0xff00_0000u32 as i32))
            .unwrap_or(0);
        let foil = stack
            .component("minecraft:enchantment_glint_override")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or_else(|| {
                stack
                    .component("minecraft:enchantments")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|v| !v.is_empty())
            });
        let mut glint_pending = foil;
        for layer in layers {
            let Some(color) = layer.color(dyed) else {
                continue;
            };
            p::emit_rig(
                rig,
                &self.transforms,
                layer.texture,
                color,
                EntityMaterial::ArmorCutoutNoCull,
                1.,
                light,
                out,
            )?;
            if glint_pending {
                p::emit_rig(
                    rig,
                    &self.transforms,
                    content
                        .texture("minecraft:misc/enchanted_glint_armor")
                        .ok_or("armor glint texture not activated")?,
                    color,
                    EntityMaterial::ArmorEntityGlint,
                    1.,
                    light,
                    out,
                )?;
                glint_pending = false;
            }
        }
        Ok(true)
    }
}

fn load_asset(content: &Content, asset: &str) -> Result<BTreeMap<String, Vec<Layer>>> {
    let (ns, path) = asset.split_once(':').unwrap_or(("minecraft", asset));
    let bytes = content.resource_bytes(&format!("assets/{ns}/equipment/{path}.json"))?;
    let asset: Asset = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let mut layers = BTreeMap::new();
    for (kind, definitions) in asset.layers {
        if !matches!(
            kind.as_str(),
            "humanoid" | "humanoid_leggings" | "humanoid_baby"
        ) {
            continue;
        }
        let mut activated = Vec::with_capacity(definitions.len());
        for definition in definitions {
            let (ns, path) = definition
                .texture
                .split_once(':')
                .unwrap_or(("minecraft", &definition.texture));
            activated.push(Layer {
                texture: p::texture(content, &format!("{ns}:entity/equipment/{kind}/{path}"))?,
                dyeable: definition.dyeable.map(|v| v.color_when_undyed),
            });
        }
        layers.insert(kind, activated);
    }
    Ok(layers)
}

fn humanoid_rig<'a>(models: &'a Models, e: &Entity, slot: usize, baby: bool) -> Option<&'a str> {
    let part = match slot {
        2 => "feet",
        3 => "legs",
        4 => "chest",
        5 => "head",
        _ => return None,
    };
    // The activated renderer contract already identifies every family's exact
    // adult/baby per-slot model, including slim-player-compatible armor geometry.
    let layer = models.presentation.get(&e.type_name)?["layers"]
        .as_array()?
        .iter()
        .find(|layer| {
            layer["class"] == "net.minecraft.client.renderer.entity.layers.HumanoidArmorLayer"
        })?;
    layer["fields"][if baby { "babyModelSet" } else { "modelSet" }][part]["rig"].as_str()
}

fn armor_asset(stack: &Stack, slot: usize) -> Option<&str> {
    let name = match slot {
        2 => "feet",
        3 => "legs",
        4 => "chest",
        5 => "head",
        _ => return None,
    };
    let equippable = stack.component("minecraft:equippable")?;
    (equippable["slot"].as_str()? == name).then_some(())?;
    equippable["asset_id"].as_str()
}
fn armor_rig(slot: usize, small: bool) -> Option<&'static str> {
    Some(match (slot, small) {
        (2, false) => "minecraft:armor_stand#boots",
        (3, false) => "minecraft:armor_stand#leggings",
        (4, false) => "minecraft:armor_stand#chestplate",
        (5, false) => "minecraft:armor_stand#helmet",
        (2, true) => "minecraft:armor_stand_small#boots",
        (3, true) => "minecraft:armor_stand_small#leggings",
        (4, true) => "minecraft:armor_stand_small#chestplate",
        (5, true) => "minecraft:armor_stand_small#helmet",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn humanoid_fixture(kind: &str) -> (super::super::Store, Models, Content, Cache) {
        use super::super::resources::{Bone, BoneRole, Dimensions, EntitySchema, Rig};
        use crate::protocol::{Reader, Writer};
        use glam::Vec3;

        let mut content = Content::test_cube();
        content.entity_names.insert(1, kind.into());
        let slots = [(4, "chest"), (3, "legs"), (2, "feet"), (5, "head")];
        for (index, (_, name)) in slots.iter().enumerate() {
            let id = index as u32 + 1;
            content.item_names.insert(id, format!("test:{name}"));
            content.insert_test_item_components(
                id,
                BTreeMap::from([(
                    "minecraft:equippable".into(),
                    serde_json::json!({"slot":name,"asset_id":"test:cloth"}),
                )]),
            );
        }
        let mut store = super::super::Store::new();
        let mut spawn = Writer::default();
        spawn.var(4);
        spawn.0.extend([0u8; 16]);
        spawn.var(1);
        for _ in 0..3 {
            spawn.f64(0.);
        }
        spawn.0.extend([0u8; 5]);
        store
            .apply(1, &mut Reader::new(&spawn.0), &content, 1)
            .unwrap();
        let mut equipment = Writer::default();
        equipment.var(4);
        for (index, (slot, _)) in slots.iter().enumerate() {
            equipment.byte(*slot | if index + 1 < slots.len() { 0x80 } else { 0 });
            equipment.var(1);
            equipment.var(index as i32 + 1);
            equipment.var(0);
            equipment.var(0);
        }
        store
            .apply(102, &mut Reader::new(&equipment.0), &content, 2)
            .unwrap();
        let mut rigs = BTreeMap::new();
        let mut sets = serde_json::Map::new();
        for (set, prefix) in [("modelSet", "adult"), ("babyModelSet", "baby")] {
            let mut parts = serde_json::Map::new();
            for (_, part) in slots {
                let key = format!("test:{prefix}/{part}");
                let (name, role) = match part {
                    "head" => ("head", BoneRole::Head),
                    "chest" => ("right_arm", BoneRole::RightArm),
                    _ => ("right_leg", BoneRole::RightLeg),
                };
                rigs.insert(
                    key.clone(),
                    Rig {
                        bones: vec![Bone {
                            parent: None,
                            pivot: Vec3::ZERO,
                            rotation: Vec3::ZERO,
                            scale: Vec3::ONE,
                            role,
                            name: name.into(),
                            quads: vec![[
                                [0., 0., 0., 0., 0.],
                                [1., 0., 0., 1., 0.],
                                [1., 1., 0., 1., 1.],
                                [0., 1., 0., 0., 1.],
                            ]],
                            normals: vec![[0., 0., 1.]],
                        }],
                        part_by_name: BTreeMap::from([(name.into(), 0)]),
                    },
                );
                parts.insert(part.into(), serde_json::json!({"rig":key}));
            }
            sets.insert(set.into(), parts.into());
        }
        let schema = EntitySchema {
            protocol_id: 1,
            width: 0.6,
            height: 1.8,
            eye_height: 1.6,
            fire_immune: false,
            class: "test.Humanoid".into(),
            hierarchy: vec![],
            dimension_constants: BTreeMap::new(),
            enum_ids: BTreeMap::new(),
            enum_numbers: BTreeMap::new(),
            method_owners: BTreeMap::new(),
            dimensions: Dimensions {
                width: 0.6,
                height: 1.8,
                eye_height: 1.6,
                fixed: false,
                attachments: BTreeMap::new(),
            },
            default_attributes: BTreeMap::new(),
            accessors: BTreeMap::from([("DATA_BABY_ID".into(), [0, 8])]),
        };
        let models = Models {
            rigs,
            schemas: BTreeMap::from([(kind.into(), schema)]),
            families: BTreeMap::new(),
            builtin_variants: BTreeMap::new(),
            simple_particles: vec![],
            default_skins: vec![],
            poses: BTreeMap::new(),
            attributes: BTreeMap::new(),
            attribute_ids: BTreeMap::new(),
            digest: String::new(),
            presentation: BTreeMap::from([(
                kind.into(),
                serde_json::json!({"layers":[
                    {"class":"net.minecraft.client.renderer.entity.layers.ItemInHandLayer","fields":{}},
                    {"class":"net.minecraft.client.renderer.entity.layers.HumanoidArmorLayer","fields":sets}
                ]}),
            )]),
            animations: BTreeMap::new(),
            motion: BTreeMap::new(),
        };
        let mut cache = Cache::default();
        cache.assets.insert(
            "test:cloth".into(),
            Ok([
                ("humanoid", 2, 32),
                ("humanoid_leggings", 3, 32),
                ("humanoid_baby", 4, 64),
            ]
            .into_iter()
            .map(|(kind, layer, height)| {
                (
                    kind.into(),
                    vec![Layer {
                        texture: TextureRegion {
                            layer,
                            uv: [0., 0., 1., 1.],
                            width: 64,
                            height,
                        },
                        dyeable: Some(Some(0x804020)),
                    }],
                )
            })
            .collect()),
        );
        (store, models, content, cache)
    }

    #[test]
    fn p1_humanoid_armor_received_slots_render_player_zombie_skeleton_and_piglin_families() {
        for kind in [
            "minecraft:player",
            "minecraft:zombie",
            "minecraft:husk",
            "minecraft:drowned",
            "minecraft:zombie_villager",
            "minecraft:skeleton",
            "minecraft:stray",
            "minecraft:bogged",
            "minecraft:parched",
            "minecraft:wither_skeleton",
            "minecraft:piglin",
            "minecraft:piglin_brute",
            "minecraft:zombified_piglin",
        ] {
            let (mut store, models, content, mut cache) = humanoid_fixture(kind);
            for (slot, part) in [(4, "chest"), (3, "legs"), (2, "feet"), (5, "head")] {
                let entity = store.get(4).unwrap();
                assert_eq!(
                    humanoid_rig(&models, entity, slot, false),
                    Some(format!("test:adult/{part}").as_str())
                );
                let mut out = DynamicGeometry::default();
                assert!(
                    cache
                        .append(
                            entity,
                            slot,
                            entity.equipment_value(slot).unwrap(),
                            &models,
                            &content,
                            Mat4::IDENTITY,
                            2,
                            0.,
                            [3., 7.],
                            &mut out,
                        )
                        .unwrap(),
                    "{kind}:{part}"
                );
                assert_eq!(out.vertices.len(), 4, "{kind}:{part}");
                assert_eq!(
                    out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
                    6
                );
                assert!(
                    out.vertices
                        .iter()
                        .all(|v| v.texture_layer == if slot == 3 { 3 } else { 2 })
                );
                store
                    .apply(
                        102,
                        &mut crate::protocol::Reader::new(&[4, slot as u8, 0]),
                        &content,
                        3,
                    )
                    .unwrap();
                assert!(store.get(4).unwrap().equipment_value(slot).is_none());
            }
        }
    }

    #[test]
    fn p1_humanoid_armor_babies_use_source_baby_models_and_texture_for_all_slots() {
        for kind in [
            "minecraft:zombie",
            "minecraft:husk",
            "minecraft:drowned",
            "minecraft:zombie_villager",
            "minecraft:piglin",
            "minecraft:zombified_piglin",
        ] {
            let (mut store, models, content, mut cache) = humanoid_fixture(kind);
            store
                .get_mut(4)
                .unwrap()
                .metadata
                .insert(0, super::super::Metadata::Bool(true));
            let entity = store.get(4).unwrap();
            for (slot, part) in [(4, "chest"), (3, "legs"), (2, "feet"), (5, "head")] {
                assert_eq!(
                    humanoid_rig(&models, entity, slot, true),
                    Some(format!("test:baby/{part}").as_str())
                );
                let mut out = DynamicGeometry::default();
                assert!(
                    cache
                        .append(
                            entity,
                            slot,
                            entity.equipment_value(slot).unwrap(),
                            &models,
                            &content,
                            Mat4::IDENTITY,
                            2,
                            0.,
                            [15.; 2],
                            &mut out,
                        )
                        .unwrap()
                );
                assert_eq!(out.vertices.len(), 4);
                assert!(out.vertices.iter().all(|v| v.texture_layer == 4));
            }
        }
    }

    #[test]
    fn p1_humanoid_armor_black_dye_white_overlay_and_only_first_layer_glint() {
        let (store, models, mut content, mut cache) = humanoid_fixture("minecraft:player");
        let layers = cache
            .assets
            .get_mut("test:cloth")
            .unwrap()
            .as_mut()
            .unwrap()
            .get_mut("humanoid")
            .unwrap();
        layers.push(Layer {
            texture: layers[0].texture,
            dyeable: None,
        });
        content.insert_test_texture("minecraft:misc/enchanted_glint_armor", layers[0].texture);
        let entity = store.get(4).unwrap();
        let mut stack = entity.equipment_value(4).unwrap().clone();
        let components = Arc::make_mut(&mut stack.components);
        components.insert("minecraft:dyed_color".into(), serde_json::json!(0));
        components.insert(
            "minecraft:enchantment_glint_override".into(),
            serde_json::json!(true),
        );
        let mut out = DynamicGeometry::default();
        assert!(
            cache
                .append(
                    entity,
                    4,
                    &stack,
                    &models,
                    &content,
                    Mat4::IDENTITY,
                    2,
                    0.,
                    [15.; 2],
                    &mut out
                )
                .unwrap()
        );
        assert_eq!(out.vertices.len(), 12);
        assert_eq!(
            out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
            12
        );
        assert_eq!(
            out.indices[EntityMaterial::ArmorEntityGlint as usize].len(),
            6
        );
        assert!(out.vertices[..8].iter().all(|v| v.color[..3] == [0.; 3]));
        assert!(out.vertices[8..].iter().all(|v| v.color[..3] == [1.; 3]));
    }

    #[test]
    fn p1_humanoid_armor_uses_received_pose_and_preserves_nonarmor_fallbacks() {
        let (mut store, mut models, content, mut cache) = humanoid_fixture("minecraft:zombie");
        let entity = store.get(4).unwrap();
        let mut before = DynamicGeometry::default();
        cache
            .append(
                entity,
                5,
                entity.equipment_value(5).unwrap(),
                &models,
                &content,
                Mat4::IDENTITY,
                2,
                0.,
                [15.; 2],
                &mut before,
            )
            .unwrap();
        store.get_mut(4).unwrap().transform.pitch = 45.;
        let entity = store.get(4).unwrap();
        let mut after = DynamicGeometry::default();
        cache
            .append(
                entity,
                5,
                entity.equipment_value(5).unwrap(),
                &models,
                &content,
                Mat4::IDENTITY,
                2,
                0.,
                [15.; 2],
                &mut after,
            )
            .unwrap();
        assert_ne!(before.vertices[2].position, after.vertices[2].position);
        let mut fallback = DynamicGeometry::default();
        assert!(
            !cache
                .append(
                    entity,
                    3,
                    entity.equipment_value(4).unwrap(),
                    &models,
                    &content,
                    Mat4::IDENTITY,
                    2,
                    0.,
                    [15.; 2],
                    &mut fallback
                )
                .unwrap()
        );
        models.presentation.clear();
        assert!(
            !cache
                .append(
                    entity,
                    4,
                    entity.equipment_value(4).unwrap(),
                    &models,
                    &content,
                    Mat4::IDENTITY,
                    2,
                    0.,
                    [15.; 2],
                    &mut fallback
                )
                .unwrap()
        );
        assert!(fallback.vertices.is_empty());
    }

    #[test]
    fn p1_armor_layer_requires_matching_equippable_slot_and_asset() {
        let mut stack = Stack::default();
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:equippable".into(),
            serde_json::json!({"slot":"chest", "asset_id":"test:plate"}),
        )]));
        assert_eq!(armor_asset(&stack, 4), Some("test:plate"));
        for slot in [0, 1, 2, 3, 5, 6, 7] {
            assert_eq!(armor_asset(&stack, slot), None);
        }
        stack.components = Arc::new(BTreeMap::from([(
            "minecraft:equippable".into(),
            serde_json::json!({"slot":"head"}),
        )]));
        assert_eq!(armor_asset(&stack, 5), None);
    }

    #[test]
    fn p1_armor_stand_uses_source_per_slot_and_small_rigs() {
        for (slot, part) in [
            (2, "boots"),
            (3, "leggings"),
            (4, "chestplate"),
            (5, "helmet"),
        ] {
            assert_eq!(
                armor_rig(slot, false).unwrap(),
                format!("minecraft:armor_stand#{part}")
            );
            assert_eq!(
                armor_rig(slot, true).unwrap(),
                format!("minecraft:armor_stand_small#{part}")
            );
        }
        assert_eq!(armor_rig(0, false), None);
    }

    #[test]
    fn p1_armor_layer_dye_and_undyed_overlay_colors() {
        let texture = TextureRegion {
            layer: 0,
            uv: [0., 0., 1., 1.],
            width: 64,
            height: 32,
        };
        let plain = Layer {
            texture,
            dyeable: None,
        };
        assert_eq!(plain.color(0x123456), Some([1.; 3]));
        let leather = Layer {
            texture,
            dyeable: Some(Some(0x804020)),
        };
        assert_eq!(
            leather.color(0),
            Some([128. / 255., 64. / 255., 32. / 255.])
        );
        assert_eq!(leather.color(0x00ff00), Some([0., 1., 0.]));
        let only_dyed = Layer {
            texture,
            dyeable: Some(None),
        };
        assert_eq!(only_dyed.color(0), None);
        assert_eq!(only_dyed.color(0xff0000), Some([1., 0., 0.]));
    }

    #[test]
    fn p1_received_armor_stand_equipment_emits_armor_and_clears_on_removal() {
        use super::super::{
            Store,
            resources::{Bone, BoneRole, Rig},
        };
        use crate::protocol::{Reader, Writer};
        use glam::Vec3;
        let mut content = Content::test_cube();
        content
            .entity_names
            .insert(1, "minecraft:armor_stand".into());
        content.item_names.insert(1, "test:plate".into());
        content.insert_test_item_components(
            1,
            BTreeMap::from([(
                "minecraft:equippable".into(),
                serde_json::json!({"slot":"chest", "asset_id":"test:plate"}),
            )]),
        );
        let mut store = Store::new();
        let mut spawn = Writer::default();
        spawn.var(4);
        spawn.0.extend([0u8; 16]);
        spawn.var(1);
        for coordinate in [0., 0., 0.] {
            spawn.f64(coordinate);
        }
        spawn.0.extend([0u8; 5]);
        store
            .apply(1, &mut Reader::new(&spawn.0), &content, 1)
            .unwrap();
        // Local source javap: target VarInt, ordinal CHEST=4, count/item/empty patch.
        store
            .apply(102, &mut Reader::new(&[4, 4, 1, 1, 0, 0]), &content, 2)
            .unwrap();
        let observed = store.observation(None, 512);
        assert_eq!(observed["entities"][0]["network_id"], 4);
        assert_eq!(observed["entities"][0]["equipment"][4]["count"], 1);
        assert_eq!(observed["entities"][0]["origin"], "server_observed");
        let rig = Rig {
            bones: vec![Bone {
                parent: None,
                pivot: Vec3::ZERO,
                rotation: Vec3::ZERO,
                scale: Vec3::ONE,
                role: BoneRole::Fixed,
                name: "body".into(),
                quads: vec![[
                    [0., 0., 0., 0., 0.],
                    [1., 0., 0., 1., 0.],
                    [1., 1., 0., 1., 1.],
                    [0., 1., 0., 0., 1.],
                ]],
                normals: vec![[0., 0., 1.]],
            }],
            part_by_name: BTreeMap::new(),
        };
        let models = Models {
            rigs: BTreeMap::from([("minecraft:armor_stand#chestplate".into(), rig)]),
            schemas: BTreeMap::new(),
            families: BTreeMap::new(),
            builtin_variants: BTreeMap::new(),
            simple_particles: Vec::new(),
            default_skins: Vec::new(),
            poses: BTreeMap::new(),
            attributes: BTreeMap::new(),
            attribute_ids: BTreeMap::new(),
            digest: String::new(),
            presentation: BTreeMap::new(),
            animations: BTreeMap::new(),
            motion: BTreeMap::new(),
        };
        let texture = TextureRegion {
            layer: 0,
            uv: [0., 0., 1., 1.],
            width: 64,
            height: 32,
        };
        let mut cache = Cache::default();
        cache.assets.insert(
            "test:plate".into(),
            Ok(BTreeMap::from([(
                "humanoid".into(),
                vec![Layer {
                    texture,
                    dyeable: None,
                }],
            )])),
        );
        let entity = store.get(4).unwrap();
        let mut out = DynamicGeometry::default();
        assert!(
            cache
                .append(
                    entity,
                    4,
                    entity.equipment_value(4).unwrap(),
                    &models,
                    &content,
                    Mat4::IDENTITY,
                    2,
                    0.,
                    [15.; 2],
                    &mut out
                )
                .unwrap()
        );
        assert_eq!(out.vertices.len(), 4);
        assert_eq!(
            out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
            6
        );
        content.insert_test_texture("minecraft:misc/enchanted_glint_armor", texture);
        let mut glinted = entity.equipment_value(4).unwrap().clone();
        Arc::make_mut(&mut glinted.components).insert(
            "minecraft:enchantment_glint_override".into(),
            serde_json::json!(true),
        );
        cache
            .assets
            .get_mut("test:plate")
            .unwrap()
            .as_mut()
            .unwrap()
            .get_mut("humanoid")
            .unwrap()
            .push(Layer {
                texture,
                dyeable: None,
            });
        out = DynamicGeometry::default();
        assert!(
            cache
                .append(
                    entity,
                    4,
                    &glinted,
                    &models,
                    &content,
                    Mat4::IDENTITY,
                    2,
                    0.,
                    [15.; 2],
                    &mut out
                )
                .unwrap()
        );
        assert_eq!(out.vertices.len(), 12);
        assert_eq!(
            out.indices[EntityMaterial::ArmorCutoutNoCull as usize].len(),
            12
        );
        assert_eq!(
            out.indices[EntityMaterial::ArmorEntityGlint as usize].len(),
            6
        );
        store
            .apply(102, &mut Reader::new(&[4, 4, 0]), &content, 3)
            .unwrap();
        assert!(store.get(4).unwrap().equipment_value(4).is_none());
        store.equipment_cache = cache;
        store.activate_models(Arc::new(models));
        assert!(store.equipment_cache.assets.is_empty());
        assert!(store.equipment_cache.transforms.is_empty());
    }
}
