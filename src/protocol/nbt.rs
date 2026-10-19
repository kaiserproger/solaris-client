//! Type-preserving network NBT for server-driven dialogs. `Reader::nbt` flattens to JSON,
//! which loses Byte/Int/Float distinctions; a dialog action's `additions` must round-trip
//! to the server exactly as received, and input values must be written with vanilla's types.
use super::{Reader, Result, Writer};
use serde_json::{Map, Value};

const DEPTH_LIMIT: usize = 32;

#[derive(Clone, Debug, PartialEq)]
pub enum Nbt {
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<i8>),
    String(String),
    /// Element type is kept so empty and nested lists re-encode identically.
    List(u8, Vec<Nbt>),
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    fn type_id(&self) -> u8 {
        match self {
            Self::Byte(_) => 1,
            Self::Short(_) => 2,
            Self::Int(_) => 3,
            Self::Long(_) => 4,
            Self::Float(_) => 5,
            Self::Double(_) => 6,
            Self::ByteArray(_) => 7,
            Self::String(_) => 8,
            Self::List(..) => 9,
            Self::Compound(_) => 10,
            Self::IntArray(_) => 11,
            Self::LongArray(_) => 12,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Nbt> {
        match self {
            Self::Compound(entries) => entries.iter().find(|(name, _)| name == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    /// Vanilla `Codec.BOOL` over NbtOps accepts any numeric tag (non-zero = true).
    pub fn as_bool(&self) -> Option<bool> {
        self.as_f64().map(|value| value != 0.0)
    }

    pub fn as_f64(&self) -> Option<f64> {
        Some(match self {
            Self::Byte(v) => f64::from(*v),
            Self::Short(v) => f64::from(*v),
            Self::Int(v) => f64::from(*v),
            Self::Long(v) => *v as f64,
            Self::Float(v) => f64::from(*v),
            Self::Double(v) => *v,
            _ => return None,
        })
    }

    pub fn as_i64(&self) -> Option<i64> {
        Some(match self {
            Self::Byte(v) => i64::from(*v),
            Self::Short(v) => i64::from(*v),
            Self::Int(v) => i64::from(*v),
            Self::Long(v) => *v,
            _ => return None,
        })
    }

    pub fn as_list(&self) -> Option<&[Nbt]> {
        match self {
            Self::List(_, items) => Some(items),
            _ => None,
        }
    }

    /// Same flattening as `Reader::nbt`, so components resolve through the existing text renderer.
    pub fn to_json(&self) -> Value {
        match self {
            Self::Byte(v) => Value::from(*v),
            Self::Short(v) => Value::from(*v),
            Self::Int(v) => Value::from(*v),
            Self::Long(v) => Value::from(*v),
            Self::Float(v) => Value::from(*v),
            Self::Double(v) => Value::from(*v),
            Self::ByteArray(v) => Value::Array(v.iter().map(|b| Value::from(*b as u8)).collect()),
            Self::String(v) => Value::from(v.as_str()),
            Self::List(_, items) => Value::Array(items.iter().map(Nbt::to_json).collect()),
            Self::Compound(entries) => {
                let mut map = Map::new();
                for (name, value) in entries {
                    map.insert(name.clone(), value.to_json());
                }
                Value::Object(map)
            }
            Self::IntArray(v) => Value::Array(v.iter().map(|i| Value::from(*i)).collect()),
            Self::LongArray(v) => Value::Array(v.iter().map(|i| Value::from(*i)).collect()),
        }
    }

    /// Anonymous network root (`FriendlyByteBuf.writeNbt`): type id, then the payload.
    pub fn write_root(&self, writer: &mut Writer) -> Result<()> {
        writer.byte(self.type_id());
        self.write_payload(writer, 0)
    }

    fn write_payload(&self, w: &mut Writer, depth: usize) -> Result<()> {
        if depth > DEPTH_LIMIT {
            return Err("NBT nesting limit".into());
        }
        match self {
            Self::Byte(v) => w.byte(*v as u8),
            Self::Short(v) => w.0.extend(v.to_be_bytes()),
            Self::Int(v) => w.i32(*v),
            Self::Long(v) => w.i64(*v),
            Self::Float(v) => w.f32(*v),
            Self::Double(v) => w.f64(*v),
            Self::ByteArray(v) => {
                w.i32(v.len() as i32);
                w.0.extend(v.iter().map(|b| *b as u8));
            }
            Self::String(v) => write_utf(w, v)?,
            Self::List(element, items) => {
                let element = if items.is_empty() { 0 } else { *element };
                if items.iter().any(|item| item.type_id() != element) {
                    return Err("NBT list element type mismatch".into());
                }
                w.byte(element);
                w.i32(items.len() as i32);
                for item in items {
                    item.write_payload(w, depth + 1)?;
                }
            }
            Self::Compound(entries) => {
                for (name, value) in entries {
                    w.byte(value.type_id());
                    write_utf(w, name)?;
                    value.write_payload(w, depth + 1)?;
                }
                w.byte(0);
            }
            Self::IntArray(v) => {
                w.i32(v.len() as i32);
                for i in v {
                    w.i32(*i);
                }
            }
            Self::LongArray(v) => {
                w.i32(v.len() as i32);
                for i in v {
                    w.i64(*i);
                }
            }
        }
        Ok(())
    }
}

/// NBT strings are length-prefixed by an unsigned short of (Modified) UTF-8 bytes.
fn write_utf(w: &mut Writer, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let len = u16::try_from(bytes.len()).map_err(|_| "NBT string exceeds 65535 bytes")?;
    w.0.extend(len.to_be_bytes());
    w.0.extend(bytes);
    Ok(())
}

impl Reader<'_> {
    /// Anonymous network root with every tag type preserved.
    pub fn nbt_typed(&mut self) -> Result<Nbt> {
        let ty = self.u8()?;
        self.typed_tag(ty, 0)
    }

    fn typed_tag(&mut self, ty: u8, depth: usize) -> Result<Nbt> {
        if depth > DEPTH_LIMIT {
            return Err("NBT nesting limit".into());
        }
        Ok(match ty {
            1 => Nbt::Byte(self.u8()? as i8),
            2 => Nbt::Short(self.i16()?),
            3 => Nbt::Int(self.i32()?),
            4 => Nbt::Long(self.i64()?),
            5 => Nbt::Float(self.f32()?),
            6 => Nbt::Double(self.f64()?),
            7 => {
                let n = self.i32()?;
                if !(0..=1048576).contains(&n) {
                    return Err("NBT byte array bound".into());
                }
                Nbt::ByteArray(self.take(n as usize)?.iter().map(|b| *b as i8).collect())
            }
            8 => {
                let n = self.i16()? as u16 as usize;
                Nbt::String(
                    std::str::from_utf8(self.take(n)?)
                        .map_err(|_| "NBT UTF8")?
                        .to_owned(),
                )
            }
            9 => {
                let element = self.u8()?;
                let n = self.i32()?;
                if !(0..=65536).contains(&n) || element == 0 && n != 0 {
                    return Err("NBT list bound".into());
                }
                Nbt::List(
                    element,
                    (0..n)
                        .map(|_| self.typed_tag(element, depth + 1))
                        .collect::<Result<_>>()?,
                )
            }
            10 => {
                let mut entries: Vec<(String, Nbt)> = Vec::new();
                loop {
                    let t = self.u8()?;
                    if t == 0 {
                        break;
                    }
                    if entries.len() >= 4096 {
                        return Err("NBT compound bound".into());
                    }
                    let n = self.i16()? as u16 as usize;
                    let name = std::str::from_utf8(self.take(n)?)
                        .map_err(|_| "NBT key UTF8")?
                        .to_owned();
                    if entries.iter().any(|(existing, _)| *existing == name) {
                        return Err("duplicate NBT key".into());
                    }
                    let value = self.typed_tag(t, depth + 1)?;
                    entries.push((name, value));
                }
                Nbt::Compound(entries)
            }
            11 | 12 => {
                let n = self.i32()?;
                if !(0..=65536).contains(&n) {
                    return Err("NBT array bound".into());
                }
                if ty == 11 {
                    Nbt::IntArray((0..n).map(|_| self.i32()).collect::<Result<_>>()?)
                } else {
                    Nbt::LongArray((0..n).map(|_| self.i64()).collect::<Result<_>>()?)
                }
            }
            _ => return Err(format!("unknown NBT type {ty}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_round_trip_preserves_every_tag_type() {
        let tag = Nbt::Compound(vec![
            ("b".into(), Nbt::Byte(-1)),
            ("s".into(), Nbt::Short(7)),
            ("i".into(), Nbt::Int(70000)),
            ("l".into(), Nbt::Long(1 << 40)),
            ("f".into(), Nbt::Float(12.5)),
            ("d".into(), Nbt::Double(0.25)),
            ("str".into(), Nbt::String("Ёлки".into())),
            ("list".into(), Nbt::List(8, vec![Nbt::String("a".into())])),
            ("empty".into(), Nbt::List(0, vec![])),
            ("ia".into(), Nbt::IntArray(vec![1, 2])),
            ("la".into(), Nbt::LongArray(vec![3])),
            ("ba".into(), Nbt::ByteArray(vec![-1, 4])),
            (
                "nested".into(),
                Nbt::Compound(vec![("x".into(), Nbt::Byte(1))]),
            ),
        ]);
        let mut writer = Writer::default();
        tag.write_root(&mut writer).unwrap();
        let mut reader = Reader::new(&writer.0);
        assert_eq!(reader.nbt_typed().unwrap(), tag);
        reader.done().unwrap();
        // The JSON view matches the existing flattening reader.
        let json = Reader::new(&writer.0).nbt().unwrap();
        assert_eq!(tag.to_json(), json);
    }

    #[test]
    fn duplicate_keys_and_mixed_lists_are_rejected() {
        let duplicate = [10, 1, 0, 1, b'a', 1, 1, 0, 1, b'a', 2, 0];
        assert!(Reader::new(&duplicate).nbt_typed().is_err());
        let mut writer = Writer::default();
        assert!(
            Nbt::List(1, vec![Nbt::Byte(1), Nbt::Int(1)])
                .write_root(&mut writer)
                .is_err()
        );
    }
}
