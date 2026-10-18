//! Item component NBT retains DynamicOps numeric widths for persistent Codec hashes.
use crate::{
    protocol::{Reader, Result, Writer},
    ui::hash::Value as H,
};
pub fn read(r: &mut Reader<'_>) -> Result<H> {
    let tag = r.u8()?;
    payload(r, tag, 0)
}
fn count(r: &mut Reader<'_>, max: i32) -> Result<usize> {
    let n = r.i32()?;
    if n < 0 || n > max {
        return Err("component NBT collection bound".into());
    }
    Ok(n as usize)
}
fn string(r: &mut Reader<'_>) -> Result<String> {
    let n = r.i16()? as u16 as usize;
    let bytes = r.take(n)?;
    let mut units = Vec::with_capacity(n);
    let mut p = 0;
    // DataInput.readUTF uses modified UTF-8, including six-byte supplementary scalars.
    while p < bytes.len() {
        let a = bytes[p];
        p += 1;
        let unit = if a < 128 {
            a as u16
        } else if a & 0xe0 == 0xc0 {
            let b = *bytes.get(p).ok_or("truncated modified UTF8")?;
            p += 1;
            if b & 0xc0 != 0x80 {
                return Err("invalid modified UTF8 continuation".into());
            }
            let v = ((a & 31) as u16) << 6 | (b & 63) as u16;
            if v < 128 && v != 0 {
                return Err("overlong modified UTF8".into());
            }
            v
        } else if a & 0xf0 == 0xe0 {
            let b = *bytes.get(p).ok_or("truncated modified UTF8")?;
            let c = *bytes.get(p + 1).ok_or("truncated modified UTF8")?;
            p += 2;
            if b & 0xc0 != 0x80 || c & 0xc0 != 0x80 {
                return Err("invalid modified UTF8 continuation".into());
            }
            let v = ((a & 15) as u16) << 12 | ((b & 63) as u16) << 6 | (c & 63) as u16;
            if v < 2048 {
                return Err("overlong modified UTF8".into());
            }
            v
        } else {
            return Err("invalid modified UTF8 leading byte".into());
        };
        units.push(unit);
    }
    String::from_utf16(&units).map_err(|_| "unpaired UTF16 surrogate in component NBT".into())
}
fn payload(r: &mut Reader<'_>, tag: u8, depth: usize) -> Result<H> {
    if depth > 32 {
        return Err("component NBT nesting bound".into());
    }
    Ok(match tag {
        0 => H::Empty,
        1 => H::Byte(r.u8()? as i8),
        2 => H::Short(r.i16()?),
        3 => H::Int(r.i32()?),
        4 => H::Long(r.i64()?),
        5 => H::Float(r.f32()?),
        6 => H::Double(r.f64()?),
        7 => {
            let n = count(r, 1_048_576)?;
            H::Bytes(r.take(n)?.to_vec())
        }
        8 => H::String(string(r)?),
        9 => {
            let ty = r.u8()?;
            let n = count(r, 65_536)?;
            if ty == 0 && n != 0 {
                return Err("nonempty NBT end-tag list".into());
            }
            let mut list = Vec::with_capacity(n);
            for _ in 0..n {
                let value = payload(r, ty, depth + 1)?;
                let value = match value {
                    H::Map(mut fields)
                        if ty == 10 && fields.len() == 1 && fields[0].0 == H::string("") =>
                    {
                        fields.pop().unwrap().1
                    }
                    value => value,
                };
                list.push(value);
            }
            H::List(list)
        }
        10 => {
            let mut fields = Vec::new();
            loop {
                let ty = r.u8()?;
                if ty == 0 {
                    break;
                }
                if fields.len() >= 4096 {
                    return Err("component NBT map bound".into());
                }
                let key = H::String(string(r)?);
                if fields.iter().any(|(k, _)| k == &key) {
                    return Err("duplicate component NBT field".into());
                }
                fields.push((key, payload(r, ty, depth + 1)?));
            }
            H::Map(fields)
        }
        11 => {
            let n = count(r, 65_536)?;
            let mut values = Vec::with_capacity(n);
            for _ in 0..n {
                values.push(r.i32()?);
            }
            H::Ints(values)
        }
        12 => {
            let n = count(r, 65_536)?;
            let mut values = Vec::with_capacity(n);
            for _ in 0..n {
                values.push(r.i64()?);
            }
            H::Longs(values)
        }
        _ => return Err(format!("unknown component NBT tag {tag}")),
    })
}

/// NbtUtils.compareNbt(pattern, actual, true): type-sensitive compound subset
/// and unordered list subset, including the target's list-size precondition.
pub fn matches(pattern: &H, actual: &H) -> bool {
    match (pattern, actual) {
        (H::Map(p), H::Map(a)) => {
            a.len() >= p.len()
                && p.iter().all(|(k, v)| {
                    a.iter()
                        .find(|(key, _)| key == k)
                        .is_some_and(|(_, value)| matches(v, value))
                })
        }
        (H::List(p), H::List(a)) => {
            if p.is_empty() {
                a.is_empty()
            } else {
                a.len() >= p.len() && p.iter().all(|v| a.iter().any(|value| matches(v, value)))
            }
        }
        _ => pattern == actual,
    }
}

/// Writes the nameless network NBT framing used by component stream codecs.
pub fn write(value: &H, writer: &mut Writer) -> Result<()> {
    writer.0.push(tag_id(value)?);
    write_payload(value, writer)
}
fn tag_id(value: &H) -> Result<u8> {
    Ok(match value {
        H::Empty => 0,
        H::Byte(_) => 1,
        H::Short(_) => 2,
        H::Int(_) => 3,
        H::Long(_) => 4,
        H::Float(_) => 5,
        H::Double(_) => 6,
        H::Bytes(_) => 7,
        H::String(_) => 8,
        H::List(_) => 9,
        H::Map(_) => 10,
        H::Ints(_) => 11,
        H::Longs(_) => 12,
        H::Bool(_) => return Err("HashOps boolean is not an NBT tag".into()),
    })
}
fn write_string(value: &str, writer: &mut Writer) -> Result<()> {
    let length: usize = value
        .encode_utf16()
        .map(|v| {
            if v != 0 && v < 128 {
                1
            } else if v < 2048 {
                2
            } else {
                3
            }
        })
        .sum();
    let length = u16::try_from(length).map_err(|_| "modified UTF8 string bound")?;
    writer.0.extend_from_slice(&length.to_be_bytes());
    for v in value.encode_utf16() {
        if v != 0 && v < 128 {
            writer.0.push(v as u8);
        } else if v < 2048 {
            writer
                .0
                .extend_from_slice(&[0xc0 | (v >> 6) as u8, 0x80 | (v & 63) as u8]);
        } else {
            writer.0.extend_from_slice(&[
                0xe0 | (v >> 12) as u8,
                0x80 | ((v >> 6) & 63) as u8,
                0x80 | (v & 63) as u8,
            ]);
        }
    }
    Ok(())
}
fn write_payload(value: &H, writer: &mut Writer) -> Result<()> {
    match value {
        H::Empty => {}
        H::Byte(v) => writer.0.push(*v as u8),
        H::Short(v) => writer.0.extend_from_slice(&v.to_be_bytes()),
        H::Int(v) => writer.0.extend_from_slice(&v.to_be_bytes()),
        H::Long(v) => writer.0.extend_from_slice(&v.to_be_bytes()),
        H::Float(v) => writer.0.extend_from_slice(&v.to_be_bytes()),
        H::Double(v) => writer.0.extend_from_slice(&v.to_be_bytes()),
        H::String(v) => write_string(v, writer)?,
        H::Bytes(v) => {
            writer.0.extend_from_slice(&(v.len() as i32).to_be_bytes());
            writer.0.extend_from_slice(v);
        }
        H::Ints(v) => {
            writer.0.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for n in v {
                writer.0.extend_from_slice(&n.to_be_bytes());
            }
        }
        H::Longs(v) => {
            writer.0.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for n in v {
                writer.0.extend_from_slice(&n.to_be_bytes());
            }
        }
        H::List(v) => {
            let mut ty = v.first().map(tag_id).transpose()?.unwrap_or(0);
            for value in v {
                let id = tag_id(value)?;
                if id == 0 {
                    return Err("NBT list cannot contain end tag".into());
                }
                if id != ty {
                    ty = 10;
                    break;
                }
            }
            writer.0.push(ty);
            writer.0.extend_from_slice(&(v.len() as i32).to_be_bytes());
            for n in v {
                let wrapper = matches!(n, H::Map(fields) if fields.len() == 1 && fields[0].0 == H::string(""));
                if ty == 10 && (!matches!(n, H::Map(_)) || wrapper) {
                    writer.0.push(tag_id(n)?);
                    write_string("", writer)?;
                    write_payload(n, writer)?;
                    writer.0.push(0);
                } else {
                    write_payload(n, writer)?;
                }
            }
        }
        H::Map(v) => {
            for (key, value) in v {
                let H::String(key) = key else {
                    return Err("NBT key must be string".into());
                };
                let ty = tag_id(value)?;
                if ty == 0 {
                    return Err("NBT compound cannot contain end tag".into());
                }
                writer.0.push(ty);
                write_string(key, writer)?;
                write_payload(value, writer)?;
            }
            writer.0.push(0);
        }
        H::Bool(_) => return Err("HashOps boolean is not an NBT tag".into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_mixed_list_preserves_nested_empty_key_and_nbt_classes() {
        // Captured from the pinned 26.1.2 ListTag/NbtIo implementation.
        let wire = "0a070005617272617900000002ff0001000462797465ff060006646f75626c653f1a36e2eb1c432d02000573686f727400020900056d697865640a000000030300000000000100080000000178000a000003000000000002000008000474657874000ac3a9eda0bdedba80c080050005666c6f61744b1896800400046c6f6e67000000000000000300";
        let bytes: Vec<u8> = wire
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        let mut reader = Reader::new(&bytes);
        let value = read(&mut reader).unwrap();
        reader.done().unwrap();
        assert_eq!(
            value,
            H::record([
                ("array", H::Bytes(vec![255, 0])),
                ("byte", H::Byte(-1)),
                ("double", H::Double(0.0001)),
                ("float", H::Float(1e7)),
                ("long", H::Long(3)),
                (
                    "mixed",
                    H::List(vec![
                        H::Int(1),
                        H::string("x"),
                        H::record([("", H::Int(2))])
                    ])
                ),
                ("short", H::Short(2)),
                ("text", H::string("é🚀\0")),
            ])
        );
        assert_eq!(value.hash(), 522275696);
    }
}
