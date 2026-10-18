use super::{Reader, Result};
use std::{collections::BTreeMap, sync::Arc};
mod border;
mod environment;
mod heightmap;
mod random;
pub use border::Border;
pub use heightmap::SurfaceHeightmap;
#[derive(Clone)]
pub struct Palette {
    pub bits: u8,
    pub values: Vec<u32>,
    pub words: Vec<u64>,
    pub size: usize,
}
impl Palette {
    fn read(r: &mut Reader<'_>, size: usize, indirect: u8) -> Result<Self> {
        let raw = r.u8()?;
        let bits = if raw == 0 {
            0
        } else if size == 4096 && raw < 4 {
            4
        } else {
            raw
        };
        if bits > 32 {
            return Err("palette bit width exceeds bound".into());
        }
        let values = if bits == 0 {
            vec![r.count(1000000)? as u32]
        } else if bits <= indirect {
            let n = r.count(1usize << bits)?;
            if n == 0 {
                return Err("empty indirect palette".into());
            }
            (0..n)
                .map(|_| r.count(1000000).map(|v| v as u32))
                .collect::<Result<_>>()?
        } else {
            Vec::new()
        };
        let n = if bits == 0 {
            0
        } else {
            size.div_ceil(64 / usize::from(bits))
        };
        let words = (0..n)
            .map(|_| r.i64().map(|v| v as u64))
            .collect::<Result<_>>()?;
        let p = Self {
            bits,
            values,
            words,
            size,
        };
        if !p.values.is_empty() {
            for i in 0..size {
                p.get_checked(i)?;
            }
        }
        Ok(p)
    }
    fn get_checked(&self, i: usize) -> Result<u32> {
        if self.bits == 0 {
            return Ok(self.values[0]);
        }
        let per = 64 / usize::from(self.bits);
        let raw = ((self.words[i / per] >> ((i % per) * usize::from(self.bits)))
            & ((1u64 << self.bits) - 1)) as u32;
        if self.values.is_empty() {
            Ok(raw)
        } else {
            self.values
                .get(raw as usize)
                .copied()
                .ok_or("palette index out of bounds".into())
        }
    }
    pub fn get(&self, i: usize) -> u32 {
        self.get_checked(i).expect("palette validated on decode")
    }
    pub fn set(&mut self, i: usize, v: u32) {
        if self.get(i) == v {
            return;
        }
        let indirect_max = if self.size == 4096 { 8 } else { 3 };
        let raw = if !self.values.is_empty() {
            if let Some(index) = self.values.iter().position(|value| *value == v) {
                index as u32
            } else if self.values.len() < 1usize << indirect_max {
                self.values.push(v);
                let minimum = if self.size == 4096 { 4 } else { 1 };
                let bits =
                    ((usize::BITS - (self.values.len() - 1).leading_zeros()) as u8).max(minimum);
                if bits > self.bits {
                    self.repack(bits, false);
                }
                (self.values.len() - 1) as u32
            } else {
                let maximum = self.values.iter().copied().max().unwrap_or(0).max(v);
                self.repack((32 - maximum.leading_zeros()) as u8, true);
                v
            }
        } else {
            let bits = (32 - v.leading_zeros()).max(1) as u8;
            if bits > self.bits {
                self.repack(bits, true);
            }
            v
        };
        let per = 64 / usize::from(self.bits);
        let shift = (i % per) * usize::from(self.bits);
        let mask = (1u64 << self.bits) - 1;
        self.words[i / per] = (self.words[i / per] & !(mask << shift)) | (u64::from(raw) << shift);
    }
    fn repack(&mut self, bits: u8, direct: bool) {
        let per = 64 / usize::from(bits);
        let mut words = vec![0; self.size.div_ceil(per)];
        if self.bits != 0 || direct {
            for i in 0..self.size {
                let value = if direct {
                    self.get(i)
                } else {
                    let old_per = 64 / usize::from(self.bits);
                    ((self.words[i / old_per] >> ((i % old_per) * usize::from(self.bits)))
                        & ((1u64 << self.bits) - 1)) as u32
                };
                words[i / per] |= u64::from(value) << ((i % per) * usize::from(bits));
            }
        }
        self.bits = bits;
        self.words = words;
        if direct {
            self.values = Vec::new();
        }
    }
}
#[derive(Clone)]
pub struct Section {
    pub blocks: Palette,
    pub biomes: Palette,
    pub non_empty_block_count: u16,
}
#[derive(Clone)]
pub struct Chunk {
    pub sections: Vec<Arc<Section>>,
    pub sky: BTreeMap<usize, Arc<Vec<u8>>>,
    pub block: BTreeMap<usize, Arc<Vec<u8>>>,
    pub block_entities: Arc<Vec<ChunkBlockEntity>>,
    pub surface_heightmap: Arc<SurfaceHeightmap>,
    pub surface_heightmap_received: bool,
}
#[derive(Clone)]
pub struct ChunkBlockEntity {
    pub position: [i32; 3],
    pub kind: u32,
    pub data: Arc<serde_json::Value>,
    pub raw_nbt: Arc<[u8]>,
}
pub(crate) fn empty_block_entities() -> Arc<Vec<ChunkBlockEntity>> {
    static EMPTY: std::sync::LazyLock<Arc<Vec<ChunkBlockEntity>>> =
        std::sync::LazyLock::new(|| Arc::new(Vec::new()));
    EMPTY.clone()
}
impl Chunk {
    pub fn resident_bytes(&self) -> usize {
        let sections = self
            .sections
            .iter()
            .map(|s| {
                std::mem::size_of::<Section>()
                    + s.blocks.values.capacity() * 4
                    + s.blocks.words.capacity() * 8
                    + s.biomes.values.capacity() * 4
                    + s.biomes.words.capacity() * 8
            })
            .sum::<usize>();
        let light = [&self.sky, &self.block]
            .into_iter()
            .flat_map(|map| map.values())
            .map(|bytes| bytes.capacity() + 128)
            .sum::<usize>();
        std::mem::size_of::<Self>()
            + self.sections.capacity() * std::mem::size_of::<Arc<Section>>()
            + sections
            + light
            + self.surface_heightmap.resident_bytes()
            + self.block_entities.capacity() * std::mem::size_of::<ChunkBlockEntity>()
            + self
                .block_entities
                .iter()
                .map(|e| value_residency(&e.data) + e.raw_nbt.len() + 32)
                .sum::<usize>()
    }
    pub fn decode(
        r: &mut Reader<'_>,
        count: usize,
        is_air: impl FnMut(u32) -> Result<bool>,
    ) -> Result<([i32; 2], Self)> {
        let x = r.i32()?;
        let z = r.i32()?;
        let hm = r.count(16)?;
        let mut surface_words = None;
        for _ in 0..hm {
            let kind = r.var()?;
            if !(0..6).contains(&kind) {
                return Err("unknown chunk heightmap type".into());
            }
            let words = r.longs(256)?;
            if kind == 1 {
                surface_words = Some(words);
            }
        }
        let raw = r.array(2 * 1024 * 1024)?;
        let mut body = Reader::new(raw);
        let mut sections = Vec::with_capacity(count);
        for _ in 0..count {
            let n = body.i16()?;
            if !(0..=4096).contains(&n) {
                return Err("invalid section block count".into());
            }
            let fluid_count = body.i16()?;
            if !(0..=4096).contains(&fluid_count) {
                return Err("invalid section fluid count".into());
            }
            sections.push(Arc::new(Section {
                blocks: Palette::read(&mut body, 4096, 8)?,
                biomes: Palette::read(&mut body, 64, 3)?,
                non_empty_block_count: n as u16,
            }));
        }
        body.done()?;
        let n = r.count(65536)?;
        let mut block_entities = Vec::with_capacity(n);
        for _ in 0..n {
            let xz = r.u8()?;
            let y = r.i16()?;
            let kind = r.count(65536)? as u32;
            let nbt_start = r.offset;
            let data = Arc::new(r.nbt()?);
            let raw_nbt = Arc::from(&r.bytes[nbt_start..r.offset]);
            block_entities.push(ChunkBlockEntity {
                position: [
                    x * 16 + i32::from(xz >> 4),
                    i32::from(y),
                    z * 16 + i32::from(xz & 15),
                ],
                kind,
                data,
                raw_nbt,
            });
        }
        let surface_heightmap_received = surface_words.is_some();
        let surface_heightmap = Arc::new(SurfaceHeightmap::received(
            i32::try_from(count)
                .ok()
                .and_then(|n| n.checked_mul(16))
                .ok_or("chunk dimension height overflow")?,
            surface_words,
            &sections,
            is_air,
        )?);
        let mut out = Self {
            sections,
            surface_heightmap,
            surface_heightmap_received,
            sky: BTreeMap::new(),
            block: BTreeMap::new(),
            block_entities: if block_entities.is_empty() {
                empty_block_entities()
            } else {
                Arc::new(block_entities)
            },
        };
        out.light(r)?;
        r.done()?;
        Ok(([x, z], out))
    }
    pub fn light(&mut self, r: &mut Reader<'_>) -> Result<()> {
        let sky = r.longs(16)?;
        let block = r.longs(16)?;
        let empty_sky = r.longs(16)?;
        let empty_block = r.longs(16)?;
        for (maps, mask, empty) in [
            (&mut self.sky, sky, empty_sky),
            (&mut self.block, block, empty_block),
        ] {
            let count = r.count(1024)?;
            let bits: Vec<_> = mask
                .iter()
                .enumerate()
                .flat_map(|(i, v)| {
                    (0..64)
                        .filter(move |b| v & (1u64 << b) != 0)
                        .map(move |b| i * 64 + b)
                })
                .collect();
            if count != bits.len() {
                return Err("light mask and arrays disagree".into());
            }
            for bit in bits {
                let bytes = r.array(2048)?;
                if bytes.len() != 2048 {
                    return Err("invalid light layer length".into());
                }
                maps.insert(bit, Arc::new(bytes.to_vec()));
            }
            for (i, v) in empty.iter().enumerate() {
                for b in 0..64 {
                    if v & (1u64 << b) != 0 {
                        maps.insert(i * 64 + b, Arc::new(vec![0; 2048]));
                    }
                }
            }
        }
        Ok(())
    }
}
#[derive(Default)]
pub struct World {
    chunks: BTreeMap<[i32; 2], Arc<Chunk>>,
    pub min_y: i32,
    pub height: i32,
    pub border: Border,
    resident_bytes: usize,
    chunk_lifecycle_revision: u64,
    random: random::RandomSources,
    pub(crate) environment: crate::environment::PointState,
    environment_context: environment::Context,
}
impl World {
    pub fn new(min_y: i32, height: i32) -> Self {
        Self {
            min_y,
            height,
            ..Self::default()
        }
    }
    pub fn initialize_random_sources(&self) -> Result<()> {
        self.random.initialize()
    }
    pub fn sound_seed(&self) -> Result<u64> {
        self.random.sound_seed()
    }
    pub fn local_sound_seed(&self) -> Result<u64> {
        self.random.local_sound_seed()
    }
    pub fn next_float(&self) -> Result<f32> {
        self.random.next_float()
    }
    pub fn next_int_bound(&self, bound: u32) -> Result<u32> {
        self.random.next_int_bound(bound)
    }
    pub fn next_double(&self) -> Result<f64> {
        self.random.next_double()
    }
    pub fn chunk_count(&self) -> usize {
        self.chunks.len()
    }
    /// Invalidates terrain view membership without scanning all loaded columns
    /// on idle turns. Replacing a column (including its section layout) counts.
    pub(crate) fn chunk_lifecycle_revision(&self) -> u64 {
        self.chunk_lifecycle_revision
    }
    pub fn resident_bytes(&self) -> usize {
        self.resident_bytes
    }
    pub fn chunk(&self, pos: [i32; 2]) -> Option<&Chunk> {
        self.chunks.get(&pos).map(Arc::as_ref)
    }
    pub fn chunks(&self) -> impl ExactSizeIterator<Item = (&[i32; 2], &Arc<Chunk>)> {
        self.chunks.iter()
    }
    pub fn insert(&mut self, pos: [i32; 2], mut chunk: Chunk) -> Result<()> {
        if self.chunks.len() >= 2048 && !self.chunks.contains_key(&pos) {
            return Err("client chunk residency count bound reached".into());
        }
        if !chunk.surface_heightmap_received {
            if let Some(previous) = self.chunks.get(&pos) {
                chunk.surface_heightmap = previous.surface_heightmap.clone();
            }
        }
        let old_bytes = self.chunks.get(&pos).map_or(0, |c| c.resident_bytes());
        let next_bytes = self.resident_bytes - old_bytes + chunk.resident_bytes();
        if next_bytes > 512 * 1024 * 1024 {
            return Err("client decoded chunk residency byte bound reached".into());
        }
        self.chunks.insert(pos, Arc::new(chunk));
        self.resident_bytes = next_bytes;
        self.chunk_lifecycle_revision = self.chunk_lifecycle_revision.wrapping_add(1);
        Ok(())
    }
    pub fn remove(&mut self, pos: [i32; 2]) {
        if let Some(chunk) = self.chunks.remove(&pos) {
            self.resident_bytes -= chunk.resident_bytes();
            self.chunk_lifecycle_revision = self.chunk_lifecycle_revision.wrapping_add(1);
        }
    }
    pub fn clear(&mut self) {
        if !self.chunks.is_empty() {
            self.chunk_lifecycle_revision = self.chunk_lifecycle_revision.wrapping_add(1);
        }
        self.chunks.clear();
        self.resident_bytes = 0;
    }
    pub fn update_light(&mut self, pos: [i32; 2], r: &mut Reader<'_>) -> Result<()> {
        let Some(old) = self.chunks.get(&pos) else {
            return Err("light update for unknown chunk".into());
        };
        let mut staged = (**old).clone();
        staged.light(r)?;
        r.done()?;
        self.insert(pos, staged)
    }
    pub fn snapshot(&self, center: [i32; 2]) -> Neighborhood {
        Neighborhood {
            center,
            min_y: self.min_y,
            height: self.height,
            biome_zoom_seed: self.environment_context.biome_zoom_seed,
            chunks: std::array::from_fn(|i| {
                let offset = [(i % 3) as i32 - 1, (i / 3) as i32 - 1];
                self.chunks
                    .get(&[center[0] + offset[0], center[1] + offset[1]])
                    .cloned()
            }),
        }
    }
    pub fn light_levels(&self, p: [i32; 3]) -> [Option<u8>; 2] {
        light_levels(
            self.chunks.get(&chunk_position(p)).map(Arc::as_ref),
            self.min_y,
            self.height,
            p,
        )
    }
    pub fn light_at(&self, p: [i32; 3]) -> Option<u8> {
        combined_light(self.light_levels(p))
    }
    pub fn biome(&self, p: [i32; 3]) -> Option<u32> {
        biome(
            self.chunks.get(&chunk_position(p)).map(Arc::as_ref),
            self.min_y,
            self.height,
            p,
        )
    }
    pub fn border_contains_block(&self, position: [i32; 3]) -> bool {
        self.border.contains(position.map(f64::from))
    }
    pub fn border_contains_point(&self, position: [f64; 3]) -> bool {
        self.border.contains(position)
    }
    pub fn surface_height(&self, position: [i32; 2]) -> Option<i32> {
        let chunk = self
            .chunks
            .get(&[position[0].div_euclid(16), position[1].div_euclid(16)])?;
        Some(
            chunk
                .surface_heightmap
                .first_available(position, self.min_y),
        )
    }
    pub fn block(&self, p: [i32; 3]) -> Option<u32> {
        block(
            self.chunks.get(&chunk_position(p)).map(Arc::as_ref),
            self.min_y,
            self.height,
            p,
        )
    }
    pub fn set(
        &mut self,
        p: [i32; 3],
        state: u32,
        mut is_air: impl FnMut(u32) -> Result<bool>,
    ) -> Result<()> {
        let s =
            section_index(self.min_y, self.height, p[1]).ok_or("block update outside dimension")?;
        if let Some(chunk) = self.chunks.get_mut(&chunk_position(p)) {
            let old_state = chunk.sections[s].blocks.get(block_index(p));
            if old_state == state {
                return Ok(());
            }
            let old_air = is_air(old_state)?;
            let air = is_air(state)?;
            let next_height = chunk
                .surface_heightmap
                .next_height(p, self.min_y, air, |y| {
                    is_air(
                        chunk.sections[((y - self.min_y) / 16) as usize]
                            .blocks
                            .get(block_index([p[0], y, p[2]])),
                    )
                })?;
            let old_bytes = chunk.resident_bytes();
            let growth_bound = if chunk.sections[s].blocks.bits == 32 {
                0
            } else {
                chunk.sections[s].blocks.size.div_ceil(2) * 8
            };
            if self.resident_bytes + growth_bound > 512 * 1024 * 1024 {
                return Err("client decoded chunk residency byte bound reached".into());
            }
            let chunk = Arc::make_mut(chunk);
            let section = Arc::make_mut(&mut chunk.sections[s]);
            section.blocks.set(block_index(p), state);
            section.non_empty_block_count = (i32::from(section.non_empty_block_count)
                + i32::from(old_air)
                - i32::from(air)) as u16;
            if let Some(height) = next_height {
                Arc::make_mut(&mut chunk.surface_heightmap).set_offset([p[0], p[2]], height);
            }
            self.resident_bytes = self.resident_bytes - old_bytes + chunk.resident_bytes();
        }
        Ok(())
    }
}

/// Nine immutable chunk handles suffice for face/AO/fluid/tint neighborhood reads.
/// A worker never clones palettes and cannot observe a half-applied update.
pub struct Neighborhood {
    pub center: [i32; 2],
    pub min_y: i32,
    pub height: i32,
    biome_zoom_seed: Option<i64>,
    chunks: [Option<Arc<Chunk>>; 9],
}
impl Neighborhood {
    pub fn center_chunk(&self) -> Option<&Chunk> {
        self.chunks[4].as_deref()
    }
    fn chunk(&self, p: [i32; 3]) -> Option<&Chunk> {
        let key = chunk_position(p);
        let x = i64::from(key[0]) - i64::from(self.center[0]) + 1;
        let z = i64::from(key[1]) - i64::from(self.center[1]) + 1;
        if !(0..3).contains(&x) || !(0..3).contains(&z) {
            return None;
        }
        self.chunks[(z * 3 + x) as usize].as_deref()
    }
    pub fn block(&self, p: [i32; 3]) -> Option<u32> {
        block(self.chunk(p), self.min_y, self.height, p)
    }
    pub fn biome(&self, p: [i32; 3]) -> Option<u32> {
        biome(self.chunk(p), self.min_y, self.height, p)
    }
    pub fn light_levels(&self, p: [i32; 3]) -> [Option<u8>; 2] {
        light_levels(self.chunk(p), self.min_y, self.height, p)
    }
    pub fn light_at(&self, p: [i32; 3]) -> Option<u8> {
        combined_light(self.light_levels(p))
    }
}
fn chunk_position(p: [i32; 3]) -> [i32; 2] {
    [p[0].div_euclid(16), p[2].div_euclid(16)]
}
fn block_index(p: [i32; 3]) -> usize {
    (p[1].rem_euclid(16) * 256 + p[2].rem_euclid(16) * 16 + p[0].rem_euclid(16)) as usize
}
fn section_index(min_y: i32, height: i32, y: i32) -> Option<usize> {
    let section = (i64::from(y) - i64::from(min_y)).div_euclid(16);
    (section >= 0 && section < i64::from(height) / 16).then_some(section as usize)
}
fn block(chunk: Option<&Chunk>, min_y: i32, height: i32, p: [i32; 3]) -> Option<u32> {
    let section = section_index(min_y, height, p[1])?;
    Some(chunk?.sections[section].blocks.get(block_index(p)))
}
fn biome(chunk: Option<&Chunk>, min_y: i32, height: i32, p: [i32; 3]) -> Option<u32> {
    let section = section_index(min_y, height, p[1])?;
    let index =
        p[1].rem_euclid(16) / 4 * 16 + p[2].rem_euclid(16) / 4 * 4 + p[0].rem_euclid(16) / 4;
    Some(chunk?.sections[section].biomes.get(index as usize))
}
fn light_levels(chunk: Option<&Chunk>, min_y: i32, height: i32, p: [i32; 3]) -> [Option<u8>; 2] {
    let section = (i64::from(p[1]) - i64::from(min_y)).div_euclid(16);
    // Received light arrays include one layer below and above the dimension.
    if section < -1 || section > i64::from(height) / 16 {
        return [None; 2];
    }
    let Some(chunk) = chunk else { return [None; 2] };
    let layer = (section + 1) as usize;
    let index = block_index(p);
    [&chunk.sky, &chunk.block].map(|map| {
        map.get(&layer)
            .map(|a| (a[index / 2] >> ((index % 2) * 4)) & 15)
    })
}
fn combined_light(levels: [Option<u8>; 2]) -> Option<u8> {
    match levels {
        [None, None] => None,
        [sky, block] => Some(sky.unwrap_or(0).max(block.unwrap_or(0))),
    }
}

fn value_residency(value: &serde_json::Value) -> usize {
    use serde_json::Value;
    std::mem::size_of::<Value>()
        + match value {
            Value::String(s) => s.capacity(),
            Value::Array(v) => {
                v.capacity() * std::mem::size_of::<Value>()
                    + v.iter().map(value_residency).sum::<usize>()
            }
            Value::Object(v) => v
                .iter()
                .map(|(key, value)| 128 + key.capacity() + value_residency(value))
                .sum(),
            _ => 0,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uniform(size: usize, value: u32) -> Palette {
        Palette {
            bits: 0,
            values: vec![value],
            words: Vec::new(),
            size,
        }
    }
    fn chunk(value: u32, biome: u32) -> Chunk {
        Chunk {
            sections: vec![Arc::new(Section {
                blocks: uniform(4096, value),
                biomes: uniform(64, biome),
                non_empty_block_count: 4096,
            })],
            sky: BTreeMap::new(),
            block: BTreeMap::new(),
            block_entities: empty_block_entities(),
            surface_heightmap: Arc::new(SurfaceHeightmap::new(16).unwrap()),
            surface_heightmap_received: false,
        }
    }

    #[test]
    fn palette_edits_preserve_cells_through_indirect_and_direct_growth() {
        let mut palette = uniform(4096, 0);
        for i in 0..300 {
            palette.set(i, i as u32 + 1);
        }
        palette.set(4095, 65535);
        palette.set(20, 0);
        for i in 0..4096 {
            let expected = if i == 4095 {
                65535
            } else if i < 300 && i != 20 {
                i as u32 + 1
            } else {
                0
            };
            assert_eq!(palette.get(i), expected, "cell {i}");
        }
    }

    #[test]
    fn immutable_neighborhood_preserves_prior_state_on_negative_chunk_boundary() {
        let mut world = World::new(-64, 16);
        world.insert([-1, -1], chunk(3, 7)).unwrap();
        world.insert([0, -1], chunk(4, 9)).unwrap();
        let before = world.snapshot([-1, -1]);
        world.set([-1, -64, -1], 5, |_| Ok(false)).unwrap();
        world.remove([0, -1]);
        assert_eq!(world.block([-1, -64, -1]), Some(5));
        assert_eq!(before.block([-1, -64, -1]), Some(3));
        assert_eq!(before.block([0, -64, -1]), Some(4));
        assert_eq!(world.block([0, -64, -1]), None);
        assert_eq!(before.biome([-1, -64, -1]), Some(7));
        assert_eq!(before.biome([0, -64, -1]), Some(9));
        assert_eq!(before.block([-1, -65, -1]), None);
        assert_eq!(before.biome([-1, -48, -1]), None);
    }

    #[test]
    fn rejected_chunk_admission_keeps_received_world_coherent() {
        let mut world = World::new(0, 16);
        for x in 0..2048 {
            world.insert([x, 0], chunk(x as u32, 0)).unwrap();
        }
        let bytes = world.resident_bytes();
        assert!(world.insert([2048, 0], chunk(2048, 0)).is_err());
        assert_eq!(world.block([2048 * 16, 0, 0]), None);
        assert_eq!(world.block([2047 * 16, 0, 0]), Some(2047));
        assert_eq!(world.resident_bytes(), bytes);
        world.remove([2047, 0]);
        world.insert([2048, 0], chunk(2048, 0)).unwrap();
        assert_eq!(world.block([2048 * 16, 0, 0]), Some(2048));
    }

    #[test]
    fn chunk_lifecycle_revision_detects_same_count_replacement_and_unload() {
        let mut world = World::new(0, 16);
        let empty = world.chunk_lifecycle_revision();
        world.insert([0, 0], chunk(1, 0)).unwrap();
        let inserted = world.chunk_lifecycle_revision();
        assert_ne!(empty, inserted);
        world.insert([0, 0], chunk(2, 0)).unwrap();
        assert_eq!(world.chunk_count(), 1);
        let replaced = world.chunk_lifecycle_revision();
        assert_ne!(inserted, replaced);
        world.remove([9, 9]);
        assert_eq!(world.chunk_lifecycle_revision(), replaced);
        world.remove([0, 0]);
        assert_ne!(world.chunk_lifecycle_revision(), replaced);
        let removed = world.chunk_lifecycle_revision();
        world.clear();
        assert_eq!(world.chunk_lifecycle_revision(), removed);
        world.insert([1, 0], chunk(3, 0)).unwrap();
        let filled = world.chunk_lifecycle_revision();
        world.clear();
        assert_ne!(world.chunk_lifecycle_revision(), filled);
    }
}
