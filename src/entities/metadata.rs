//! Serializer IDs are exported from the local 26.1.2 EntityDataSerializers registry.
use crate::{
    inventory::{self, Stack},
    protocol::{Reader, Result, content::Content},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum Metadata {
    Byte(i8),
    Int(i32),
    Long(String),
    Float(f32),
    String(String),
    Component(Value),
    OptionalComponent(Option<Value>),
    Item(Stack),
    Bool(bool),
    Rotations([f32; 3]),
    BlockPosition([i32; 3]),
    OptionalBlockPosition(Option<[i32; 3]>),
    Direction(u8),
    EntityReference(Option<String>),
    BlockState(u32),
    OptionalBlockState(Option<u32>),
    Particle(Value),
    Particles(Vec<Value>),
    Villager([u32; 3]),
    OptionalUnsignedInt(Option<u32>),
    Pose(u8),
    Registry { serializer: u32, id: u32 },
    State { serializer: u32, id: u32 },
    GlobalPosition(Option<(String, [i32; 3])>),
    Vector([f32; 3]),
    Quaternion([f32; 4]),
    Profile(Value),
    Arm(u8),
}
impl Metadata {
    pub fn serializer_id(&self) -> u32 {
        match self {
            Self::Byte(_) => 0,
            Self::Int(_) => 1,
            Self::Long(_) => 2,
            Self::Float(_) => 3,
            Self::String(_) => 4,
            Self::Component(_) => 5,
            Self::OptionalComponent(_) => 6,
            Self::Item(_) => 7,
            Self::Bool(_) => 8,
            Self::Rotations(_) => 9,
            Self::BlockPosition(_) => 10,
            Self::OptionalBlockPosition(_) => 11,
            Self::Direction(_) => 12,
            Self::EntityReference(_) => 13,
            Self::BlockState(_) => 14,
            Self::OptionalBlockState(_) => 15,
            Self::Particle(_) => 16,
            Self::Particles(_) => 17,
            Self::Villager(_) => 18,
            Self::OptionalUnsignedInt(_) => 19,
            Self::Pose(_) => 20,
            Self::Registry { serializer, .. } | Self::State { serializer, .. } => *serializer,
            Self::GlobalPosition(_) => 33,
            Self::Vector(_) => 39,
            Self::Quaternion(_) => 40,
            Self::Profile(_) => 41,
            Self::Arm(_) => 42,
        }
    }
    pub fn int(&self) -> Option<i32> {
        match self {
            Self::Byte(v) => Some(i32::from(*v)),
            Self::Int(v) => Some(*v),
            Self::Pose(v) | Self::Arm(v) | Self::Direction(v) => Some(i32::from(*v)),
            Self::State { id, .. } | Self::Registry { id, .. } => i32::try_from(*id).ok(),
            _ => None,
        }
    }
    pub fn boolean(&self) -> Option<bool> {
        if let Self::Bool(v) = self {
            Some(*v)
        } else {
            None
        }
    }
}
pub fn position(raw: i64) -> [i32; 3] {
    [
        (raw >> 38) as i32,
        (raw << 52 >> 52) as i32,
        (raw << 26 >> 38) as i32,
    ]
}
pub fn uuid(r: &mut Reader<'_>) -> Result<String> {
    Ok(uuid::Uuid::from_slice(r.take(16)?)
        .map_err(|e| e.to_string())?
        .to_string())
}
fn unsigned(r: &mut Reader<'_>) -> Result<u32> {
    let n = r.var()?;
    u32::try_from(n).map_err(|_| "negative metadata registry/index".into())
}
pub fn decode(
    r: &mut Reader<'_>,
    content: &Content,
    models: &super::Models,
) -> Result<BTreeMap<u8, Metadata>> {
    let mut values = BTreeMap::new();
    let start = r.offset;
    loop {
        let index = r.u8()?;
        if index == 255 {
            break;
        }
        if values.len() >= 254 || values.contains_key(&index) {
            return Err("duplicate/excess entity metadata index".into());
        }
        let serializer = r.var()?;
        let value = match serializer {
            0 => Metadata::Byte(r.u8()? as i8),
            1 => Metadata::Int(r.var()?),
            2 => Metadata::Long((r.varlong()? as i64).to_string()),
            3 => Metadata::Float(r.f32()?),
            4 => Metadata::String(r.string(32767)?),
            5 => Metadata::Component(r.nbt()?),
            6 => Metadata::OptionalComponent(if r.bool()? { Some(r.nbt()?) } else { None }),
            7 => Metadata::Item(inventory::read_stack(r, content)?),
            8 => Metadata::Bool(r.bool()?),
            9 => Metadata::Rotations([r.f32()?, r.f32()?, r.f32()?]),
            10 => Metadata::BlockPosition(position(r.i64()?)),
            11 => Metadata::OptionalBlockPosition(if r.bool()? {
                Some(position(r.i64()?))
            } else {
                None
            }),
            12 => Metadata::Direction(r.count(5)? as u8),
            13 => Metadata::EntityReference(if r.bool()? { Some(uuid(r)?) } else { None }),
            14 => Metadata::BlockState(unsigned(r)?),
            15 => {
                let n = unsigned(r)?;
                Metadata::OptionalBlockState(if n == 0 { None } else { Some(n) })
            }
            16 => Metadata::Particle(particle(r, content, models)?),
            17 => {
                let n = r.count(256)?;
                Metadata::Particles(
                    (0..n)
                        .map(|_| particle(r, content, models))
                        .collect::<Result<_>>()?,
                )
            }
            18 => Metadata::Villager([unsigned(r)?, unsigned(r)?, unsigned(r)?]),
            19 => {
                let n = unsigned(r)?;
                Metadata::OptionalUnsignedInt(n.checked_sub(1))
            }
            20 => Metadata::Pose(r.count(17)? as u8),
            21..=32 | 34 => Metadata::Registry {
                serializer: serializer as u32,
                id: unsigned(r)?,
            },
            33 => Metadata::GlobalPosition(if r.bool()? {
                Some((r.string(32767)?, position(r.i64()?)))
            } else {
                None
            }),
            35..=38 => Metadata::State {
                serializer: serializer as u32,
                id: unsigned(r)?,
            },
            39 => Metadata::Vector([r.f32()?, r.f32()?, r.f32()?]),
            40 => Metadata::Quaternion([r.f32()?, r.f32()?, r.f32()?, r.f32()?]),
            41 => Metadata::Profile(inventory::read_profile(r)?),
            42 => Metadata::Arm(r.count(1)? as u8),
            _ => {
                return Err(format!(
                    "unknown mandatory entity metadata serializer {serializer}"
                ));
            }
        };
        if r.offset - start > 65536 {
            return Err("entity metadata byte bound".into());
        }
        values.insert(index, value);
    }
    Ok(values)
}
fn particle(r: &mut Reader<'_>, content: &Content, models: &super::Models) -> Result<Value> {
    let id = unsigned(r)?;
    let name = content
        .particle_names
        .get(&id)
        .ok_or_else(|| format!("unknown particle registry {id}"))?;
    let data = match name.as_str() {
        "minecraft:block"
        | "minecraft:block_marker"
        | "minecraft:falling_dust"
        | "minecraft:dust_pillar"
        | "minecraft:block_crumble" => json!({"block_state":unsigned(r)?}),
        "minecraft:dust" => json!({"color":r.i32()?,"scale":r.f32()?}),
        "minecraft:dust_color_transition" => {
            json!({"from":r.i32()?,"to":r.i32()?,"scale":r.f32()?})
        }
        "minecraft:entity_effect" | "minecraft:tinted_leaves" => json!({"color":r.i32()?}),
        "minecraft:item" => json!({"stack":inventory::read_template(r, content)?}),
        "minecraft:sculk_charge" => json!({"roll":r.f32()?}),
        "minecraft:dragon_breath" => json!({"power":r.f32()?}),
        "minecraft:effect" | "minecraft:instant_effect" => {
            json!({"color":r.i32()?,"power":r.f32()?})
        }
        "minecraft:flash" => json!({"color":r.i32()?}),
        "minecraft:shriek" => json!({"delay":unsigned(r)?}),
        "minecraft:trail" => {
            json!({"target":[r.f64()?,r.f64()?,r.f64()?],"color":r.i32()?,"duration":unsigned(r)?})
        }
        "minecraft:vibration" => {
            // PositionSource's built-in registry exports block=0, entity=1 in this target.
            let destination = match unsigned(r)? {
                0 => json!({"type":"minecraft:block","position":position(r.i64()?)}),
                1 => json!({"type":"minecraft:entity","entity_id":r.var()?,"eye_offset":r.f32()?}),
                _ => return Err("unknown vibration position source type".into()),
            };
            json!({"destination":destination,"arrival_ticks":unsigned(r)?})
        }
        _ if models
            .simple_particles
            .binary_search_by(|n| n.as_str().cmp(name.as_str()))
            .is_ok() =>
        {
            Value::Null
        }
        _ => return Err(format!("particle option codec not activated for {name}")),
    };
    Ok(json!({"type_id":id,"type":name,"data":data}))
}
