//! Far terrain beyond the server view distance (Solaris `[lod]`, overworld).
//!
//! The world is cut into tiles of 16x16 point samples spaced `1 << level`
//! blocks (levels 0..=6: 1 to 64 blocks). Tiles nearer the camera are finer:
//! a quadtree splits a tile while the camera is closer than [`SPLIT`] tile
//! widths.
//! Each sample is the top block of one column, packed for the GPU as height
//! plus provenance and an RGBA colour (texture average times biome tint).
//!
//! Samples come from, most trusted first: chunks received this session, tiles
//! the server answered from its stored chunks, the on-disk cache of earlier
//! sessions, the server's router surface for never-generated columns, and
//! columns this machine generated with the server's program (only servers
//! whose operator shares it; see `generation`). A sample only replaces one of
//! equal or lower trust. Player edits far away are shown only after the chunk
//! is received again or its tile is asked for again (each tile once per
//! session); the cache stores colours and heights only, never the program.
mod generation;
pub(crate) mod program;

use crate::api::{FarGeneration, FarScene, FarTileDraw};
use crate::protocol::{
    content::{Content, FluidKind, TintKind},
    world::Chunk,
    Reader, Result,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub(crate) const EDGE: usize = 16;
pub(crate) const SAMPLES: usize = EDGE * EDGE;
/// Two words per sample: height+source, RGBA8 colour.
pub(crate) const WORDS: usize = SAMPLES * 2;
/// Finest level: 1-block samples, so the ring right past the view distance
/// matches the real terrain's columns.
pub(crate) const MIN_LEVEL: u8 = 0;
pub(crate) const MAX_LEVEL: u8 = 6;
pub(crate) const DEFAULT_DISTANCE: u32 = 2048;
pub(crate) const LOW_PRESET_DISTANCE: u32 = 1024;
pub(crate) const MAX_DISTANCE: u32 = 8192;
const PROTOCOL: u8 = 1;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Cache files read per update; a file is one tile (2 KiB).
const CACHE_LOADS_PER_UPDATE: usize = 24;
/// Tiles in one request payload (the server accepts up to 16).
const TILES_PER_REQUEST: usize = 16;
/// Bumped whenever cached colours change meaning (SFL1 stored gray cubes).
const CACHE_MAGIC: &[u8; 4] = b"SFL2";
/// Off-screen tiles fed by received chunks stay resident this long.
const EVICT_AFTER: Duration = Duration::from_secs(30);

/// Sample provenance, ordered by trust.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub(crate) enum Source {
    Missing = 0,
    Generated = 1,
    Router = 2,
    Cached = 3,
    Stored = 4,
    Received = 5,
}
impl Source {
    fn from_bits(bits: u32) -> Self {
        match bits & 15 {
            1 => Self::Generated,
            2 => Self::Router,
            3 => Self::Cached,
            4 => Self::Stored,
            5 => Self::Received,
            _ => Self::Missing,
        }
    }
}

pub(crate) fn pack(height: i32, source: Source) -> u32 {
    u32::from(height.clamp(-32768, 32767) as i16 as u16) | (source as u32) << 16
}
pub(crate) fn unpack(word: u32) -> (i32, Source) {
    (i32::from(word as u16 as i16), Source::from_bits(word >> 16))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct TileKey {
    pub level: u8,
    pub x: i32,
    pub z: i32,
}
impl TileKey {
    pub fn spacing(self) -> i64 {
        1 << self.level
    }
    pub fn size(self) -> i64 {
        EDGE as i64 * self.spacing()
    }
    pub fn origin(self) -> [i64; 2] {
        [
            i64::from(self.x) * self.size(),
            i64::from(self.z) * self.size(),
        ]
    }
    /// Horizontal distance from `point` to the tile's square (0 inside).
    fn distance(self, point: [f64; 2]) -> f64 {
        let size = self.size() as f64;
        let origin = self.origin().map(|v| v as f64);
        let gap = |axis: usize| {
            (origin[axis] - point[axis])
                .max(point[axis] - origin[axis] - size)
                .max(0.)
        };
        gap(0).hypot(gap(1))
    }
    fn children(self) -> [Self; 4] {
        std::array::from_fn(|i| Self {
            level: self.level - 1,
            x: self.x * 2 + (i & 1) as i32,
            z: self.z * 2 + (i >> 1) as i32,
        })
    }
    /// The tile at `level` containing block column `[x, z]`.
    fn containing(level: u8, column: [i64; 2]) -> Self {
        let size = EDGE as i64 * (1 << level);
        Self {
            level,
            x: column[0].div_euclid(size) as i32,
            z: column[1].div_euclid(size) as i32,
        }
    }
}

/// A tile splits while the camera is nearer than this many of its widths:
/// each level's ring is twice as wide as the previous one, and 8 keeps 1-block
/// samples out to ~280 blocks and 8-block samples at 2048.
pub(crate) const SPLIT: f64 = 8.;

/// Quadtree leaves within `distance` of `centre`, nearest first (rings per
/// [`SPLIT`]). `covered` tiles (every chunk loaded as real terrain) are left
/// out.
pub(crate) fn desired_tiles(
    centre: [f64; 2],
    distance: f64,
    covered: &dyn Fn(TileKey) -> bool,
) -> Vec<TileKey> {
    fn visit(
        key: TileKey,
        centre: [f64; 2],
        distance: f64,
        covered: &dyn Fn(TileKey) -> bool,
        out: &mut Vec<(f64, TileKey)>,
    ) {
        let d = key.distance(centre);
        if d > distance {
            return;
        }
        if key.level > MIN_LEVEL && d < SPLIT * key.size() as f64 {
            for child in key.children() {
                visit(child, centre, distance, covered, out);
            }
        } else if !covered(key) {
            out.push((d, key));
        }
    }
    let root = TileKey {
        level: MAX_LEVEL,
        x: 0,
        z: 0,
    }
    .size() as f64;
    let range = |axis: usize| {
        ((centre[axis] - distance) / root).floor() as i32
            ..=((centre[axis] + distance) / root).floor() as i32
    };
    let mut out = Vec::new();
    for z in range(1) {
        for x in range(0) {
            let key = TileKey {
                level: MAX_LEVEL,
                x,
                z,
            };
            visit(key, centre, distance, covered, &mut out);
        }
    }
    out.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    out.into_iter().map(|(_, key)| key).collect()
}

struct Tile {
    data: Arc<[u32; WORDS]>,
    revision: u64,
    /// Changed since it was last written to the cache.
    dirty: bool,
    /// Asked of the server this session (answered or in flight).
    requested: bool,
    /// Last chunk ingest; off-screen tiles stay resident a while after it.
    touched: Instant,
    /// Handed to generation this session (or found complete).
    generated: bool,
}
impl Tile {
    fn empty() -> Self {
        Self {
            data: Arc::new([0; WORDS]),
            revision: 0,
            dirty: false,
            requested: false,
            touched: Instant::now(),
            generated: false,
        }
    }
    fn has_samples(&self) -> bool {
        self.data
            .chunks_exact(2)
            .any(|s| unpack(s[0]).1 != Source::Missing)
    }
    fn missing(&self) -> bool {
        self.data
            .chunks_exact(2)
            .any(|s| unpack(s[0]).1 == Source::Missing)
    }
    /// Every sample already comes from stored or received chunks.
    fn authoritative(&self) -> bool {
        self.data
            .chunks_exact(2)
            .all(|s| unpack(s[0]).1 >= Source::Stored)
    }
}

/// What the server announced in `solaris:lod/hello`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ServerLod {
    pub max_distance: u32,
    pub max_inflight: usize,
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    pub keys: bool,
}

pub(crate) fn decode_hello(payload: &[u8]) -> Result<ServerLod> {
    let mut r = Reader::new(payload);
    if r.u8()? != PROTOCOL {
        return Err("unsupported far terrain protocol".into());
    }
    let flags = r.u8()?;
    let max_distance = u32::from_be_bytes(r.take(4)?.try_into().unwrap());
    let max_inflight = u16::from_be_bytes(r.take(2)?.try_into().unwrap());
    let min_y = r.i32()?;
    let height = r.i32()?;
    let sea_level = r.i32()?;
    let _worldgen_revision = r.i32()?;
    r.done()?;
    if !(1..=4096).contains(&height) || max_inflight == 0 {
        return Err("invalid far terrain hello".into());
    }
    Ok(ServerLod {
        max_distance,
        max_inflight: usize::from(max_inflight.min(64)),
        min_y,
        height,
        sea_level,
        keys: flags & 1 != 0,
    })
}

/// One decoded server sample: source, height, block state, biome id.
pub(crate) type WireSample = (Source, i32, Option<u32>, Option<u32>);

pub(crate) fn decode_tile(payload: &[u8]) -> Result<(TileKey, Vec<WireSample>)> {
    let mut r = Reader::new(payload);
    if r.u8()? != PROTOCOL {
        return Err("unsupported far terrain protocol".into());
    }
    let x = r.i32()?;
    let z = r.i32()?;
    let level = r.u8()?;
    let _flags = r.u8()?;
    if !(MIN_LEVEL..=MAX_LEVEL).contains(&level) {
        return Err("far terrain tile level out of range".into());
    }
    let mut samples = Vec::with_capacity(SAMPLES);
    for _ in 0..SAMPLES {
        let source = match r.u8()? {
            0 => Source::Missing,
            1 => Source::Stored,
            2 => Source::Router,
            _ => return Err("unknown far terrain sample origin".into()),
        };
        let height = i32::from(r.i16()?);
        let optional = |v: i32| u32::try_from(v).ok().and_then(|v| v.checked_sub(1));
        let block = optional(r.var()?);
        let biome = optional(r.var()?);
        samples.push((source, height, block, biome));
    }
    r.done()?;
    Ok((TileKey { level, x, z }, samples))
}

pub(crate) fn encode_request(tiles: &[(TileKey, bool)]) -> Vec<u8> {
    let mut out = vec![PROTOCOL, tiles.len() as u8];
    for (key, stored_only) in tiles {
        out.extend(key.x.to_be_bytes());
        out.extend(key.z.to_be_bytes());
        out.push(key.level);
        out.push(u8::from(*stored_only));
    }
    out
}

/// Block and biome colours, resolved lazily from the received content.
struct Colours {
    /// Untinted texture average and tint per block state.
    states: HashMap<u32, ([f32; 3], TintKind)>,
    /// Grass, foliage, dry foliage, water tint and name per biome id.
    biomes: Vec<([[f32; 3]; 4], String)>,
    water: [f32; 3],
    lava: [f32; 3],
    /// Surface block for router columns, by name.
    surfaces: HashMap<&'static str, u32>,
}
impl Colours {
    fn new(content: &Content) -> Result<Self> {
        let mut biomes = Vec::new();
        for (name, source) in content.registry_entries("minecraft:worldgen/biome")? {
            let colors = content.biome_colors(source, 0., 0.)?;
            biomes.push((
                [
                    colors.grass,
                    colors.foliage,
                    colors.dry_foliage,
                    colors.water,
                ],
                name.clone(),
            ));
        }
        let fluid = |kind| -> Result<[f32; 3]> {
            let region = content.fluid_textures(kind)?.still;
            Ok(average(
                content,
                region.layer,
                [region.uv[0], region.uv[1], region.uv[2], region.uv[3]],
            ))
        };
        let mut surfaces = HashMap::new();
        for name in SURFACE_BLOCKS {
            if let Ok(state) =
                content.resolve_block_state(&format!("minecraft:{name}"), &Default::default())
            {
                surfaces.insert(*name, state);
            }
        }
        Ok(Self {
            states: HashMap::new(),
            biomes,
            water: fluid(FluidKind::Water)?,
            lava: fluid(FluidKind::Lava)?,
            surfaces,
        })
    }

    fn state(&mut self, content: &Content, state: u32) -> ([f32; 3], TintKind) {
        if let Some(found) = self.states.get(&state) {
            return *found;
        }
        let info = content.info(state).ok();
        // Random-variant cubes (grass, stone, sand, dirt) keep quads in parts:
        // `for_each_quad` visits both, preferring the top face.
        let mut quad: Option<([[f32; 2]; 4], u32, TintKind)> = None;
        let mut top = false;
        if let Ok(Some(model)) = content.model_for(state) {
            model.for_each_quad([0; 3], |q| {
                if !top && (quad.is_none() || q.face == 1) {
                    quad = Some((q.uv, q.texture_layer, q.tint));
                    top = q.face == 1;
                }
            });
        }
        let found = match (
            info.and_then(|info| info.fluid.map(|fluid| fluid.kind)),
            quad,
        ) {
            (Some(FluidKind::Lava), _) => (self.lava, TintKind::None),
            (Some(FluidKind::Water), None) => (self.water, TintKind::Water),
            (_, None) => ([0.5; 3], TintKind::None),
            (_, Some((uvs, layer, tint))) => {
                let (mut min, mut max) = ([1f32; 2], [0f32; 2]);
                for uv in uvs {
                    for axis in 0..2 {
                        min[axis] = min[axis].min(uv[axis]);
                        max[axis] = max[axis].max(uv[axis]);
                    }
                }
                (
                    average(content, layer, [min[0], min[1], max[0], max[1]]),
                    tint,
                )
            }
        };
        self.states.insert(state, found);
        found
    }

    /// Display-encoded RGBA8 of a block in a biome.
    fn rgba(&mut self, content: &Content, state: u32, biome: Option<u32>) -> u32 {
        let (rgb, tint) = self.state(content, state);
        let tints = biome.and_then(|id| self.biomes.get(id as usize));
        let tint = match (tint, tints) {
            (TintKind::None, _) => [1.; 3],
            (TintKind::Constant(color), _) => color,
            (TintKind::Grass, Some(t)) => t.0[0],
            (TintKind::Foliage, Some(t)) => t.0[1],
            (TintKind::DryFoliage, Some(t)) => t.0[2],
            (TintKind::Water, Some(t)) => t.0[3],
            // No received biome: a neutral plains-like tint.
            (TintKind::Water, None) => [0.25, 0.46, 0.89],
            (_, None) => [0.57, 0.74, 0.35],
        };
        let channel = |i: usize| ((rgb[i] * tint[i]).clamp(0., 1.) * 255.).round() as u32;
        channel(0) | channel(1) << 8 | channel(2) << 16 | 255 << 24
    }

    /// The block a router column shows from far away, by biome name.
    fn surface(&self, biome: Option<u32>, underwater: bool) -> Option<u32> {
        if underwater {
            return self.surfaces.get("water").copied();
        }
        let name = biome
            .and_then(|id| self.biomes.get(id as usize))
            .map_or("", |(_, name)| name.as_str());
        let name = name.strip_prefix("minecraft:").unwrap_or(name);
        let block = SURFACE_RULES
            .iter()
            .find(|(pattern, _)| name.contains(pattern))
            .map_or("grass_block", |(_, block)| block);
        self.surfaces
            .get(block)
            .or_else(|| self.surfaces.get("grass_block"))
            .copied()
    }
}

const SURFACE_BLOCKS: &[&str] = &[
    "grass_block",
    "water",
    "sand",
    "red_sand",
    "snow_block",
    "stone",
    "gravel",
    "mycelium",
    "mud",
    "oak_leaves",
    "spruce_leaves",
    "birch_leaves",
    "jungle_leaves",
    "dark_oak_leaves",
    "cherry_leaves",
    "pale_oak_leaves",
];
/// First matching biome-name fragment wins.
const SURFACE_RULES: &[(&str, &str)] = &[
    ("snowy_taiga", "snow_block"),
    ("snowy", "snow_block"),
    ("frozen", "snow_block"),
    ("ice_spikes", "snow_block"),
    ("grove", "snow_block"),
    ("jagged_peaks", "snow_block"),
    ("stony", "stone"),
    ("caves", "stone"),
    ("deep_dark", "stone"),
    ("badlands", "red_sand"),
    ("desert", "sand"),
    ("beach", "sand"),
    ("ocean", "sand"),
    ("mushroom", "mycelium"),
    ("gravelly", "gravel"),
    ("mangrove", "mud"),
    ("taiga", "spruce_leaves"),
    ("birch", "birch_leaves"),
    ("jungle", "jungle_leaves"),
    ("dark_forest", "dark_oak_leaves"),
    ("cherry", "cherry_leaves"),
    ("pale_garden", "pale_oak_leaves"),
    ("forest", "oak_leaves"),
    ("swamp", "oak_leaves"),
    ("wooded", "oak_leaves"),
];

/// Alpha-weighted mean colour of an atlas rectangle (display encoded).
fn average(content: &Content, layer: u32, uv: [f32; 4]) -> [f32; 3] {
    let side = content.width as usize;
    let page = side * side * 4;
    let to_pixel = |v: f32| ((v.clamp(0., 1.) * side as f32) as usize).min(side - 1);
    let (x0, y0) = (to_pixel(uv[0]), to_pixel(uv[1]));
    let (x1, y1) = (to_pixel(uv[2]).max(x0 + 1), to_pixel(uv[3]).max(y0 + 1));
    let mut sum = [0f32; 4];
    for y in y0..y1.min(side) {
        for x in x0..x1.min(side) {
            let offset = layer as usize * page + (y * side + x) * 4;
            let Some(pixel) = content.rgba.get(offset..offset + 4) else {
                continue;
            };
            let alpha = f32::from(pixel[3]) / 255.;
            for channel in 0..3 {
                sum[channel] += f32::from(pixel[channel]) / 255. * alpha;
            }
            sum[3] += alpha;
        }
    }
    if sum[3] <= 0. {
        return [0.5; 3];
    }
    [sum[0] / sum[3], sum[1] / sum[3], sum[2] / sum[3]]
}

#[derive(Default, Clone, Copy, Debug)]
pub(crate) struct Counters {
    pub tiles_received: u64,
    pub requests_sent: u64,
    pub cache_loads: u64,
    pub cache_writes: u64,
    pub chunks_ingested: u64,
}

/// Session-owned far terrain: tile store, requests, cache.
pub(crate) struct Far {
    /// Client setting in blocks; 0 disables far terrain.
    distance: u32,
    server: Option<ServerLod>,
    overworld: bool,
    cache_dir: Option<PathBuf>,
    tiles: HashMap<TileKey, Tile>,
    desired: Vec<TileKey>,
    desired_for: Option<([i64; 2], u64, u32)>,
    desired_at: Instant,
    inflight: HashMap<TileKey, Instant>,
    colours: Option<Colours>,
    writer: Option<mpsc::Sender<(PathBuf, Vec<u8>)>>,
    revision: u64,
    scene_revision: Option<u64>,
    pub(crate) counters: Counters,
    /// The server's generation program, when its operator shares it.
    generation: Option<generation::Generation>,
    /// The renderer generates tiles; else (headless, no compute) the CPU.
    gpu: bool,
}

impl Default for Far {
    fn default() -> Self {
        Self {
            distance: DEFAULT_DISTANCE,
            server: None,
            overworld: false,
            cache_dir: None,
            tiles: HashMap::new(),
            desired: Vec::new(),
            desired_for: None,
            desired_at: Instant::now(),
            inflight: HashMap::new(),
            colours: None,
            writer: None,
            revision: 1,
            scene_revision: None,
            counters: Counters::default(),
            generation: None,
            gpu: false,
        }
    }
}

impl Far {
    /// Far distance actually drawn: 0 unless the server offers far terrain in
    /// the overworld and the client setting is on.
    pub(crate) fn active_distance(&self) -> u32 {
        match self.server {
            Some(server) if self.overworld => self.distance.min(server.max_distance),
            _ => 0,
        }
    }

    pub(crate) fn set_distance(&mut self, distance: u32) {
        let distance = distance.min(MAX_DISTANCE);
        if distance != self.distance {
            self.distance = distance;
            self.desired_for = None;
            self.revision += 1;
        }
    }

    pub(crate) fn hello(&mut self, payload: &[u8]) -> Result<()> {
        self.server = Some(decode_hello(payload)?);
        self.desired_for = None;
        self.revision += 1;
        Ok(())
    }

    /// A new dimension (or connection): flush the old store, open this one's cache.
    pub(crate) fn enter_dimension(&mut self, dimension: &str, server_address: Option<&str>) {
        self.flush();
        self.tiles.clear();
        self.inflight.clear();
        self.desired.clear();
        self.desired_for = None;
        self.overworld = dimension == "minecraft:overworld";
        self.cache_dir = match (self.overworld, server_address, cache_root()) {
            (true, Some(address), Some(root)) => {
                Some(root.join(sanitize(address)).join("overworld"))
            }
            _ => None,
        };
        self.reset_generation();
        self.revision += 1;
    }

    /// Connection ended: write everything and forget the server's offer.
    pub(crate) fn disconnect(&mut self) {
        self.flush();
        self.tiles.clear();
        self.inflight.clear();
        self.desired.clear();
        self.desired_for = None;
        self.server = None;
        self.overworld = false;
        self.colours = None;
        self.generation = None;
        self.revision += 1;
    }

    /// Content (atlas, biomes) changed: colours are resolved again on demand.
    pub(crate) fn reset_colours(&mut self) {
        self.colours = None;
        self.forget_palette();
    }

    fn colours(&mut self, content: &Content) -> Result<&mut Colours> {
        if self.colours.is_none() {
            self.colours = Some(Colours::new(content)?);
        }
        Ok(self.colours.as_mut().expect("colours resolved above"))
    }

    /// Summaries of one received chunk for every level's samples inside it.
    pub(crate) fn ingest_chunk(
        &mut self,
        position: [i32; 2],
        chunk: &Chunk,
        min_y: i32,
        content: &Content,
    ) -> Result<()> {
        if self.active_distance() == 0 {
            return Ok(());
        }
        let air = |state: u32| content.block_is_air(state).unwrap_or(true);
        let base = [i64::from(position[0]) * 16, i64::from(position[1]) * 16];
        let mut updates: Vec<(TileKey, usize, i32, u32, Option<u32>)> = Vec::new();
        for level in MIN_LEVEL..=MAX_LEVEL {
            let spacing = 1i64 << level;
            let mut z = base[1] + (spacing - base[1].rem_euclid(spacing)) % spacing;
            while z < base[1] + 16 {
                let mut x = base[0] + (spacing - base[0].rem_euclid(spacing)) % spacing;
                while x < base[0] + 16 {
                    let local = [(x - base[0]) as usize, (z - base[1]) as usize];
                    if let Some((height, state, biome)) = top(chunk, local, min_y, &air) {
                        let key = TileKey::containing(level, [x, z]);
                        let origin = key.origin();
                        let index = ((z - origin[1]) / spacing) as usize * EDGE
                            + ((x - origin[0]) / spacing) as usize;
                        updates.push((key, index, height, state, biome));
                    }
                    x += spacing;
                }
                z += spacing;
            }
        }
        let colours = self.colours(content)?;
        let resolved: Vec<_> = updates
            .into_iter()
            .map(|(key, index, height, state, biome)| {
                (key, index, height, colours.rgba(content, state, biome))
            })
            .collect();
        for (key, index, height, rgba) in resolved {
            let tile = self.resident(key);
            tile.touched = Instant::now();
            if write(tile, index, height, rgba, Source::Received) {
                self.revision += 1;
            }
        }
        self.counters.chunks_ingested += 1;
        Ok(())
    }

    /// The in-memory tile, loaded from the cache (or empty) on first use.
    fn resident(&mut self, key: TileKey) -> &mut Tile {
        if !self.tiles.contains_key(&key) {
            let tile = self
                .tile_path(key)
                .and_then(|path| read_cache(&path))
                .inspect(|_| self.counters.cache_loads += 1)
                .unwrap_or_else(Tile::empty);
            self.tiles.insert(key, tile);
        }
        self.tiles.get_mut(&key).expect("inserted above")
    }

    /// One `solaris:lod/tile` answer.
    pub(crate) fn tile(&mut self, payload: &[u8], content: &Content) -> Result<()> {
        let (key, samples) = decode_tile(payload)?;
        self.inflight.remove(&key);
        self.counters.tiles_received += 1;
        let Some(sea_level) = self.server.map(|server| server.sea_level) else {
            return Ok(());
        };
        let colours = self.colours(content)?;
        let mut resolved = Vec::with_capacity(SAMPLES);
        for (index, (source, height, block, biome)) in samples.into_iter().enumerate() {
            if source == Source::Missing {
                continue;
            }
            let underwater = block.is_none() && height < sea_level - 1;
            let height = if underwater { sea_level - 1 } else { height };
            let Some(state) = block.or_else(|| colours.surface(biome, underwater)) else {
                continue;
            };
            resolved.push((index, height, colours.rgba(content, state, biome), source));
        }
        let tile = self.tiles.entry(key).or_insert_with(Tile::empty);
        tile.requested = true;
        let mut changed = false;
        for (index, height, rgba, source) in resolved {
            changed |= write(tile, index, height, rgba, source);
        }
        if changed {
            self.revision += 1;
        }
        Ok(())
    }

    /// Choose tiles around the camera, load cached ones, and return request
    /// payloads for the server (bounded by its in-flight limit).
    pub(crate) fn update(
        &mut self,
        camera: [f64; 3],
        loaded_revision: u64,
        loaded: &dyn Fn([i32; 2]) -> bool,
    ) -> Vec<Vec<u8>> {
        let distance = self.active_distance();
        let Some(server) = self.server.filter(|_| distance > 0) else {
            return Vec::new();
        };
        let cell = [camera[0], camera[2]].map(|v| (v / 16.).floor() as i64);
        let now = Instant::now();
        // Moving recomputes at once; chunk loads alone at most once a second.
        let stale = match self.desired_for {
            None => true,
            Some((old_cell, old_loaded, old_distance)) => {
                old_cell != cell
                    || old_distance != distance
                    || (old_loaded != loaded_revision
                        && now.duration_since(self.desired_at) >= Duration::from_secs(1))
            }
        };
        if stale {
            self.desired_for = Some((cell, loaded_revision, distance));
            self.desired_at = now;
            let covered = |key: TileKey| {
                // Only fine tiles can lie wholly inside the loaded area.
                key.level <= 3 && {
                    let chunks = (key.size() / 16) as i32;
                    let first = [key.x * chunks, key.z * chunks];
                    (0..chunks * chunks)
                        .all(|i| loaded([first[0] + i % chunks, first[1] + i / chunks]))
                }
            };
            self.desired = desired_tiles([camera[0], camera[2]], f64::from(distance), &covered);
            let keep: HashSet<TileKey> = self.desired.iter().copied().collect();
            // Tiles fed by recently received chunks stay until they settle.
            let evicted: Vec<TileKey> = self
                .tiles
                .iter()
                .filter(|(key, tile)| {
                    !keep.contains(key) && now.duration_since(tile.touched) >= EVICT_AFTER
                })
                .map(|(key, _)| *key)
                .collect();
            for key in evicted {
                self.store(key);
                self.tiles.remove(&key);
            }
            self.revision += 1;
        }
        let mut loads = 0;
        for index in 0..self.desired.len() {
            let key = self.desired[index];
            if self.tiles.contains_key(&key) {
                continue;
            }
            if loads == CACHE_LOADS_PER_UPDATE {
                break;
            }
            loads += 1;
            if self.resident(key).has_samples() {
                self.revision += 1;
            }
        }
        let expired: Vec<TileKey> = self
            .inflight
            .iter()
            .filter(|(_, sent)| now.duration_since(**sent) >= REQUEST_TIMEOUT)
            .map(|(key, _)| *key)
            .collect();
        for key in expired {
            self.inflight.remove(&key);
            if let Some(tile) = self.tiles.get_mut(&key) {
                tile.requested = false;
            }
        }
        // Holding the program, ask only for stored columns: this machine
        // generates the rest instead of the server's router.
        let stored_only = self.generation.is_some();
        let mut batch = Vec::new();
        for key in &self.desired {
            if self.inflight.len() + batch.len() >= server.max_inflight {
                break;
            }
            let Some(tile) = self.tiles.get_mut(key) else {
                break;
            };
            if tile.requested || self.inflight.contains_key(key) {
                continue;
            }
            if tile.authoritative() {
                tile.requested = true;
                continue;
            }
            tile.requested = true;
            batch.push((*key, stored_only));
        }
        let mut requests = Vec::new();
        for tiles in batch.chunks(TILES_PER_REQUEST) {
            for (key, _) in tiles {
                self.inflight.insert(*key, now);
            }
            self.counters.requests_sent += tiles.len() as u64;
            requests.push(encode_request(tiles));
        }
        self.schedule_generation(now);
        requests
    }

    /// Publish the drawable tiles when anything changed.
    pub(crate) fn scene(&mut self, scene: &mut FarScene) -> bool {
        if self.scene_revision == Some(self.revision) {
            return false;
        }
        self.scene_revision = Some(self.revision);
        scene.distance = self.active_distance() as f32;
        scene.revision = self.revision;
        scene.tiles.clear();
        let (program, tiles, revision) = self.generation_scene();
        scene.generation = FarGeneration {
            program,
            tiles,
            revision,
        };
        if scene.distance > 0. {
            for key in &self.desired {
                if let Some(tile) = self.tiles.get(key).filter(|tile| tile.has_samples()) {
                    scene.tiles.push(FarTileDraw {
                        level: key.level,
                        x: key.x,
                        z: key.z,
                        revision: tile.revision,
                        data: tile.data.clone(),
                    });
                }
            }
        }
        true
    }

    pub(crate) fn metrics(&self) -> serde_json::Value {
        serde_json::json!({
            "distance": self.active_distance(),
            "server_offer": self.server.is_some(),
            "program_offered": self.server.is_some_and(|server| server.keys),
            "desired_tiles": self.desired.len(),
            "resident_tiles": self.tiles.len(),
            "resident_bytes": self.tiles.len() * (WORDS * 4 + 64),
            "inflight": self.inflight.len(),
            "tiles_received": self.counters.tiles_received,
            "requests_sent": self.counters.requests_sent,
            "cache_loads": self.counters.cache_loads,
            "cache_writes": self.counters.cache_writes,
            "chunks_ingested": self.counters.chunks_ingested,
            "generation": self.generation_metrics(),
        })
    }

    fn tile_path(&self, key: TileKey) -> Option<PathBuf> {
        Some(
            self.cache_dir
                .as_ref()?
                .join(format!("l{}", key.level))
                .join(format!("{}_{}.bin", key.x, key.z)),
        )
    }

    /// Queue a dirty tile for the background cache writer.
    fn store(&mut self, key: TileKey) {
        let Some(path) = self.tile_path(key) else {
            return;
        };
        let Some(tile) = self.tiles.get_mut(&key).filter(|tile| tile.dirty) else {
            return;
        };
        tile.dirty = false;
        let mut bytes = Vec::with_capacity(4 + WORDS * 4);
        bytes.extend(CACHE_MAGIC);
        for word in tile.data.iter() {
            bytes.extend(word.to_le_bytes());
        }
        let writer = self.writer.get_or_insert_with(spawn_writer);
        if writer.send((path, bytes)).is_ok() {
            self.counters.cache_writes += 1;
        }
    }

    fn flush(&mut self) {
        let keys: Vec<TileKey> = self.tiles.keys().copied().collect();
        for key in keys {
            self.store(key);
        }
    }
}

impl Drop for Far {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Write one sample when `source` is at least as trusted as the present one.
fn write(tile: &mut Tile, index: usize, height: i32, rgba: u32, source: Source) -> bool {
    let current = [tile.data[index * 2], tile.data[index * 2 + 1]];
    if unpack(current[0]).1 > source {
        return false;
    }
    let next = [pack(height, source), rgba];
    if current == next {
        return false;
    }
    let data = Arc::make_mut(&mut tile.data);
    data[index * 2] = next[0];
    data[index * 2 + 1] = next[1];
    tile.revision += 1;
    tile.dirty = true;
    true
}

/// Top non-air block of a chunk column: (y, state, biome).
fn top(
    chunk: &Chunk,
    [x, z]: [usize; 2],
    min_y: i32,
    air: &dyn Fn(u32) -> bool,
) -> Option<(i32, u32, Option<u32>)> {
    for (section_index, section) in chunk.sections.iter().enumerate().rev() {
        if section.non_empty_block_count == 0 {
            continue;
        }
        for y in (0..16).rev() {
            let state = section.blocks.get(y * 256 + z * 16 + x);
            if !air(state) {
                let biome = section.biomes.get(y / 4 * 16 + z / 4 * 4 + x / 4);
                return Some((
                    min_y + section_index as i32 * 16 + y as i32,
                    state,
                    Some(biome),
                ));
            }
        }
    }
    None
}

fn cache_root() -> Option<PathBuf> {
    crate::singleplayer::data_root()
        .ok()
        .map(|root| root.join("far-terrain"))
}

fn sanitize(address: &str) -> String {
    address
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn read_cache(path: &Path) -> Option<Tile> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() != 4 + WORDS * 4 || &bytes[..4] != CACHE_MAGIC {
        return None;
    }
    let mut data = [0u32; WORDS];
    for (word, chunk) in data.iter_mut().zip(bytes[4..].chunks_exact(4)) {
        *word = u32::from_le_bytes(chunk.try_into().expect("4-byte chunk"));
    }
    // Earlier sessions' authoritative samples rank below this session's.
    for sample in data.chunks_exact_mut(2) {
        let (height, source) = unpack(sample[0]);
        if source > Source::Cached {
            sample[0] = pack(height, Source::Cached);
        }
    }
    Some(Tile {
        data: Arc::new(data),
        revision: 1,
        dirty: false,
        requested: false,
        touched: Instant::now(),
        generated: false,
    })
}

fn spawn_writer() -> mpsc::Sender<(PathBuf, Vec<u8>)> {
    let (send, receive) = mpsc::channel::<(PathBuf, Vec<u8>)>();
    let spawned = std::thread::Builder::new()
        .name("far-terrain-cache".into())
        .spawn(move || {
            while let Ok((path, bytes)) = receive.recv() {
                let written = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| {
                        let staged = path.with_extension("tmp");
                        std::fs::write(&staged, &bytes)?;
                        std::fs::rename(&staged, &path)
                    });
                if let Err(error) = written {
                    eprintln!("far terrain cache write {}: {error}", path.display());
                }
            }
        });
    if let Err(error) = spawned {
        eprintln!("far terrain cache writer unavailable: {error}");
    }
    send
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quadtree_rings_double_and_leave_no_gaps() {
        let tiles = desired_tiles([10., -20.], 2048., &|_| false);
        // Leaves never overlap and cover the disc: sum of areas >= disc area.
        let area: f64 = tiles.iter().map(|t| (t.size() * t.size()) as f64).sum();
        assert!(area >= std::f64::consts::PI * 2048. * 2048.);
        for (i, a) in tiles.iter().enumerate() {
            for b in &tiles[i + 1..] {
                let (ao, bo) = (a.origin(), b.origin());
                let overlap = (0..2)
                    .all(|axis| ao[axis] < bo[axis] + b.size() && bo[axis] < ao[axis] + a.size());
                assert!(!overlap, "{a:?} overlaps {b:?}");
            }
        }
        // Rings: a leaf did not split (at least SPLIT widths away unless at
        // the finest level) and its parent did (nearer than SPLIT parent
        // widths, so the leaf is within that plus the parent's diagonal).
        for tile in &tiles {
            let d = tile.distance([10., -20.]);
            let size = tile.size() as f64;
            if tile.level > MIN_LEVEL {
                assert!(d >= SPLIT * size, "{tile:?} at {d} should have split");
            }
            if tile.level < MAX_LEVEL {
                let parent = 2. * size;
                assert!(
                    d < parent * (SPLIT + std::f64::consts::SQRT_2),
                    "{tile:?} at {d}"
                );
            }
        }
        // Within 2048 blocks: 1-block samples nearest, 8-block at the edge,
        // and all of it fits the GPU pool (4096 tiles).
        assert!(tiles.iter().any(|t| t.level == 0) && tiles.iter().any(|t| t.level == 3));
        assert!(tiles.iter().all(|t| t.level <= 4));
        assert!(tiles.len() < 3100, "{} tiles", tiles.len());
        // Nearest first.
        assert!(
            tiles
                .windows(2)
                .all(|w| w[0].distance([10., -20.]) <= w[1].distance([10., -20.]))
        );
    }

    #[test]
    fn covered_tiles_are_left_out() {
        let all = desired_tiles([0., 0.], 512., &|_| false);
        let some = desired_tiles([0., 0.], 512., &|key| {
            key.level == 0 && key.distance([0., 0.]) < 64.
        });
        assert!(some.len() < all.len());
        assert!(
            some.iter()
                .all(|key| !(key.level == 0 && key.distance([0., 0.]) < 64.))
        );
    }

    #[test]
    fn samples_only_yield_to_equal_or_more_trusted_sources() {
        let mut tile = Tile::empty();
        assert!(write(&mut tile, 3, 70, 0xff00ff00, Source::Router));
        assert!(write(&mut tile, 3, 72, 0xff0000ff, Source::Stored));
        assert!(!write(&mut tile, 3, 60, 0xffffffff, Source::Router));
        assert_eq!(unpack(tile.data[6]), (72, Source::Stored));
        assert!(write(&mut tile, 3, -10, 1, Source::Received));
        assert_eq!(unpack(tile.data[6]), (-10, Source::Received));
        assert!(tile.dirty);
    }

    #[test]
    fn wire_round_trip_of_request_hello_and_tile() {
        let request = encode_request(&[(
            TileKey {
                level: 4,
                x: -3,
                z: 9,
            },
            true,
        )]);
        assert_eq!(request, [1, 1, 255, 255, 255, 253, 0, 0, 0, 9, 4, 1]);
        let mut hello = vec![1, 1];
        hello.extend(8192u32.to_be_bytes());
        hello.extend(4u16.to_be_bytes());
        for v in [-64i32, 384, 63, 27] {
            hello.extend(v.to_be_bytes());
        }
        let server = decode_hello(&hello).unwrap();
        assert_eq!(
            (
                server.max_distance,
                server.max_inflight,
                server.sea_level,
                server.keys
            ),
            (8192, 4, 63, true)
        );
        let mut tile = vec![1];
        tile.extend(5i32.to_be_bytes());
        tile.extend((-6i32).to_be_bytes());
        tile.extend([2, 0]);
        // Stored sample with block 300 (varint 301) and biome 0 (varint 1).
        tile.extend([1, 0, 70, 0xad, 0x02, 1]);
        for _ in 1..SAMPLES {
            tile.extend([0, 0, 0, 0, 0]);
        }
        let (key, samples) = decode_tile(&tile).unwrap();
        assert_eq!(
            key,
            TileKey {
                level: 2,
                x: 5,
                z: -6
            }
        );
        assert_eq!(samples[0], (Source::Stored, 70, Some(300), Some(0)));
        assert_eq!(samples[1], (Source::Missing, 0, None, None));
        assert!(decode_tile(&tile[..tile.len() - 1]).is_err());
    }

    #[test]
    fn cache_round_trip_demotes_authoritative_samples() {
        let dir = std::env::temp_dir().join(format!("far-cache-test-{}", std::process::id()));
        let path = dir.join("l1").join("0_0.bin");
        let mut far = Far::default();
        far.cache_dir = Some(dir.clone());
        let key = TileKey {
            level: 1,
            x: 0,
            z: 0,
        };
        let mut tile = Tile::empty();
        write(&mut tile, 0, 80, 0xff112233, Source::Received);
        write(&mut tile, 1, 50, 0xff445566, Source::Router);
        far.tiles.insert(key, tile);
        far.store(key);
        drop(far.writer.take());
        // The writer thread drains its queue before exiting; wait for the file.
        let deadline = Instant::now() + Duration::from_secs(5);
        let loaded = loop {
            if let Some(tile) = read_cache(&path) {
                break tile;
            }
            assert!(Instant::now() < deadline, "cache file never written");
            std::thread::yield_now();
        };
        assert_eq!(unpack(loaded.data[0]), (80, Source::Cached));
        assert_eq!(loaded.data[1], 0xff112233);
        assert_eq!(unpack(loaded.data[2]), (50, Source::Router));
        let _ = std::fs::remove_dir_all(dir);
    }
}
