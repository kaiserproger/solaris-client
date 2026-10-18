//! Protocol 775 HashOps CRC32C: hashes typed persistent Codec values, not wire bytes.
//! Numeric payloads and child hashes are little-endian; strings contain UTF-16 code units.
#[derive(Clone, Debug)]
pub enum Value {
    Empty,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    String(String),
    Bool(bool),
    List(Vec<Value>),
    Map(Vec<(Value, Value)>),
    Bytes(Vec<u8>),
    Ints(Vec<i32>),
    Longs(Vec<i64>),
}
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        use Value::*;
        match (self, other) {
            (Empty, Empty) => true,
            (Byte(a), Byte(b)) => a == b,
            (Short(a), Short(b)) => a == b,
            (Int(a), Int(b)) => a == b,
            (Long(a), Long(b)) => a == b,
            (Float(a), Float(b)) => a.to_bits() == b.to_bits(),
            (Double(a), Double(b)) => a.to_bits() == b.to_bits(),
            (String(a), String(b)) => a == b,
            (Bool(a), Bool(b)) => a == b,
            (List(a), List(b)) => a == b,
            (Map(a), Map(b)) => {
                a.len() == b.len()
                    && a.iter().all(|(key, value)| {
                        b.iter()
                            .find(|(k, _)| k == key)
                            .is_some_and(|(_, v)| v == value)
                    })
            }
            (Bytes(a), Bytes(b)) => a == b,
            (Ints(a), Ints(b)) => a == b,
            (Longs(a), Longs(b)) => a == b,
            _ => false,
        }
    }
}
struct Crc(u32);
impl Crc {
    fn new(tag: u8) -> Self {
        Self(crc32c::crc32c(&[tag]))
    }
    fn byte(&mut self, b: u8) {
        self.bytes(&[b]);
    }
    fn bytes(&mut self, bytes: &[u8]) {
        self.0 = crc32c::crc32c_append(self.0, bytes);
    }
    fn finish(self) -> u32 {
        self.0
    }
}
impl Value {
    pub fn string(value: impl Into<String>) -> Self {
        Self::String(value.into())
    }
    pub fn record(fields: impl IntoIterator<Item = (impl Into<String>, Value)>) -> Self {
        Self::Map(
            fields
                .into_iter()
                .map(|(k, v)| (Self::String(k.into()), v))
                .collect(),
        )
    }
    pub fn json(&self) -> Result<serde_json::Value, String> {
        use serde_json::{Value as J, json};
        Ok(match self {
            Self::Empty => J::Null,
            Self::Byte(v) => json!(v),
            Self::Short(v) => json!(v),
            Self::Int(v) => json!(v),
            Self::Long(v) => json!(v),
            Self::Float(v) => {
                if !v.is_finite() {
                    return Err("nonfinite component float".into());
                }
                let shortest = v
                    .to_string()
                    .parse::<f64>()
                    .map_err(|_| "component float JSON")?;
                J::Number(serde_json::Number::from_f64(shortest).ok_or("component float JSON")?)
            }
            Self::Double(v) => {
                J::Number(serde_json::Number::from_f64(*v).ok_or("nonfinite component double")?)
            }
            Self::Bool(v) => json!(v),
            Self::String(v) => json!(v),
            Self::List(v) => J::Array(v.iter().map(Self::json).collect::<Result<_, _>>()?),
            Self::Map(v) => {
                let mut object = serde_json::Map::new();
                for (key, value) in v {
                    let Self::String(key) = key else {
                        return Err("component JSON requires string map keys".into());
                    };
                    if object.insert(key.clone(), value.json()?).is_some() {
                        return Err("component JSON has duplicate map keys".into());
                    }
                }
                J::Object(object)
            }
            Self::Bytes(v) => J::Array(v.iter().map(|v| json!(*v as i8)).collect()),
            Self::Ints(v) => json!(v),
            Self::Longs(v) => json!(v),
        })
    }
    pub fn hash(&self) -> u32 {
        match self {
            Self::Empty => Crc::new(1).finish(),
            Self::Byte(x) => scalar(6, &x.to_le_bytes()),
            Self::Short(x) => scalar(7, &x.to_le_bytes()),
            Self::Int(x) => scalar(8, &x.to_le_bytes()),
            Self::Long(x) => scalar(9, &x.to_le_bytes()),
            Self::Float(x) => scalar(10, &x.to_bits().to_le_bytes()),
            Self::Double(x) => scalar(11, &x.to_bits().to_le_bytes()),
            Self::String(x) => {
                let mut c = Crc::new(12);
                c.bytes(&(x.encode_utf16().count() as u32).to_le_bytes());
                for unit in x.encode_utf16() {
                    c.bytes(&unit.to_le_bytes());
                }
                c.finish()
            }
            Self::Bool(x) => scalar(13, &[u8::from(*x)]),
            Self::List(xs) => {
                let mut c = Crc::new(4);
                for x in xs {
                    c.bytes(&x.hash().to_le_bytes());
                }
                c.byte(5);
                c.finish()
            }
            Self::Map(xs) => {
                let mut entries: Vec<_> = xs.iter().map(|(k, v)| (k.hash(), v.hash())).collect();
                entries.sort_unstable();
                let mut c = Crc::new(2);
                for (k, v) in entries {
                    c.bytes(&k.to_le_bytes());
                    c.bytes(&v.to_le_bytes());
                }
                c.byte(3);
                c.finish()
            }
            Self::Bytes(xs) => {
                let mut c = Crc::new(14);
                c.bytes(xs);
                c.byte(15);
                c.finish()
            }
            Self::Ints(xs) => {
                let mut c = Crc::new(16);
                for x in xs {
                    c.bytes(&x.to_le_bytes());
                }
                c.byte(17);
                c.finish()
            }
            Self::Longs(xs) => {
                let mut c = Crc::new(18);
                for x in xs {
                    c.bytes(&x.to_le_bytes());
                }
                c.byte(19);
                c.finish()
            }
        }
    }
}
fn scalar(tag: u8, bytes: &[u8]) -> u32 {
    let mut c = Crc::new(tag);
    c.bytes(bytes);
    c.finish()
}

#[cfg(test)]
mod tests {
    use super::Value as H;
    #[test]
    fn persistent_typed_hashes_match_target_java() {
        let fixtures = [
            (H::Empty, 2685849682),
            (H::Byte(-5), 4064928430),
            (H::Short(-32767), 2243978268),
            (H::Int(1234567), 1046786361),
            (H::Long(-9876543210), 112787343),
            (H::Float(0.25), 2540678803),
            (H::Double(-0.125), 4075545613),
            (H::String("A🚀é".into()), 1934733251),
            (H::Bool(false), 828198337),
            (H::Bool(true), 3275148994),
            (H::List(vec![H::Int(1), H::string("x")]), 2225307967),
            (H::Bytes(vec![0, 255, 7]), 2768880261),
            (H::Ints(vec![-1, 2, 1234]), 3046259256),
            (H::Longs(vec![-1, 9876543210]), 1126713305),
        ];
        for (value, expected) in fixtures {
            assert_eq!(value.hash(), expected, "{value:?}");
        }
    }
    #[test]
    fn persistent_map_order_is_hash_order_not_source_order() {
        let mut pairs = vec![
            (H::string("damage"), H::Int(7)),
            (H::string("name"), H::string("tool")),
        ];
        assert_eq!(H::Map(pairs.clone()).hash(), 2682302067);
        pairs.reverse();
        assert_eq!(H::Map(pairs).hash(), 2682302067);
        assert_ne!(H::Int(7).hash(), H::Byte(7).hash());
    }
}
