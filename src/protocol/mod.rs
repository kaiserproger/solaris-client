//! Bounded 26.1.2 client wire adapter. IDs are audited in ignored C02 evidence.
use flate2::{Compression, read::ZlibDecoder, write::ZlibEncoder};
use serde_json::{Map, Value};
use std::io::{Read, Write};
pub const MAX_FRAME: usize = 8 * 1024 * 1024;
pub type Result<T, E = String> = std::result::Result<T, E>;
pub(crate) fn parse_identifier(id: &str) -> Result<String> {
    let (namespace, path) = match id.split_once(':') {
        Some(("", path)) => ("minecraft", path),
        Some(parts) => parts,
        None => ("minecraft", id),
    };
    if namespace == ".."
        || !namespace.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_.-".contains(&byte)
        })
        || !path.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"/_.-".contains(&byte)
        })
    {
        return Err(format!("invalid protocol identifier {id}"));
    }
    Ok(format!("{namespace}:{path}"))
}
pub struct Reader<'a> {
    pub bytes: &'a [u8],
    pub offset: usize,
}
impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.offset.checked_add(n).ok_or("wire length overflow")?;
        let out = self.bytes.get(self.offset..end).ok_or("truncated packet")?;
        self.offset = end;
        Ok(out)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn bool(&mut self) -> Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err("invalid boolean".into()),
        }
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        let v = f32::from_bits(self.i32()? as u32);
        if v.is_finite() {
            Ok(v)
        } else {
            Err("nonfinite float".into())
        }
    }
    pub fn f64(&mut self) -> Result<f64> {
        let v = f64::from_bits(self.i64()? as u64);
        if v.is_finite() {
            Ok(v)
        } else {
            Err("nonfinite double".into())
        }
    }
    pub fn var(&mut self) -> Result<i32> {
        let mut v = 0u32;
        for i in 0..5 {
            let b = self.u8()?;
            if i == 4 && b & 0xf0 != 0 {
                return Err("oversized VarInt".into());
            }
            v |= u32::from(b & 127) << (i * 7);
            if b & 128 == 0 {
                return Ok(v as i32);
            }
        }
        Err("oversized VarInt".into())
    }
    pub fn varlong(&mut self) -> Result<u64> {
        let mut v = 0;
        for i in 0..10 {
            let b = self.u8()?;
            if i == 9 && b & 0xfe != 0 {
                return Err("oversized VarLong".into());
            }
            v |= u64::from(b & 127) << (i * 7);
            if b & 128 == 0 {
                return Ok(v);
            }
        }
        Err("oversized VarLong".into())
    }
    pub fn count(&mut self, max: usize) -> Result<usize> {
        let n = self.var()?;
        if n < 0 || n as usize > max {
            Err(format!("wire count {n} exceeds {max}"))
        } else {
            Ok(n as usize)
        }
    }
    pub fn string(&mut self, max: usize) -> Result<String> {
        let n = self.count(max.saturating_mul(4))?;
        let s = std::str::from_utf8(self.take(n)?).map_err(|_| "invalid UTF8")?;
        if s.encode_utf16().count() > max {
            return Err("string exceeds character bound".into());
        }
        Ok(s.to_owned())
    }
    pub fn array(&mut self, max: usize) -> Result<&'a [u8]> {
        let n = self.count(max)?;
        self.take(n)
    }
    pub fn longs(&mut self, max: usize) -> Result<Vec<u64>> {
        let n = self.count(max)?;
        (0..n).map(|_| self.i64().map(|v| v as u64)).collect()
    }
    pub fn done(&self) -> Result<()> {
        if self.offset == self.bytes.len() {
            Ok(())
        } else {
            Err(format!(
                "{} trailing packet bytes",
                self.bytes.len() - self.offset
            ))
        }
    }
    pub fn nbt(&mut self) -> Result<Value> {
        let ty = self.u8()?;
        self.tag(ty, 0)
    }
    fn tag(&mut self, ty: u8, depth: usize) -> Result<Value> {
        if depth > 32 {
            return Err("NBT nesting limit".into());
        }
        Ok(match ty {
            0 => Value::Null,
            1 => Value::from(self.u8()? as i8),
            2 => Value::from(self.i16()?),
            3 => Value::from(self.i32()?),
            4 => Value::from(self.i64()?),
            5 => Value::from(self.f32()?),
            6 => Value::from(self.f64()?),
            7 => {
                let n = self.i32()?;
                if !(0..=1048576).contains(&n) {
                    return Err("NBT byte array bound".into());
                }
                Value::Array(
                    self.take(n as usize)?
                        .iter()
                        .map(|v| Value::from(*v))
                        .collect(),
                )
            }
            8 => {
                let n = self.i16()? as u16 as usize;
                Value::from(std::str::from_utf8(self.take(n)?).map_err(|_| "NBT UTF8")?)
            }
            9 => {
                let t = self.u8()?;
                let n = self.i32()?;
                if !(0..=65536).contains(&n) || t == 0 && n != 0 {
                    return Err("NBT list bound".into());
                }
                Value::Array(
                    (0..n)
                        .map(|_| self.tag(t, depth + 1))
                        .collect::<Result<_>>()?,
                )
            }
            10 => {
                let mut m = Map::new();
                loop {
                    let t = self.u8()?;
                    if t == 0 {
                        break;
                    }
                    if m.len() >= 4096 {
                        return Err("NBT compound bound".into());
                    }
                    let n = self.i16()? as u16 as usize;
                    let name = std::str::from_utf8(self.take(n)?)
                        .map_err(|_| "NBT key UTF8")?
                        .to_owned();
                    let val = self.tag(t, depth + 1)?;
                    if m.insert(name, val).is_some() {
                        return Err("duplicate NBT key".into());
                    }
                }
                Value::Object(m)
            }
            11 | 12 => {
                let n = self.i32()?;
                if !(0..=65536).contains(&n) {
                    return Err("NBT array bound".into());
                }
                Value::Array(
                    (0..n)
                        .map(|_| {
                            if ty == 11 {
                                self.i32().map(Value::from)
                            } else {
                                self.i64().map(Value::from)
                            }
                        })
                        .collect::<Result<_>>()?,
                )
            }
            _ => return Err(format!("unknown NBT type {ty}")),
        })
    }
}
#[derive(Default)]
pub struct Writer(pub Vec<u8>);
impl Writer {
    pub fn var(&mut self, v: i32) {
        let mut v = v as u32;
        loop {
            if v & !127 == 0 {
                self.0.push(v as u8);
                break;
            }
            self.0.push((v as u8 & 127) | 128);
            v >>= 7
        }
    }
    pub fn string(&mut self, v: &str) {
        self.var(v.len() as i32);
        self.0.extend(v.as_bytes())
    }
    pub fn i64(&mut self, v: i64) {
        self.0.extend(v.to_be_bytes())
    }
    pub fn i32(&mut self, v: i32) {
        self.0.extend(v.to_be_bytes())
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend(v.to_be_bytes())
    }
    pub fn f64(&mut self, v: f64) {
        self.0.extend(v.to_be_bytes())
    }
    pub fn byte(&mut self, v: u8) {
        self.0.push(v)
    }
}
pub fn frame(id: i32, body: &[u8], threshold: Option<usize>) -> Result<Vec<u8>> {
    let mut p = Writer::default();
    p.var(id);
    p.0.extend(body);
    let payload = if let Some(t) = threshold {
        let mut c = Writer::default();
        if p.0.len() >= t {
            c.var(p.0.len() as i32);
            let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
            z.write_all(&p.0).map_err(|e| e.to_string())?;
            c.0.extend(z.finish().map_err(|e| e.to_string())?)
        } else {
            c.var(0);
            c.0.extend(p.0)
        }
        c.0
    } else {
        p.0
    };
    if payload.len() > MAX_FRAME {
        return Err("outbound frame bound".into());
    }
    let mut out = Writer::default();
    out.var(payload.len() as i32);
    out.0.extend(payload);
    Ok(out.0)
}
pub fn extract(buffer: &mut Vec<u8>, threshold: Option<usize>) -> Result<Option<(i32, Vec<u8>)>> {
    let mut h = Reader::new(buffer);
    let n = match h.var() {
        Ok(v) => v,
        Err(e) if e == "truncated packet" => return Ok(None),
        Err(e) => return Err(e),
    };
    if n < 0 || n as usize > MAX_FRAME {
        return Err("frame length bound".into());
    }
    let off = h.offset;
    let n = n as usize;
    if buffer.len() < off + n {
        return Ok(None);
    }
    let raw = &buffer[off..off + n];
    let decoded;
    let payload = if let Some(t) = threshold {
        let mut r = Reader::new(raw);
        let size = r.count(MAX_FRAME)?;
        if size == 0 {
            if raw.len() - r.offset >= t {
                return Err("uncompressed frame over threshold".into());
            }
            &raw[r.offset..]
        } else {
            if size < t {
                return Err("compressed frame below threshold".into());
            }
            let mut z = ZlibDecoder::new(&raw[r.offset..]);
            let mut data = Vec::with_capacity(size);
            z.by_ref()
                .take(size as u64 + 1)
                .read_to_end(&mut data)
                .map_err(|e| e.to_string())?;
            if data.len() != size || z.total_in() as usize != raw.len() - r.offset {
                return Err("compressed size mismatch".into());
            }
            decoded = data;
            &decoded
        }
    } else {
        raw
    };
    let mut r = Reader::new(payload);
    let id = r.var()?;
    let body = payload[r.offset..].to_vec();
    buffer.drain(..off + n);
    Ok(Some((id, body)))
}
pub fn pack_pos(p: [i32; 3]) -> i64 {
    ((i64::from(p[0]) & 0x3ffffff) << 38)
        | ((i64::from(p[2]) & 0x3ffffff) << 12)
        | (i64::from(p[1]) & 4095)
}
pub fn unpack_pos(v: i64) -> [i32; 3] {
    [
        (v >> 38) as i32,
        ((v << 52) >> 52) as i32,
        ((v << 26) >> 38) as i32,
    ]
}
pub(crate) fn read_registry_tags(
    r: &mut Reader<'_>,
) -> Result<std::collections::BTreeMap<String, std::collections::BTreeMap<String, Vec<i32>>>> {
    let mut received = std::collections::BTreeMap::new();
    let mut member_count = 0usize;
    for _ in 0..r.count(1024)? {
        let registry = r.string(32767)?;
        let mut tags = std::collections::BTreeMap::new();
        for _ in 0..r.count(65536)? {
            let name = r.string(32767)?;
            let count = r.count(1048576)?;
            member_count = member_count
                .checked_add(count)
                .ok_or("registry tag member bound")?;
            if member_count > 1048576 {
                return Err("registry tag member bound".into());
            }
            let mut members = Vec::with_capacity(count);
            for _ in 0..count {
                members.push(r.var()?);
            }
            if tags.insert(name, members).is_some() {
                return Err("duplicate received registry tag".into());
            }
        }
        if received.insert(registry, tags).is_some() {
            return Err("duplicate received tag registry".into());
        }
    }
    r.done()?;
    Ok(received)
}
pub mod chat_verification;
pub mod content;
pub mod models;
pub mod network;
pub mod world;
