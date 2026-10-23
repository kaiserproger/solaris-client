use super::models::shapes::{FeatureRequirements, TerrainFact, rectangles_cover};
pub use super::models::{BakedModel, BakedQuad, ModelTransform, TintKind};
use super::{
    Result,
    models::{Baker, JavaRandom},
};
use crate::api::{AtlasMip, AtlasPatch, RenderLayer};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufReader, Cursor},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

mod loader_resources;
mod loader_blocks;
mod placement;

#[derive(serde::Deserialize)]
struct ItemDefaultsReport {
    components: BTreeMap<String, Value>,
}

struct RegistryTag {
    ordered: Vec<i32>,
    sorted: Option<Vec<i32>>,
}
impl RegistryTag {
    fn new(ordered: Vec<i32>) -> Self {
        let sorted = if ordered.windows(2).all(|pair| pair[0] < pair[1]) {
            None
        } else {
            let mut sorted = ordered.clone();
            sorted.sort_unstable();
            sorted.dedup();
            Some(sorted)
        };
        Self { ordered, sorted }
    }
    fn contains(&self, id: i32) -> bool {
        self.sorted
            .as_deref()
            .unwrap_or(&self.ordered)
            .binary_search(&id)
            .is_ok()
    }
}

fn property_key<'a>(
    domains: &BTreeMap<String, Vec<String>>,
    mut value: impl FnMut(&str) -> Option<&'a str>,
) -> Result<usize> {
    let mut key = 0usize;
    for (name, values) in domains {
        let property = value(name).ok_or_else(|| format!("missing source property {name}"))?;
        let ordinal = values
            .iter()
            .position(|candidate| candidate == property)
            .ok_or_else(|| format!("invalid source property {name}={property}"))?;
        key = key
            .checked_mul(values.len())
            .and_then(|key| key.checked_add(ordinal))
            .ok_or("source property index overflow")?;
    }
    Ok(key)
}

#[derive(serde::Deserialize)]
struct BlockUseReport<'a> {
    version: &'a str,
    #[serde(borrow)]
    blocks: BTreeMap<&'a str, BlockUseSource<'a>>,
}
#[derive(serde::Deserialize)]
struct BlockUseSource<'a> {
    use_item_on_owner: &'a str,
    use_without_item_owner: &'a str,
}
#[derive(serde::Deserialize)]
struct ItemUseSource<'a> {
    #[serde(rename = "use")]
    air: &'a str,
    #[serde(rename = "useOn")]
    on_block: &'a str,
}
#[derive(serde::Deserialize)]
struct InteractionReport<'a> {
    version: &'a str,
    #[serde(borrow)]
    blocks: BTreeMap<&'a str, PlacementSource<'a>>,
    #[serde(borrow)]
    compostable_items: BTreeMap<&'a str, f32>,
    #[serde(borrow)]
    item_blocks: BTreeMap<&'a str, u32>,
    #[serde(borrow)]
    item_spawn_types: BTreeMap<&'a str, &'a str>,
}
#[derive(serde::Deserialize)]
struct PlacementSource<'a> {
    can_open_by_hand: Option<bool>,
    default_state: u32,
    replaceable: bool,
    placement_owner: &'a str,
    replace_owner: &'a str,
    survival_owner: &'a str,
    signal_owner: &'a str,
    direct_signal_owner: &'a str,
    bucket_pickup_owner: Option<&'a str>,
    can_place_liquid_owner: Option<&'a str>,
    place_liquid_owner: Option<&'a str>,
}
struct BlockInteraction {
    use_item_on: Arc<str>,
    use_without_item: Arc<str>,
    placement: Arc<str>,
    replace: Arc<str>,
    survival: Arc<str>,
    hand_open: Option<bool>,
    default_state: u32,
    replaceable: bool,
    signal: Arc<str>,
    direct_signal: Arc<str>,
    bucket_pickup: Option<Arc<str>>,
    can_place_liquid: Option<Arc<str>>,
    place_liquid: Option<Arc<str>>,
}
struct ItemInteraction {
    air: Arc<str>,
    on_block: Arc<str>,
    compost_chance: Option<f32>,
    block: Option<u32>,
    class: Arc<str>,
    description_id: Box<str>,
    name_owner: Arc<str>,
    can_fit_container: bool,
    is_banner: bool,
    is_bundle: bool,
    brewing_ingredient: bool,
    crafting_remainder: Option<Value>,
    spawn_type: Option<Arc<str>>,
}

#[derive(serde::Deserialize)]
struct ItemPropertiesReport<'a> {
    source_version: &'a str,
    #[serde(borrow)]
    items: BTreeMap<&'a str, ItemPropertiesSource<'a>>,
}
#[derive(serde::Deserialize)]
struct ItemPropertiesSource<'a> {
    class: &'a str,
    description_id: &'a str,
    name_owner: &'a str,
    can_fit_container: bool,
    is_banner: bool,
    is_bundle: bool,
    brewing_ingredient: bool,
    crafting_remainder: Option<Value>,
}
fn interaction_owner(pool: &mut BTreeSet<Arc<str>>, name: &str) -> Arc<str> {
    if let Some(owner) = pool.get(name) {
        return Arc::clone(owner);
    }
    let owner: Arc<str> = Arc::from(name);
    pool.insert(Arc::clone(&owner));
    owner
}
fn interaction_facts(
    data: &Path,
    names: &[String],
    items: &BTreeMap<u32, String>,
) -> Result<(Vec<BlockInteraction>, BTreeMap<u32, ItemInteraction>)> {
    let block_path = std::env::var("SOLARIS_CLIENT_BLOCK_USE_FACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data.join("reports/block_use_classes.json"));
    let block_bytes = bounded_bytes(&block_path, 1 << 20)?;
    let blocks: BlockUseReport<'_> = serde_json::from_slice(&block_bytes)
        .map_err(|error| format!("invalid block use facts: {error}"))?;
    let item_path = std::env::var("SOLARIS_CLIENT_ITEM_USE_FACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data.join("reports/item_use_owners.json"));
    let item_bytes = bounded_bytes(&item_path, 1 << 20)?;
    let item_use: BTreeMap<&str, ItemUseSource<'_>> = serde_json::from_slice(&item_bytes)
        .map_err(|error| format!("invalid item use facts: {error}"))?;
    let interaction_path = std::env::var("SOLARIS_CLIENT_INTERACTION_FACTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data.join("reports/interaction_facts.json"));
    let interaction_bytes = bounded_bytes(&interaction_path, 1 << 20)?;
    let interaction: InteractionReport<'_> = serde_json::from_slice(&interaction_bytes)
        .map_err(|error| format!("invalid interaction facts: {error}"))?;
    let properties_path = std::env::var("SOLARIS_CLIENT_ITEM_PROPERTIES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data.join("reports/item_properties.json"));
    let properties_bytes = bounded_bytes(&properties_path, 1 << 20)?;
    let mut item_properties: ItemPropertiesReport<'_> =
        serde_json::from_slice(&properties_bytes)
            .map_err(|error| format!("invalid item properties: {error}"))?;
    if item_properties.source_version != "26.1.2" {
        return Err("item properties version mismatch".into());
    }
    if blocks.version != "26.1.2" || interaction.version != "26.1.2" {
        return Err("interaction facts version mismatch".into());
    }
    let mut pool = BTreeSet::new();
    let mut block_facts = Vec::with_capacity(names.len());
    for name in names {
        let source = blocks
            .blocks
            .get(name.as_str())
            .ok_or_else(|| format!("verified block use owners missing {name}"))?;
        let placement = interaction
            .blocks
            .get(name.as_str())
            .ok_or_else(|| format!("verified placement facts missing {name}"))?;
        block_facts.push(BlockInteraction {
            use_item_on: interaction_owner(&mut pool, source.use_item_on_owner),
            use_without_item: interaction_owner(&mut pool, source.use_without_item_owner),
            placement: interaction_owner(&mut pool, placement.placement_owner),
            replace: interaction_owner(&mut pool, placement.replace_owner),
            survival: interaction_owner(&mut pool, placement.survival_owner),
            hand_open: placement.can_open_by_hand,
            default_state: placement.default_state,
            replaceable: placement.replaceable,
            signal: interaction_owner(&mut pool, placement.signal_owner),
            direct_signal: interaction_owner(&mut pool, placement.direct_signal_owner),
            bucket_pickup: placement
                .bucket_pickup_owner
                .map(|owner| interaction_owner(&mut pool, owner)),
            can_place_liquid: placement
                .can_place_liquid_owner
                .map(|owner| interaction_owner(&mut pool, owner)),
            place_liquid: placement
                .place_liquid_owner
                .map(|owner| interaction_owner(&mut pool, owner)),
        });
    }
    let mut item_facts = BTreeMap::new();
    for (&id, name) in items {
        let source = item_use
            .get(name.as_str())
            .ok_or_else(|| format!("verified item use owners missing {name}"))?;
        let properties = item_properties
            .items
            .remove(name.as_str())
            .ok_or_else(|| format!("verified item properties missing {name}"))?;
        item_facts.insert(
            id,
            ItemInteraction {
                air: interaction_owner(&mut pool, source.air),
                on_block: interaction_owner(&mut pool, source.on_block),
                compost_chance: interaction.compostable_items.get(name.as_str()).copied(),
                block: interaction.item_blocks.get(name.as_str()).copied(),
                class: interaction_owner(&mut pool, properties.class),
                description_id: properties.description_id.into(),
                name_owner: interaction_owner(&mut pool, properties.name_owner),
                can_fit_container: properties.can_fit_container,
                is_banner: properties.is_banner,
                is_bundle: properties.is_bundle,
                brewing_ingredient: properties.brewing_ingredient,
                crafting_remainder: properties.crafting_remainder,
                spawn_type: interaction
                    .item_spawn_types
                    .get(name.as_str())
                    .map(|owner| interaction_owner(&mut pool, owner)),
            },
        );
    }
    Ok((block_facts, item_facts))
}

#[derive(serde::Deserialize)]
struct StatePredicatesReport<'a> {
    game_version: &'a str,
    states: BTreeMap<usize, StatePredicates>,
}
#[derive(serde::Deserialize)]
struct StatePredicates {
    is_air: bool,
    is_signal_source: bool,
}
fn state_predicates(bytes: &[u8], count: usize) -> Result<BTreeMap<usize, StatePredicates>> {
    let report: StatePredicatesReport<'_> = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid block state predicates: {error}"))?;
    if report.game_version != "26.1.2"
        || report.states.len() != count
        || report.states.keys().any(|state| *state >= count)
    {
        return Err(
            "block state predicate source version or complete holder coverage mismatch".into(),
        );
    }
    Ok(report.states)
}

#[derive(serde::Deserialize)]
struct StaticClientReport<'a> {
    game_version: &'a str,
    #[serde(borrow)]
    fuel_rules: Vec<FuelSource<'a>>,
    #[serde(borrow)]
    potions: BTreeMap<&'a str, Value>,
    #[serde(borrow)]
    mob_effects: BTreeMap<&'a str, Value>,
}
#[derive(serde::Deserialize)]
struct FuelSource<'a> {
    remove: bool,
    kind: &'a str,
    name: &'a str,
}
enum FuelMember {
    Item(u32),
    Tag(Box<str>),
}
struct FuelRule {
    remove: bool,
    member: FuelMember,
}
fn static_client_facts(
    bytes: &[u8],
    items: &BTreeMap<u32, String>,
    registries: &mut BTreeMap<String, Vec<(String, Value)>>,
) -> Result<Vec<FuelRule>> {
    let report: StaticClientReport<'_> = serde_json::from_slice(bytes)
        .map_err(|error| format!("invalid static client source facts: {error}"))?;
    if report.game_version != "26.1.2" || report.fuel_rules.len() > 512 {
        return Err("fuel source version or rule bound".into());
    }
    for (registry, mut facts) in [
        ("minecraft:potion", report.potions),
        ("minecraft:mob_effect", report.mob_effects),
    ] {
        let entries = registries
            .get_mut(registry)
            .ok_or_else(|| format!("verified static registry unavailable {registry}"))?;
        if entries.len() != facts.len() {
            return Err(format!(
                "source static metadata holder coverage mismatch {registry}"
            ));
        }
        for (name, value) in entries {
            *value = facts
                .remove(name.as_str())
                .ok_or_else(|| format!("source static metadata missing {registry}/{name}"))?;
        }
    }
    report
        .fuel_rules
        .into_iter()
        .map(|rule| {
            split_id(rule.name)?;
            let member = match rule.kind {
                "item" => FuelMember::Item(
                    items
                        .iter()
                        .find_map(|(id, name)| (name == rule.name).then_some(*id))
                        .ok_or_else(|| format!("source fuel item unavailable {}", rule.name))?,
                ),
                "tag" => FuelMember::Tag(rule.name.into()),
                _ => return Err(format!("invalid source fuel member {}", rule.kind)),
            };
            Ok(FuelRule {
                remove: rule.remove,
                member,
            })
        })
        .collect()
}

#[derive(Clone)]
pub struct BlockInfo {
    pub block: usize,
    pub registry_id: u32,
    pub is_air: bool,
    pub is_signal_source: bool,
    pub model: Option<usize>,
    pub hardness: f32,
    pub requires_tool: bool,
    pub collision: Collision,
    pub occludes: [bool; 6],
    pub fluid: Option<FluidState>,
    pub solid: bool,
    pub shade_brightness: f32,
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    pub view_blocking: bool,
    pub light_dampening: u8,
    pub light_emission: u8,
    pub solid_render: bool,
    pub collision_full: bool,
    pub sturdy_faces: [bool; 6],
    pub is_ice_block: bool,
    pub can_occlude: bool,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Collision {
    Empty,
    Cube,
    Shape(usize),
    Unsupported,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FluidKind {
    Water,
    Lava,
}
#[derive(Clone, Copy, Debug)]
pub struct FluidState {
    pub kind: FluidKind,
    pub amount: u8,
    pub falling: bool,
    pub waterlogged: bool,
}
impl FluidState {
    pub fn height(self) -> f32 {
        f32::from(self.amount) / 9.
    }
}
#[derive(Clone, Copy, Debug)]
pub struct FluidTextures {
    pub still: TextureRegion,
    pub flow: TextureRegion,
    pub overlay: Option<TextureRegion>,
    pub layer: RenderLayer,
    pub tint: TintKind,
}
#[derive(Clone, Copy, Debug)]
pub struct TextureRegion {
    pub uv: [f32; 4],
    pub layer: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Clone, Copy, Debug)]
pub struct BiomeColors {
    pub grass: [f32; 3],
    pub foliage: [f32; 3],
    pub dry_foliage: [f32; 3],
    pub water: [f32; 3],
    pub sky: [f32; 3],
    pub fog: [f32; 3],
    pub water_fog: [f32; 3],
}
#[derive(Clone, Copy, Debug)]
pub struct ResourceLimits {
    pub max_source_bytes: u64,
    pub max_texture_dimension: u32,
    pub max_decoded_bytes: usize,
    pub max_atlas_bytes: usize,
    pub max_reload_bytes: usize,
    pub max_resources: usize,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_source_bytes: 32 << 20,
            max_texture_dimension: 4096,
            max_decoded_bytes: 256 << 20,
            max_atlas_bytes: 256 << 20,
            max_reload_bytes: 512 << 20,
            max_resources: 32768,
        }
    }
}
#[derive(Clone)]
struct IndexedResource {
    path: PathBuf,
    size: u64,
    sha1: String,
}
/// Validated source stack: original archive extraction first, highest-priority pack last.
/// No network, archive execution, or writes to original resources occur here.
#[derive(Clone)]
pub struct ResourceSource {
    roots: Vec<PathBuf>,
    indexed: BTreeMap<String, IndexedResource>,
    loader: BTreeMap<String, Arc<[u8]>>,
    pub limits: ResourceLimits,
}
impl ResourceSource {
    pub(crate) fn loader_geometry_budget(&self) -> Option<usize> {
        (!self.loader.is_empty()).then_some(65536)
    }
    pub fn new(
        roots: Vec<PathBuf>,
        verified_index: Option<(&Path, &Path)>,
        limits: ResourceLimits,
    ) -> Result<Self> {
        if roots.is_empty() || roots.len() > 32 {
            return Err("resource pack stack bound".into());
        }
        let roots = roots
            .into_iter()
            .map(|p| fs::canonicalize(&p).map_err(|e| format!("{}: {e}", p.display())))
            .collect::<Result<Vec<_>>>()?;
        for root in roots.iter().skip(1) {
            validate_pack(root)?;
        }
        let mut indexed = BTreeMap::new();
        if let Some((index, objects)) = verified_index {
            let entries = json_file(index, 16 << 20)?;
            for (name, entry) in entries["objects"]
                .as_object()
                .ok_or("verified official asset index objects")?
            {
                safe_relative(name)?;
                let hash = entry["hash"]
                    .as_str()
                    .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
                    .ok_or("asset object digest")?;
                let size = entry["size"].as_u64().ok_or("asset object size")?;
                indexed.insert(
                    format!("assets/{name}"),
                    IndexedResource {
                        path: objects.join("objects").join(&hash[..2]).join(hash),
                        size,
                        sha1: hash.to_owned(),
                    },
                );
            }
            if indexed.len() > limits.max_resources {
                return Err("indexed asset count bound".into());
            }
        }
        Ok(Self {
            roots,
            indexed,
            loader: BTreeMap::new(),
            limits,
        })
    }
    pub fn resource_root(&self) -> &Path {
        &self.roots[0]
    }
    pub fn bytes(&self, relative: &str) -> Result<Vec<u8>> {
        safe_relative(relative)?;
        if let Some(bytes) = self.loader.get(relative) {
            return Ok(bytes.to_vec());
        }
        for root in self.roots.iter().rev() {
            if let Some(bytes) = root_resource_bytes(root, relative, self.limits.max_source_bytes)?
            {
                return Ok(bytes);
            }
        }
        self.indexed_bytes(relative)?
            .ok_or_else(|| format!("missing mandatory resource {relative}"))
    }
    fn indexed_bytes(&self, relative: &str) -> Result<Option<Vec<u8>>> {
        let Some(indexed) = self.indexed.get(relative) else {
            return Ok(None);
        };
        if indexed.size > self.limits.max_source_bytes {
            return Err(format!("indexed resource byte bound {relative}"));
        }
        let bytes = bounded_bytes(&indexed.path, self.limits.max_source_bytes)?;
        if bytes.len() as u64 != indexed.size
            || format!("{:x}", Sha1::digest(&bytes)) != indexed.sha1.to_ascii_lowercase()
        {
            return Err(format!("verified asset integrity mismatch {relative}"));
        }
        Ok(Some(bytes))
    }
    fn stack(&self, relative: &str) -> Result<Vec<Arc<[u8]>>> {
        safe_relative(relative)?;
        let mut stack = Vec::new();
        let base = match self.roots.first() {
            Some(root) => root_resource_bytes(root, relative, self.limits.max_source_bytes)?,
            None => None,
        };
        if let Some(bytes) = match base {
            Some(bytes) => Some(bytes),
            None => self.indexed_bytes(relative)?,
        } {
            stack.push(Arc::from(bytes));
        }
        for root in self.roots.iter().skip(1) {
            if let Some(bytes) = root_resource_bytes(root, relative, self.limits.max_source_bytes)?
            {
                stack.push(Arc::from(bytes));
            }
        }
        if let Some(bytes) = self.loader.get(relative) {
            stack.push(Arc::clone(bytes));
        }
        if stack.is_empty() {
            return Err(format!("missing mandatory resource {relative}"));
        }
        Ok(stack)
    }
    pub(crate) fn texture_metadata(&self, name: &str) -> Result<Value> {
        let (ns, path) = split_id(name)?;
        let relative = format!("assets/{ns}/textures/{path}.png.mcmeta");
        match self.bytes(&relative) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("{relative}: {e}")),
            Err(e) if e.starts_with("missing mandatory resource") => Ok(Value::Null),
            Err(e) => Err(e),
        }
    }
    pub fn json(&self, kind: &str, id: &str) -> Result<Value> {
        let (namespace, path) = split_id(id)?;
        let relative = format!("assets/{namespace}/{kind}/{path}.json");
        let bytes = self.bytes(&relative)?;
        if bytes.len() > 1 << 20 {
            return Err(format!("model/definition JSON bound {relative}"));
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("{relative}: {e}"))
    }
    fn resource_names(&self, kind: &str, extension: &str) -> Result<BTreeSet<String>> {
        let mut out = BTreeSet::new();
        for root in &self.roots {
            let assets = root.join("assets");
            for ns in fs::read_dir(&assets).map_err(|e| format!("{}: {e}", assets.display()))? {
                let ns = ns.map_err(|e| e.to_string())?;
                if !ns.file_type().map_err(|e| e.to_string())?.is_dir() {
                    continue;
                }
                let name = ns
                    .file_name()
                    .into_string()
                    .map_err(|_| "invalid namespace")?;
                split_id(&format!("{name}:x"))?;
                let resources = ns.path().join(kind);
                if resources.exists() {
                    collect_resources(
                        &resources,
                        &resources,
                        &name,
                        &mut out,
                        0,
                        self.limits.max_resources,
                        extension,
                    )?;
                }
            }
        }
        for name in self.indexed.keys().chain(self.loader.keys()) {
            if let Some(rest) = name.strip_prefix("assets/") {
                if let Some((ns, path)) = rest.split_once('/') {
                    if let Some(path) = path
                        .strip_prefix(kind)
                        .and_then(|path| path.strip_prefix('/'))
                        .and_then(|path| path.strip_suffix(extension))
                        .and_then(|path| path.strip_suffix('.'))
                    {
                        out.insert(normalize_id(&format!("{ns}:{path}"))?);
                        if out.len() > self.limits.max_resources {
                            return Err("resource count bound".into());
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}
fn root_resource_bytes(root: &Path, relative: &str, max: u64) -> Result<Option<Vec<u8>>> {
    let path = root.join(relative);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{relative}: {error}")),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(format!("invalid resource file {relative}"));
    }
    let canonical = fs::canonicalize(&path).map_err(|error| error.to_string())?;
    if !canonical.starts_with(root) {
        return Err(format!("resource escapes source root {relative}"));
    }
    bounded_bytes(&canonical, max).map(Some)
}
fn collect_resources(
    dir: &Path,
    base: &Path,
    ns: &str,
    out: &mut BTreeSet<String>,
    depth: usize,
    max: usize,
    extension: &str,
) -> Result<()> {
    if depth > 16 {
        return Err("resource directory depth".into());
    }
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_symlink() {
            return Err(format!("resource symlink {}", entry.path().display()));
        }
        if ty.is_dir() {
            collect_resources(&entry.path(), base, ns, out, depth + 1, max, extension)?;
        } else if entry.path().extension().is_some_and(|v| v == extension) {
            let path = entry.path();
            let path = path
                .strip_prefix(base)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("resource path encoding")?
                .rsplit_once('.')
                .ok_or("resource extension missing")?
                .0;
            out.insert(normalize_id(&format!("{ns}:{path}"))?);
            if out.len() > max {
                return Err("resource count bound".into());
            }
        }
    }
    Ok(())
}
fn safe_relative(path: &str) -> Result<()> {
    if path.is_empty()
        || path.len() > 512
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
        || path
            .bytes()
            .any(|b| !(b.is_ascii_lowercase() || b.is_ascii_digit() || b"/_.-".contains(&b)))
    {
        return Err(format!("invalid resource path {path}"));
    }
    Ok(())
}
fn split_id(id: &str) -> Result<(&str, &str)> {
    let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
    if ns.is_empty()
        || ns
            .bytes()
            .any(|b| !(b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b)))
    {
        return Err(format!("invalid resource namespace {id}"));
    }
    safe_relative(path)?;
    Ok((ns, path))
}
pub(crate) fn normalize_id(id: &str) -> Result<String> {
    let (ns, path) = split_id(id)?;
    Ok(format!("{ns}:{path}"))
}
pub(crate) fn bounded_bytes(path: &Path, max: u64) -> Result<Vec<u8>> {
    let metadata = fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if metadata.len() > max {
        return Err(format!("{} exceeds resource byte bound", path.display()));
    }
    let bytes = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > max {
        return Err("resource changed beyond byte bound".into());
    }
    Ok(bytes)
}
pub fn json_file(path: &Path, max: u64) -> Result<Value> {
    serde_json::from_slice(&bounded_bytes(path, max)?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub struct Content {
    pub names: Vec<String>,
    pub states: Vec<BlockInfo>,
    pub models: Vec<BakedModel>,
    pub item_names: BTreeMap<u32, String>,
    pub sound_names: BTreeMap<u32, String>,
    pub components: BTreeMap<i32, String>,
    pub attribute_names: BTreeMap<u32, String>,
    pub entity_names: BTreeMap<u32, String>,
    pub block_entity_names: BTreeMap<u32, String>,
    pub particle_names: BTreeMap<u32, String>,
    pub rgba: Arc<Vec<u8>>,
    pub mips: Arc<Vec<AtlasMip>>,
    pub width: u32,
    pub height: u32,
    pub texture_layers: u32,
    pub presentation_unsupported: usize,
    source: ResourceSource,
    textures: BTreeMap<String, TextureRegion>,
    texture_materials: BTreeMap<String, RenderLayer>,
    grass_colormap: Vec<[u8; 3]>,
    foliage_colormap: Vec<[u8; 3]>,
    dry_foliage_colormap: Vec<[u8; 3]>,
    swamp_noise: Simplex2,
    shapes: Vec<Vec<[f64; 6]>>,
    terrain: Vec<TerrainFact>,
    properties: Vec<BTreeMap<String, String>>,
    property_domains: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    required_features: FeatureRequirements,
    animations: Vec<Animation>,
    item_defaults: BTreeMap<u32, Arc<BTreeMap<String, Value>>>,
    block_interactions: Vec<BlockInteraction>,
    item_interactions: BTreeMap<u32, ItemInteraction>,
    placement: placement::PlacementFacts,
    block_states: Vec<Vec<u32>>,
    #[cfg(test)]
    pub(crate) creative_tabs_report: Arc<[u8]>,
    #[cfg(not(test))]
    creative_tabs_report: Arc<[u8]>,
    font_ids: Vec<String>,
    fuel_rules: Vec<FuelRule>,
    fuel_items: Vec<u32>,
    fuel_members: BTreeSet<u32>,
    enabled_features: Arc<BTreeSet<String>>,
    static_registries: BTreeMap<String, Vec<(String, Value)>>,
    registries: BTreeMap<String, Vec<(String, Value)>>,
    tags: Arc<BTreeMap<String, BTreeMap<String, RegistryTag>>>,
    data_root: PathBuf,
    dynamic: Mutex<DynamicAtlas>,
    loader_base: Option<loader_resources::BaseResources>,
    loader_items: BTreeMap<String, crate::loader::items::ItemDefinition>,
    loader_blocks: Vec<Value>,
    loader_carriers: BTreeMap<u32, loader_blocks::CarrierBlock>,
    loader_model_base: Option<usize>,
}
fn registry(v: &Value, name: &str) -> BTreeMap<u32, String> {
    v[format!("minecraft:{name}")]["entries"]
        .as_object()
        .map(|o| {
            o.iter()
                .filter_map(|(n, e)| e["protocol_id"].as_u64().map(|i| (i as u32, n.clone())))
                .collect()
        })
        .unwrap_or_default()
}
fn static_registries(report: &Value) -> Result<BTreeMap<String, Vec<(String, Value)>>> {
    let mut out = BTreeMap::new();
    for (registry, definition) in report.as_object().ok_or("invalid static registry report")? {
        let entries = definition["entries"]
            .as_object()
            .ok_or("static registry entries")?;
        let mut ordered = vec![None; entries.len()];
        for (name, entry) in entries {
            split_id(name)?;
            let id = entry["protocol_id"]
                .as_u64()
                .filter(|id| *id < ordered.len() as u64)
                .ok_or("static registry protocol id bound")? as usize;
            if ordered[id].replace((name.clone(), Value::Null)).is_some() {
                return Err(format!("duplicate static registry id {registry}/{id}"));
            }
        }
        out.insert(
            registry.clone(),
            ordered
                .into_iter()
                .collect::<Option<Vec<_>>>()
                .ok_or("static registry id hole")?,
        );
    }
    Ok(out)
}
fn validate_pack(root: &Path) -> Result<()> {
    if !root.is_dir() {
        return Err("resource pack must be an extracted directory".into());
    }
    let metadata = json_file(&root.join("pack.mcmeta"), 1 << 20)?;
    let format = |v: &Value| -> Result<(u64, u64)> {
        if let Some(major) = v.as_u64() {
            return Ok((major, 0));
        }
        let parts = v
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or("resource pack format must be integer or [major,minor]")?;
        Ok((
            parts[0].as_u64().ok_or("pack major format")?,
            parts[1].as_u64().ok_or("pack minor format")?,
        ))
    };
    let pack = &metadata["pack"];
    let fallback = &pack["pack_format"];
    let min = if pack["min_format"].is_null() {
        format(fallback)?
    } else {
        format(&pack["min_format"])?
    };
    let max = if pack["max_format"].is_null() {
        format(fallback)?
    } else {
        format(&pack["max_format"])?
    };
    if min > max || min > (84, 0) || max < (84, 0) {
        return Err(format!(
            "resource pack format incompatible with pinned26.1.2: {min:?}..{max:?}"
        ));
    }
    if !metadata["filter"].is_null() || !metadata["overlays"].is_null() {
        return Err("resource pack filters/overlays not supported by native resource stack".into());
    }
    let assets = root.join("assets");
    if assets.exists() {
        for namespace in fs::read_dir(assets).map_err(|e| e.to_string())? {
            if namespace
                .map_err(|e| e.to_string())?
                .path()
                .join("shaders")
                .exists()
            {
                return Err(
                    "resource pack shader overrides cannot execute in native renderer".into(),
                );
            }
        }
    }
    Ok(())
}
impl Content {
    pub fn load() -> Result<Self> {
        Self::load_mode(true)
    }
    pub(crate) fn load_state() -> Result<Self> {
        let data = PathBuf::from(
            std::env::var("SOLARIS_CLIENT_DATA_DIR")
                .map_err(|_| "SOLARIS_CLIENT_DATA_DIR required for pinned local data")?,
        );
        Self::load_from_mode(
            ResourceSource {
                roots: Vec::new(),
                indexed: BTreeMap::new(),
                loader: BTreeMap::new(),
                limits: ResourceLimits::default(),
            },
            &data,
            false,
        )
    }
    fn load_mode(presentation: bool) -> Result<Self> {
        let root =
            PathBuf::from(std::env::var("SOLARIS_CLIENT_RESOURCE_ROOT").map_err(
                |_| "SOLARIS_CLIENT_RESOURCE_ROOT required for verified local resources",
            )?);
        let data = PathBuf::from(
            std::env::var("SOLARIS_CLIENT_DATA_DIR")
                .map_err(|_| "SOLARIS_CLIENT_DATA_DIR required for pinned local data")?,
        );
        let index = std::env::var("SOLARIS_CLIENT_ASSET_INDEX")
            .ok()
            .map(PathBuf::from);
        let assets = std::env::var("SOLARIS_CLIENT_ASSET_ROOT")
            .ok()
            .map(PathBuf::from);
        if index.is_some() != assets.is_some() {
            return Err("verified asset index/root must be supplied together".into());
        }
        let mut roots = vec![root];
        if let Some(packs) = std::env::var_os("SOLARIS_CLIENT_RESOURCE_PACKS") {
            roots.extend(std::env::split_paths(&packs));
        }
        let source = ResourceSource::new(
            roots,
            index.as_deref().zip(assets.as_deref()),
            ResourceLimits::default(),
        )?;
        Self::load_from_mode(source, &data, presentation)
    }
    pub fn stage_current_reload(&self) -> Result<Self> {
        self.stage_reload(self.source.clone(), &self.data_root)
    }
    /// Stages a coherent CPU generation. Caller reserves GPU bytes and atomically publishes it.
    pub fn stage_reload(&self, mut source: ResourceSource, data: &Path) -> Result<Self> {
        for root in source.roots.iter().skip(1) {
            validate_pack(root)?;
        }
        let available = source
            .limits
            .max_reload_bytes
            .checked_sub(self.resource_bytes_used())
            .ok_or("active resources exceed reload budget")?;
        source.limits.max_decoded_bytes = source.limits.max_decoded_bytes.min(available);
        source.limits.max_atlas_bytes = source.limits.max_atlas_bytes.min(available);
        let loader_paths = self.source.loader.clone();
        source.loader.clear();
        let mut staging = Self::load_from(source, data)?;
        staging.registries = self.registries.clone();
        staging.tags = Arc::clone(&self.tags);
        staging.enabled_features = Arc::clone(&self.enabled_features);
        staging.rebuild_fuels()?;
        if !loader_paths.is_empty() || !self.loader_items.is_empty() {
            let entries = self.loader_items.values().map(|item| {
                serde_json::json!({"id": item.id, "base_item": item.base_item, "name": item.name})
            }).collect::<Vec<_>>();
            staging.activate_loader_resources(&loader_paths, &entries, &self.loader_blocks)?;
        }
        if self
            .resource_bytes_used()
            .checked_add(staging.resource_bytes_used())
            .is_none_or(|v| v > staging.source.limits.max_reload_bytes)
        {
            return Err("resource reload active+staging memory budget exceeded".into());
        }
        Ok(staging)
    }
    pub fn load_from(source: ResourceSource, data: &Path) -> Result<Self> {
        Self::load_from_mode(source, data, true)
    }
    fn load_from_mode(source: ResourceSource, data: &Path, presentation: bool) -> Result<Self> {
        let blocks = json_file(&data.join("reports/blocks.json"), 64 << 20)?;
        let mining = json_file(&data.join("reports/block_mining.json"), 16 << 20)?;
        let light = json_file(&data.join("reports/block_light.json"), 16 << 20)?;
        if mining["version"] != "26.1.2" || light["version"] != "26.1.2" {
            return Err("block facts version mismatch".into());
        }
        let registries = json_file(&data.join("reports/registries.json"), 32 << 20)?;
        let max = mining["max_state_id"]
            .as_u64()
            .filter(|v| *v < 1000000)
            .ok_or("block state count bound")? as usize;
        let collision_path = std::env::var("SOLARIS_CLIENT_COLLISION_SHAPES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data.join("reports/block_collision_shapes.bin"));
        let mut collision = ShapeTable::load(&collision_path)?;
        if collision.state_ids.len() != max + 1 {
            return Err("collision report state count mismatch".into());
        }
        let terrain_path = std::env::var("SOLARIS_CLIENT_TERRAIN_FACTS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data.join("reports/terrain_facts.json"));
        let (terrain, property_domains, required_features) =
            TerrainFact::load(&terrain_path, &collision.fingerprints, presentation)?;
        for name in registries["minecraft:item"]["entries"]
            .as_object()
            .ok_or("pinned item registry entries")?
            .keys()
        {
            if !required_features.items.contains_key(name) {
                return Err(format!("verified feature requirements missing item {name}"));
            }
        }
        collision.normalize_offsets(&terrain);
        let predicate_path = std::env::var("SOLARIS_CLIENT_BLOCK_STATE_PREDICATES")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data.join("reports/block_state_predicates.json"));
        let mut predicates = state_predicates(&bounded_bytes(&predicate_path, 8 << 20)?, max + 1)?;
        let fallback = BlockInfo {
            block: 0,
            registry_id: 0,
            is_air: false,
            is_signal_source: false,
            model: None,
            hardness: -1.,
            requires_tool: true,
            collision: Collision::Unsupported,
            occludes: [false; 6],
            fluid: None,
            solid: false,
            shade_brightness: 1.,
            friction: 0.6,
            speed_factor: 1.,
            jump_factor: 1.,
            view_blocking: false,
            light_dampening: 0,
            light_emission: 0,
            solid_render: false,
            collision_full: false,
            sturdy_faces: [false; 6],
            is_ice_block: false,
            can_occlude: false,
        };
        let mut states = vec![fallback; max + 1];
        let mut properties = vec![BTreeMap::new(); max + 1];
        let mut names = Vec::new();
        let mut baker = presentation.then(|| Baker::new(&source));
        let block_registry = registry(&registries, "block")
            .into_iter()
            .map(|(id, name)| (name, id))
            .collect::<BTreeMap<_, _>>();
        let mut texture_names = if presentation {
            source.resource_names("textures", "png")?
        } else {
            BTreeSet::new()
        };
        for (name, block) in blocks.as_object().ok_or("invalid block report")? {
            let block_id = names.len();
            names.push(name.clone());
            if !property_domains.contains_key(name) {
                return Err(format!("verified property domains missing block {name}"));
            }
            if !required_features.blocks.contains_key(name) {
                return Err(format!(
                    "verified feature requirements missing block {name}"
                ));
            }
            let definition = if presentation {
                Some(source.json("blockstates", name)?)
            } else {
                None
            };
            for state in block["states"].as_array().ok_or("missing block states")? {
                let id = state["id"]
                    .as_u64()
                    .filter(|v| *v <= max as u64)
                    .ok_or("state id beyond facts report")? as usize;
                let p = state["properties"]
                    .as_object()
                    .map(|p| {
                        p.iter()
                            .map(|(k, v)| {
                                v.as_str()
                                    .map(|v| (k.clone(), v.to_owned()))
                                    .ok_or("state property must be string".to_owned())
                            })
                            .collect::<Result<BTreeMap<_, _>>>()
                    })
                    .transpose()?
                    .unwrap_or_default();
                collision.verify(id, name, &p)?;
                let shape = collision.state_ids[id];
                let boxes = &collision.shapes[shape];
                let class = if boxes.is_empty() {
                    Collision::Empty
                } else if boxes.as_slice() == [[0., 0., 0., 1., 1., 1.]] {
                    Collision::Cube
                } else {
                    Collision::Shape(shape)
                };
                let model = baker
                    .as_mut()
                    .map(|baker| {
                        baker
                            .state(name, &state["properties"], definition.as_ref().unwrap())
                            .map_err(|e| format!("{name} state {id}: {e}"))
                    })
                    .transpose()?;
                let fluid = terrain[id].fluid(name)?;
                properties[id] = p;
                let predicates = predicates
                    .remove(&id)
                    .ok_or("duplicate block state or missing verified predicate")?;
                states[id] = BlockInfo {
                    block: block_id,
                    registry_id: *block_registry
                        .get(name)
                        .ok_or("block missing from pinned protocol registry")?,
                    is_air: predicates.is_air,
                    is_signal_source: predicates.is_signal_source,
                    model,
                    hardness: mining["entries"][id][0]
                        .as_f64()
                        .ok_or("missing hardness")? as f32,
                    requires_tool: mining["entries"][id][1].as_i64() == Some(1),
                    collision: class,
                    occludes: std::array::from_fn(|face| {
                        rectangles_cover(&terrain[id].faces[face], &[[0., 0., 1., 1.]])
                    }),
                    fluid,
                    solid: terrain[id].solid,
                    shade_brightness: terrain[id].shade_brightness,
                    friction: terrain[id].friction,
                    speed_factor: terrain[id].speed_factor,
                    jump_factor: terrain[id].jump_factor,
                    view_blocking: terrain[id].view_blocking,
                    light_dampening: terrain[id].light_dampening,
                    light_emission: light["entries"][id][0]
                        .as_u64()
                        .filter(|value| *value <= 15)
                        .ok_or("invalid Source light emission")?
                        as u8,
                    solid_render: terrain[id].solid_render,
                    collision_full: terrain[id].collision_full,
                    sturdy_faces: terrain[id].sturdy_faces,
                    is_ice_block: terrain[id].is_ice_block,
                    can_occlude: terrain[id].can_occlude,
                };
            }
        }
        // These source trees are only needed to construct the state tables.
        // Release them before loading the independent placement/action tables.
        drop(blocks);
        drop(mining);
        drop(light);
        if !predicates.is_empty() {
            return Err("block report missing source predicate holders".into());
        }
        let (models, atlas) = if let Some(baker) = baker {
            let raw_models = baker.into_models();
            for model in &raw_models {
                model.textures(&mut texture_names);
            }
            let atlas = Atlas::build(&source, &texture_names)?;
            let models = raw_models
                .into_iter()
                .map(|m| m.finish(&atlas.textures, &atlas.materials))
                .collect::<Result<Vec<_>>>()?;
            (models, atlas)
        } else {
            (Vec::new(), Atlas::default())
        };
        let item_names = registry(&registries, "item");
        let mut item_defaults = BTreeMap::new();
        for (&id, name) in &item_names {
            let (namespace, path) = split_id(name)?;
            let items_path = data
                .join("reports")
                .join(namespace)
                .join("components/item")
                .join(format!("{path}.json"));
            let bytes = bounded_bytes(&items_path, 1 << 20)?;
            let report: ItemDefaultsReport = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid item default components {name}: {error}"))?;
            item_defaults.insert(id, Arc::new(report.components));
        }
        let (block_interactions, item_interactions) = interaction_facts(data, &names, &item_names)?;
        let mut block_states = Vec::with_capacity(names.len());
        for name in &names {
            let domains = &property_domains[name];
            let count = domains.values().try_fold(1usize, |count, domain| {
                count
                    .checked_mul(domain.len())
                    .filter(|size| *size <= states.len())
                    .ok_or_else(|| format!("block property product outside state bound {name}"))
            })?;
            block_states.push(vec![u32::MAX; count]);
        }
        for (state, info) in states.iter().enumerate() {
            let domains = &property_domains[&names[info.block]];
            let key = property_key(domains, |name| {
                properties[state].get(name).map(String::as_str)
            })?;
            let cell = &mut block_states[info.block][key];
            if *cell != u32::MAX {
                return Err(format!(
                    "duplicate block state properties {}",
                    names[info.block]
                ));
            }
            *cell = state as u32;
        }
        for (block, table) in block_states.iter().enumerate() {
            let default = block_interactions[block].default_state;
            if table.contains(&u32::MAX)
                || states
                    .get(default as usize)
                    .is_none_or(|state| state.block != block)
            {
                return Err(format!("incomplete block state table {}", names[block]));
            }
        }
        for facts in item_interactions.values() {
            if facts
                .block
                .is_some_and(|state| state as usize >= states.len())
            {
                return Err("block-item default state outside received source table".into());
            }
        }
        let creative_path = std::env::var("SOLARIS_CLIENT_CREATIVE_TABS")
            .map(PathBuf::from)
            .unwrap_or_else(|_| data.join("reports/creative_tabs.json"));
        let creative_tabs_report: Arc<[u8]> = Arc::from(bounded_bytes(&creative_path, 16 << 20)?);
        let mut static_registries = static_registries(&registries)?;
        let fuel_rules =
            static_client_facts(&creative_tabs_report, &item_names, &mut static_registries)?;
        let font_ids = if presentation {
            source.resource_names("font", "json")?.into_iter().collect()
        } else {
            Vec::new()
        };
        let placement = placement::PlacementFacts::load(data, &states, &item_names)?;
        Ok(Self {
            names,
            states,
            models,
            item_names,
            sound_names: registry(&registries, "sound_event"),
            components: registry(&registries, "data_component_type")
                .into_iter()
                .map(|(i, n)| (i as i32, n))
                .collect(),
            attribute_names: registry(&registries, "attribute"),
            entity_names: registry(&registries, "entity_type"),
            block_entity_names: registry(&registries, "block_entity_type"),
            particle_names: registry(&registries, "particle_type"),
            grass_colormap: if presentation {
                colormap(&source, "grass")?
            } else {
                Vec::new()
            },
            foliage_colormap: if presentation {
                colormap(&source, "foliage")?
            } else {
                Vec::new()
            },
            dry_foliage_colormap: if presentation {
                colormap(&source, "dry_foliage")?
            } else {
                Vec::new()
            },
            swamp_noise: Simplex2::new(2345),
            shapes: collision.shapes,
            terrain,
            data_root: data.to_path_buf(),
            properties,
            property_domains,
            required_features,
            rgba: Arc::new(atlas.rgba),
            mips: Arc::new(atlas.mips),
            dynamic: Mutex::new(DynamicAtlas {
                first_layer: atlas.dynamic_first_layer,
                entries: BTreeMap::new(),
                sequence: 0,
            }),
            width: atlas.side,
            height: atlas.side,
            texture_layers: atlas.layers,
            texture_materials: atlas.materials,
            textures: atlas.textures,
            animations: atlas.animations,
            source,
            presentation_unsupported: 0,
            item_defaults,
            block_interactions,
            item_interactions,
            placement,
            block_states,
            creative_tabs_report,
            font_ids,
            fuel_rules,
            fuel_items: Vec::new(),
            fuel_members: BTreeSet::new(),
            enabled_features: Arc::new(BTreeSet::new()),
            static_registries,
            registries: BTreeMap::new(),
            tags: Arc::new(BTreeMap::new()),
            loader_base: None,
            loader_items: BTreeMap::new(),
            loader_blocks: Vec::new(),
            loader_carriers: BTreeMap::new(),
            loader_model_base: None,
        })
    }
    pub fn model_for(&self, state: u32) -> Result<Option<&BakedModel>> {
        Ok(self.info(state)?.model.map(|id| &self.models[id]))
    }
    pub fn block_is_air(&self, state: u32) -> Result<bool> {
        Ok(self.info(state)?.is_air)
    }
    pub fn block_is_signal_source(&self, state: u32) -> Result<bool> {
        Ok(self.info(state)?.is_signal_source)
    }
    pub fn selection_boxes(&self, state: u32) -> Option<&[[f64; 6]]> {
        self.terrain
            .get(state as usize)
            .map(|t| t.selection.as_slice())
    }
    pub fn occlusion_boxes(&self, state: u32) -> Option<&[[f64; 6]]> {
        self.terrain
            .get(state as usize)
            .map(|t| t.occlusion.as_slice())
    }
    pub fn inside_collision(&self, state: u32) -> Result<Option<&[[f64; 6]]>> {
        self.terrain
            .get(state as usize)
            .map(|fact| fact.inside.as_ref().map(|boxes| boxes.as_slice()))
            .ok_or_else(|| format!("unknown source inside-collision state {state}"))
    }
    pub fn block_redstone_conductor(&self, state: u32) -> Result<bool> {
        self.terrain
            .get(state as usize)
            .map(|fact| fact.redstone_conductor)
            .ok_or_else(|| format!("unknown source redstone-conductor state {state}"))
    }
    pub fn fluid_solid_face(&self, state: u32, kind: FluidKind, direction: u8) -> Result<bool> {
        if direction >= 6 {
            return Err("fluid solid face direction bound".into());
        }
        let info = self.info(state)?;
        if info.fluid.is_some_and(|fluid| fluid.kind == kind) {
            return Ok(false);
        }
        if direction == 1 {
            return Ok(true);
        }
        Ok(!info.is_ice_block && info.sturdy_faces[usize::from(direction)])
    }
    pub fn block_offset(&self, state: u32, position: [i32; 3]) -> [f64; 3] {
        self.terrain
            .get(state as usize)
            .map_or([0.; 3], |t| t.offset.at(position))
    }
    pub fn render_offset(&self, state: u32, position: [i32; 3]) -> [f32; 3] {
        self.block_offset(state, position).map(|v| v as f32)
    }
    pub fn face_visible(&self, state: u32, neighbor: u32, face: u8) -> Result<bool> {
        if face > 5 {
            return Err("face direction bound".into());
        }
        let current = self
            .terrain
            .get(state as usize)
            .ok_or("unknown current face shape")?;
        let neighboring = self
            .terrain
            .get(neighbor as usize)
            .ok_or("unknown neighboring face shape")?;
        let target = &neighboring.faces[(face ^ 1) as usize];
        if rectangles_cover(target, &[[0., 0., 1., 1.]]) {
            return Ok(false);
        }
        if current.skip[face as usize].binary_search(&neighbor).is_ok() {
            return Ok(false);
        }
        Ok(target.is_empty()
            || current.faces[face as usize].is_empty()
            || !rectangles_cover(target, &current.faces[face as usize]))
    }
    pub fn resource_root(&self) -> &Path {
        self.source.resource_root()
    }
    pub fn resource_bytes(&self, path: &str) -> Result<Vec<u8>> {
        self.source.bytes(path)
    }
    pub fn resource_stack(&self, path: &str) -> Result<Vec<Arc<[u8]>>> {
        self.source.stack(path)
    }
    pub fn texture(&self, name: &str) -> Option<TextureRegion> {
        if !name.contains(':') {
            return self.texture(&format!("minecraft:{name}"));
        }
        self.textures
            .get(name)
            .copied()
            .or_else(|| self.dynamic.lock().ok()?.entries.get(name).copied())
    }
    pub fn install_dynamic_texture(
        &self,
        id: &str,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> Result<(TextureRegion, AtlasPatch)> {
        let id = normalize_id(id)?;
        if !id.starts_with("profile:") {
            return Err("dynamic texture must belong to verified profile namespace".into());
        }
        if width == 0
            || height == 0
            || width > 96
            || height > 96
            || rgba.len() != (width * height * 4) as usize
        {
            return Err("dynamic profile sprite bound96x96/RGBA mismatch".into());
        }
        let mut atlas = self
            .dynamic
            .lock()
            .map_err(|_| "dynamic texture registry poisoned")?;
        let region = if let Some(region) = atlas.entries.get(&id).copied() {
            if region.width != width || region.height != height {
                return Err("dynamic replacement dimensions must preserve reserved handle".into());
            }
            region
        } else {
            let slot = atlas.entries.len() as u32;
            if slot >= 256 {
                return Err("dynamic profile texture slots exhausted".into());
            }
            let columns = self.width / 128;
            let slots_per_layer = columns * columns;
            let x = (slot % slots_per_layer % columns) * 128;
            let y = (slot % slots_per_layer / columns) * 128;
            let region = TextureRegion {
                uv: [
                    (x + 16) as f32 / self.width as f32,
                    (y + 16) as f32 / self.height as f32,
                    (x + 16 + width) as f32 / self.width as f32,
                    (y + 16 + height) as f32 / self.height as f32,
                ],
                layer: atlas.first_layer + slot / slots_per_layer,
                width,
                height,
            };
            atlas.entries.insert(id, region);
            region
        };
        let origin = [
            (region.uv[0] * self.width as f32) as u32 - 16,
            (region.uv[1] * self.height as f32) as u32 - 16,
        ];
        let mut pixels = vec![0; 128 * 128 * 4];
        for y in 0..128u32 {
            for x in 0..128u32 {
                let src = ((y.saturating_sub(16).min(height - 1) * width
                    + x.saturating_sub(16).min(width - 1))
                    * 4) as usize;
                let dst = ((y * 128 + x) * 4) as usize;
                pixels[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
            }
        }
        atlas.sequence = atlas
            .sequence
            .checked_add(1)
            .ok_or("dynamic texture revision overflow")?;
        Ok((
            region,
            AtlasPatch {
                mip_level: 0,
                origin,
                size: [128, 128],
                layer: region.layer,
                rgba: Arc::new(pixels),
                sequence: atlas.sequence,
            },
        ))
    }
    pub fn bake_model(&self, id: &str) -> Result<BakedModel> {
        Baker::new(&self.source)
            .model(id)?
            .finish(&self.textures, &self.texture_materials)
    }
    pub fn block_required_features(&self, state: u32) -> Result<&[String]> {
        let name = &self.names[self.info(state)?.block];
        self.required_features
            .blocks
            .get(name)
            .map(|features| features.as_ref())
            .ok_or_else(|| format!("verified required block features unavailable {name}"))
    }
    pub fn item_required_features(&self, id: u32) -> Result<&[String]> {
        let name = self
            .item_names
            .get(&id)
            .ok_or_else(|| format!("unknown feature-gated item {id}"))?;
        self.required_features
            .items
            .get(name)
            .map(|features| features.as_ref())
            .ok_or_else(|| format!("verified required item features unavailable {name}"))
    }
    pub fn state_properties(&self, state: u32) -> Option<&BTreeMap<String, String>> {
        self.properties.get(state as usize)
    }
    /// Uses actual Property<T> parsing domains and Java Comparable declaration ordering.
    pub fn compare_property(
        &self,
        state: u32,
        key: &str,
        left: &str,
        right: &str,
    ) -> Result<Option<std::cmp::Ordering>> {
        let block = &self.names[self.info(state)?.block];
        let Some(values) = self
            .property_domains
            .get(block)
            .and_then(|properties| properties.get(key))
        else {
            return Ok(None);
        };
        Ok(values
            .iter()
            .position(|value| value == left)
            .zip(values.iter().position(|value| value == right))
            .map(|(left, right)| left.cmp(&right)))
    }
    pub fn item_components(&self, id: u32) -> Result<&Arc<BTreeMap<String, Value>>> {
        self.item_defaults.get(&id).ok_or_else(||format!("verified item default components unavailable for item {id}; require reports/minecraft/components/item"))
    }
    pub fn block_use_owners(&self, state: u32) -> Result<(&str, &str)> {
        let facts = &self.block_interactions[self.info(state)?.block];
        Ok((&facts.use_item_on, &facts.use_without_item))
    }
    pub fn block_placement_owners(&self, state: u32) -> Result<(&str, &str, &str)> {
        let facts = &self.block_interactions[self.info(state)?.block];
        Ok((&facts.placement, &facts.replace, &facts.survival))
    }
    pub fn block_default_state(&self, state: u32) -> Result<u32> {
        Ok(self.block_interactions[self.info(state)?.block].default_state)
    }
    pub fn block_replaceable(&self, state: u32) -> Result<bool> {
        Ok(self.block_interactions[self.info(state)?.block].replaceable)
    }
    pub fn item_block(&self, id: u32) -> Result<Option<u32>> {
        self.item_interactions
            .get(&id)
            .map(|facts| facts.block)
            .ok_or_else(|| format!("verified block-item mapping unavailable for item {id}"))
    }
    pub fn resolve_block_state(
        &self,
        name: &str,
        properties: &BTreeMap<String, String>,
    ) -> Result<u32> {
        let name = normalize_id(name)?;
        let block = self
            .names
            .binary_search(&name)
            .map_err(|_| format!("unknown source block {name}"))?;
        let domains = &self.property_domains[&name];
        if properties
            .keys()
            .any(|property| !domains.contains_key(property))
        {
            return Err(format!("unknown source property for block {name}"));
        }
        let defaults = &self.properties[self.block_interactions[block].default_state as usize];
        let key = property_key(domains, |key| {
            properties
                .get(key)
                .or_else(|| defaults.get(key))
                .map(String::as_str)
        })?;
        Ok(self.block_states[block][key])
    }
    pub fn with_state_property(&self, state: u32, key: &str, value: &str) -> Result<u32> {
        let block = self.info(state)?.block;
        let domains = &self.property_domains[&self.names[block]];
        if !domains.contains_key(key) {
            return Err(format!(
                "unknown source property {}/{}",
                self.names[block], key
            ));
        }
        let properties = &self.properties[state as usize];
        let index = property_key(domains, |name| {
            if name == key {
                Some(value)
            } else {
                properties.get(name).map(String::as_str)
            }
        })?;
        Ok(self.block_states[block][index])
    }
    pub fn source_report(&self, name: &str) -> Result<&[u8]> {
        match name {
            "creative_tabs" => Ok(&self.creative_tabs_report),
            _ => Err(format!("unknown private source report {name}")),
        }
    }
    fn item_facts(&self, id: u32) -> Result<&ItemInteraction> {
        self.item_interactions
            .get(&id)
            .ok_or_else(|| format!("verified item facts unavailable for item {id}"))
    }
    pub fn item_class(&self, id: u32) -> Result<&str> {
        Ok(&self.item_facts(id)?.class)
    }
    pub fn item_description_id(&self, id: u32) -> Result<&str> {
        Ok(&self.item_facts(id)?.description_id)
    }
    pub fn item_name_owner(&self, id: u32) -> Result<&str> {
        Ok(&self.item_facts(id)?.name_owner)
    }
    pub fn set_enabled_features(&mut self, features: Arc<BTreeSet<String>>) -> Result<()> {
        self.enabled_features = features;
        self.rebuild_fuels()
    }
    pub fn fuel_items(&self) -> &[u32] {
        &self.fuel_items
    }
    pub fn item_is_fuel(&self, id: u32) -> Result<bool> {
        self.item_facts(id)?;
        Ok(self.fuel_members.contains(&id))
    }
    fn rebuild_fuels(&mut self) -> Result<()> {
        let mut ordered = Vec::new();
        let mut members = BTreeSet::new();
        for rule in &self.fuel_rules {
            let singleton;
            let ids: &[i32] = match &rule.member {
                FuelMember::Item(id) => {
                    singleton = [*id as i32];
                    &singleton
                }
                FuelMember::Tag(name) => self
                    .registry_tag_optional("minecraft:item", name)?
                    .unwrap_or(&[]),
            };
            if rule.remove {
                ordered.retain(|id| !ids.contains(&(*id as i32)));
                for id in ids {
                    members.remove(&(*id as u32));
                }
            } else {
                for id in ids {
                    if members.insert(*id as u32) {
                        ordered.push(*id as u32);
                    }
                }
            }
        }
        let mut retained = 0;
        for index in 0..ordered.len() {
            let id = ordered[index];
            if self
                .item_required_features(id)?
                .iter()
                .all(|feature| self.enabled_features.contains(feature))
            {
                ordered[retained] = id;
                retained += 1;
            } else {
                members.remove(&id);
            }
        }
        ordered.truncate(retained);
        self.fuel_items = ordered;
        self.fuel_members = members;
        Ok(())
    }
    pub fn item_can_fit_container(&self, id: u32) -> Result<bool> {
        Ok(self.item_facts(id)?.can_fit_container)
    }
    pub fn item_is_banner(&self, id: u32) -> Result<bool> {
        Ok(self.item_facts(id)?.is_banner)
    }
    pub fn item_is_bundle(&self, id: u32) -> Result<bool> {
        Ok(self.item_facts(id)?.is_bundle)
    }
    pub fn item_is_brewing_ingredient(&self, id: u32) -> Result<bool> {
        Ok(self.item_facts(id)?.brewing_ingredient)
    }
    pub fn item_crafting_remainder(&self, id: u32) -> Result<Option<&Value>> {
        Ok(self.item_facts(id)?.crafting_remainder.as_ref())
    }
    pub fn item_use_owner(&self, id: u32, on_block: bool) -> Result<&str> {
        let facts = self
            .item_interactions
            .get(&id)
            .ok_or_else(|| format!("verified item use owners unavailable for item {id}"))?;
        Ok(if on_block {
            &facts.on_block
        } else {
            &facts.air
        })
    }
    pub fn block_can_open_by_hand(&self, state: u32) -> Result<bool> {
        self.block_interactions[self.info(state)?.block]
            .hand_open
            .ok_or_else(|| format!("hand-opening property unavailable for block state {state}"))
    }
    pub fn item_is_compostable(&self, id: u32) -> bool {
        self.item_interactions
            .get(&id)
            .is_some_and(|facts| facts.compost_chance.is_some())
    }
    pub fn block_signal_owners(&self, state: u32) -> Result<(&str, &str)> {
        let facts = &self.block_interactions[self.info(state)?.block];
        Ok((&facts.signal, &facts.direct_signal))
    }
    pub fn block_bucket_owners(
        &self,
        state: u32,
    ) -> Result<(Option<&str>, Option<&str>, Option<&str>)> {
        let facts = &self.block_interactions[self.info(state)?.block];
        Ok((
            facts.bucket_pickup.as_deref(),
            facts.can_place_liquid.as_deref(),
            facts.place_liquid.as_deref(),
        ))
    }
    pub fn item_spawn_type(&self, id: u32) -> Result<Option<&str>> {
        Ok(self.item_facts(id)?.spawn_type.as_deref())
    }
    pub fn set_registries(&mut self, registries: BTreeMap<String, Vec<(String, Value)>>) {
        self.registries = registries;
    }
    /// Received registry tags belong to the same login generation as the holder tables.
    pub fn set_tags(&mut self, tags: BTreeMap<String, BTreeMap<String, Vec<i32>>>) -> Result<()> {
        for (registry, entries) in &tags {
            let holders = self.registry_entries(registry)?;
            for (tag, ids) in entries {
                split_id(tag)?;
                if ids
                    .iter()
                    .any(|id| *id < 0 || *id as usize >= holders.len())
                {
                    return Err(format!("tag holder outside registry {registry}/{tag}"));
                }
            }
        }
        self.tags = Arc::new(
            tags.into_iter()
                .map(|(registry, tags)| {
                    (
                        registry,
                        tags.into_iter()
                            .map(|(tag, ids)| (tag, RegistryTag::new(ids)))
                            .collect(),
                    )
                })
                .collect(),
        );
        self.rebuild_fuels()
    }
    pub fn registry_tag(&self, registry: &str, tag: &str) -> Result<&[i32]> {
        self.registry_tag_record(registry, tag)?
            .map(|tag| tag.ordered.as_slice())
            .ok_or_else(|| format!("received registry tag unavailable {registry}/{tag}"))
    }
    pub fn registry_tag_optional(&self, registry: &str, tag: &str) -> Result<Option<&[i32]>> {
        Ok(self
            .registry_tag_record(registry, tag)?
            .map(|tag| tag.ordered.as_slice()))
    }
    pub fn registry_tags<'a>(
        &'a self,
        registry: &str,
    ) -> Result<impl Iterator<Item = (&'a str, &'a [i32])>> {
        self.registry_entries(registry)?;
        Ok(self
            .tags
            .get(registry)
            .into_iter()
            .flat_map(|entries| entries.iter())
            .map(|(name, tag)| (name.as_str(), tag.ordered.as_slice())))
    }
    pub fn font_ids(&self) -> &[String] {
        &self.font_ids
    }
    pub fn registry_tag_contains(&self, registry: &str, tag: &str, id: i32) -> Result<bool> {
        Ok(self
            .registry_tag_record(registry, tag)?
            .is_some_and(|tag| tag.contains(id)))
    }
    fn registry_tag_record(&self, registry: &str, tag: &str) -> Result<Option<&RegistryTag>> {
        self.registry_entries(registry)?;
        let tag = tag.strip_prefix('#').unwrap_or(tag);
        split_id(tag)?;
        let entries = self.tags.get(registry);
        if tag.contains(':') {
            Ok(entries.and_then(|entries| entries.get(tag)))
        } else {
            let canonical = normalize_id(tag)?;
            Ok(entries.and_then(|entries| entries.get(&canonical)))
        }
    }
    pub fn block_has_tag(&self, state: u32, tag: &str) -> Result<bool> {
        let id = self.info(state)?.registry_id as i32;
        self.registry_tag_contains("minecraft:block", tag, id)
    }
    pub fn holder_set_matches_block(&self, holders: &Value, state: u32) -> Result<bool> {
        let info = self.info(state)?;
        match holders {
            Value::String(name) if name.starts_with('#') => self.block_has_tag(state, name),
            Value::String(name) => {
                let name = normalize_id(name)?;
                if !self
                    .registry_entries("minecraft:block")?
                    .iter()
                    .any(|v| v.0 == name)
                {
                    return Err(format!("unknown block holder {name}"));
                }
                Ok(self.names[info.block] == name)
            }
            Value::Number(id) => {
                Ok(id.as_i64().ok_or("block holder must be integer")?
                    == i64::from(info.registry_id))
            }
            Value::Array(values) => {
                let mut matched = false;
                for value in values {
                    matched |= self.holder_set_matches_block(value, state)?;
                }
                Ok(matched)
            }
            _ => Err("invalid block holder set".into()),
        }
    }
    pub fn registry_entries(&self, registry: &str) -> Result<&[(String, Value)]> {
        self.registries
            .get(registry)
            .or_else(|| self.static_registries.get(registry))
            .map(Vec::as_slice)
            .ok_or_else(|| format!("unknown registry {registry}"))
    }
    pub fn registry_name(&self, registry: &str, id: i32) -> Result<&str> {
        self.registry_entries(registry)?
            .get(
                usize::try_from(id)
                    .map_err(|_| format!("negative registry holder {registry}/{id}"))?,
            )
            .map(|v| v.0.as_str())
            .ok_or_else(|| format!("unknown registry holder {registry}/{id}"))
    }
    pub fn registry_entry(&self, registry: &str, id: i32) -> Result<&Value> {
        self.registry_entries(registry)?
            .get(
                usize::try_from(id)
                    .map_err(|_| format!("negative registry holder {registry}/{id}"))?,
            )
            .map(|v| &v.1)
            .ok_or_else(|| format!("unknown registry holder {registry}/{id}"))
    }
    pub fn collision_boxes(&self, state: u32) -> Option<&[[f64; 6]]> {
        match self.states.get(state as usize)?.collision {
            Collision::Empty => Some(&[]),
            Collision::Cube => Some(&[[0., 0., 0., 1., 1., 1.]]),
            Collision::Shape(id) => self.shapes.get(id).map(Vec::as_slice),
            Collision::Unsupported => None,
        }
    }
    pub fn info(&self, state: u32) -> Result<&BlockInfo> {
        self.states
            .get(state as usize)
            .ok_or_else(|| format!("unknown mandatory block state {state}"))
    }
    pub fn name(&self, state: u32) -> Option<&str> {
        self.states
            .get(state as usize)
            .map(|s| self.names[s.block].as_str())
    }
    pub fn fluid_textures(&self, kind: FluidKind) -> Result<FluidTextures> {
        let (still, flow, overlay, layer, tint) = match kind {
            FluidKind::Water => (
                "block/water_still",
                "block/water_flow",
                Some("block/water_overlay"),
                RenderLayer::Translucent,
                TintKind::Water,
            ),
            FluidKind::Lava => (
                "block/lava_still",
                "block/lava_flow",
                None,
                RenderLayer::Opaque,
                TintKind::None,
            ),
        };
        Ok(FluidTextures {
            still: self.texture(still).ok_or("missing fluid still sprite")?,
            flow: self.texture(flow).ok_or("missing fluid flow sprite")?,
            overlay: overlay
                .map(|s| self.texture(s).ok_or("missing fluid overlay sprite"))
                .transpose()?,
            layer,
            tint,
        })
    }
    pub fn grass_tint(&self, temperature: f32, downfall: f32) -> [f32; 3] {
        color_from_map(&self.grass_colormap, temperature, downfall)
    }
    pub fn grass_color(&self, biome: &Value) -> Option<[f32; 3]> {
        self.biome_colors(biome, 0., 0.).ok().map(|b| b.grass)
    }
    pub fn biome_colors(&self, biome: &Value, x: f64, z: f64) -> Result<BiomeColors> {
        let effects = &biome["effects"];
        let t = biome["temperature"]
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or("biome temperature")? as f32;
        let d = biome["downfall"]
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or("biome downfall")? as f32;
        let base = parse_color(&effects["grass_color"])
            .unwrap_or_else(|| packed(color_from_map(&self.grass_colormap, t, d)));
        let grass = match effects["grass_color_modifier"].as_str().unwrap_or("none") {
            "none" => base,
            "dark_forest" => ((base & 0xfefefe) + 0x28340a) >> 1,
            "swamp" => {
                if self.swamp_noise.value(x * 0.0225, z * 0.0225) < -0.1 {
                    0x4c763c
                } else {
                    0x6a7039
                }
            }
            m => return Err(format!("unsupported mandatory grass modifier {m}")),
        };
        let foliage = parse_color(&effects["foliage_color"])
            .map(rgb)
            .unwrap_or_else(|| color_from_map(&self.foliage_colormap, t, d));
        let dry_foliage = parse_color(&effects["dry_foliage_color"])
            .map(rgb)
            .unwrap_or_else(|| color_from_map(&self.dry_foliage_colormap, t, d));
        let water = rgb(parse_color(&effects["water_color"]).ok_or("biome water color")?);
        let sky = attribute_color(biome, "minecraft:visual/sky_color", 0)?;
        let fog = attribute_color(biome, "minecraft:visual/fog_color", 0)?;
        let water_fog = attribute_color(biome, "minecraft:visual/water_fog_color", 0x050533)?;
        Ok(BiomeColors {
            grass: rgb(grass),
            foliage,
            dry_foliage,
            water,
            sky,
            fog,
            water_fog,
        })
    }
    pub fn resource_bytes_used(&self) -> usize {
        self.rgba.len()
            + self.mips.iter().map(|m| m.rgba.len()).sum::<usize>()
            + self
                .animations
                .iter()
                .flat_map(|a| &a.frames)
                .flat_map(|levels| levels.iter())
                .map(|p| p.len())
                .sum::<usize>()
            + self
                .source
                .loader
                .values()
                .map(|bytes| bytes.len())
                .sum::<usize>()
            + self.loader_retained_bytes()
    }
    /// Deltas must be merged by rectangle into a latest-frame snapshot by the scene owner.
    pub fn animation_patches(
        &self,
        previous_tick: u64,
        tick: u64,
        out: &mut Vec<AtlasPatch>,
    ) -> Result<()> {
        out.clear();
        for animation in &self.animations {
            let current = animation.frame(tick);
            let previous = animation.frame(previous_tick);
            if current != previous {
                for level in 0..animation.frames[current.0].len() {
                    let pixels = if animation.interpolate && current.1 != 0 {
                        let (index, elapsed, duration, next) = current;
                        let a = &animation.frames[index][level];
                        let b = &animation.frames[next][level];
                        let mut bytes = Vec::with_capacity(a.len());
                        let fraction = elapsed as f64 / duration as f64;
                        for (i, (&a, &b)) in a.iter().zip(b.iter()).enumerate() {
                            bytes.push(if i % 4 == 3 {
                                a
                            } else {
                                (f64::from(a) * (1. - fraction) + f64::from(b) * fraction) as u8
                            });
                        }
                        Arc::new(bytes)
                    } else {
                        animation.frames[current.0][level].clone()
                    };
                    out.push(AtlasPatch {
                        mip_level: level as u32,
                        layer: animation.layer,
                        origin: animation.origin.map(|v| v >> level),
                        size: animation.size.map(|v| v >> level),
                        rgba: pixels,
                        sequence: tick,
                    });
                }
            }
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn insert_test_item_components(
        &mut self,
        id: u32,
        components: BTreeMap<String, Value>,
    ) {
        self.item_defaults.insert(id, Arc::new(components));
    }
    #[cfg(test)]
    pub(crate) fn insert_test_texture(&mut self, name: &str, region: TextureRegion) {
        self.textures.insert(name.to_owned(), region);
    }
    #[cfg(test)]
    pub(crate) fn insert_test_state_properties(
        &mut self,
        state: u32,
        properties: BTreeMap<String, String>,
    ) {
        self.properties[state as usize] = properties;
    }
    #[cfg(test)]
    pub(crate) fn insert_test_resource(&mut self, path: &str, bytes: &[u8]) {
        self.source.loader.insert(path.to_owned(), Arc::from(bytes));
    }
    #[cfg(test)]
    pub(crate) fn insert_test_font(&mut self, id: &str, definition: &Value) {
        let (namespace, path) = id.split_once(':').unwrap();
        self.font_ids.push(id.to_owned());
        self.insert_test_resource(
            &format!("assets/{namespace}/font/{path}.json"),
            &serde_json::to_vec(definition).unwrap(),
        );
    }
    #[cfg(test)]
    pub(crate) fn insert_test_item_interaction(
        &mut self,
        id: u32,
        class: &str,
        air: &str,
        on_block: &str,
    ) {
        let name = self
            .item_names
            .entry(id)
            .or_insert_with(|| format!("test:item_{id}"));
        self.required_features
            .items
            .insert(name.clone(), Arc::from([]));
        self.item_interactions.insert(
            id,
            ItemInteraction {
                air: Arc::from(air),
                on_block: Arc::from(on_block),
                compost_chance: None,
                block: None,
                class: Arc::from(class),
                description_id: "item.test.fixture".into(),
                name_owner: Arc::from("Item"),
                can_fit_container: true,
                is_banner: false,
                is_bundle: false,
                brewing_ingredient: false,
                crafting_remainder: None,
                spawn_type: None,
            },
        );
    }
    #[cfg(test)]
    pub(crate) fn insert_test_block_interaction(&mut self, state: u32, replaceable: bool) {
        let facts = BlockInteraction {
            use_item_on: Arc::from("BlockBehaviour"),
            use_without_item: Arc::from("BlockBehaviour"),
            placement: Arc::from("Block"),
            replace: Arc::from("BlockBehaviour"),
            survival: Arc::from("BlockBehaviour"),
            hand_open: None,
            default_state: state,
            replaceable,
            signal: Arc::from("BlockBehaviour"),
            direct_signal: Arc::from("BlockBehaviour"),
            bucket_pickup: None,
            can_place_liquid: None,
            place_liquid: None,
        };
        let index = state as usize;
        if index == self.block_interactions.len() {
            self.block_interactions.push(facts);
        } else {
            self.block_interactions[index] = facts;
        }
    }
    #[cfg(test)]
    pub(crate) fn test_cube() -> Self {
        let quad = |face| BakedQuad {
            positions: super::models::face_positions(face, [0.; 3], [1.; 3]),
            uv: [[0., 0.], [0., 1.], [1., 1.], [1., 0.]],
            tint: TintKind::None,
            tint_index: None,
            cullface: Some(face),
            face,
            shade: true,
            ambient_occlusion: true,
            layer: RenderLayer::Opaque,
            alpha: 1.,
            texture_layer: 0,
        };
        let mut model = BakedModel::default();
        model.quads = (0..6).map(quad).collect();
        Self {
            names: vec!["test:air".into(), "test:cube".into()],
            states: vec![
                BlockInfo {
                    block: 0,
                    registry_id: 0,
                    is_air: true,
                    is_signal_source: false,
                    model: None,
                    hardness: 0.,
                    requires_tool: false,
                    collision: Collision::Empty,
                    occludes: [false; 6],
                    fluid: None,
                    solid: false,
                    shade_brightness: 1.,
                    friction: 0.6,
                    speed_factor: 1.,
                    jump_factor: 1.,
                    view_blocking: false,
                    light_dampening: 0,
                    light_emission: 0,
                    solid_render: false,
                    collision_full: false,
                    sturdy_faces: [false; 6],
                    is_ice_block: false,
                    can_occlude: false,
                },
                BlockInfo {
                    block: 1,
                    registry_id: 1,
                    is_air: false,
                    is_signal_source: false,
                    model: Some(0),
                    hardness: 1.,
                    requires_tool: false,
                    collision: Collision::Cube,
                    occludes: [true; 6],
                    fluid: None,
                    solid: true,
                    shade_brightness: 0.2,
                    friction: 0.6,
                    speed_factor: 1.,
                    jump_factor: 1.,
                    view_blocking: true,
                    light_dampening: 15,
                    light_emission: 0,
                    solid_render: true,
                    collision_full: true,
                    sturdy_faces: [true; 6],
                    is_ice_block: false,
                    can_occlude: true,
                },
            ],
            models: vec![model],
            item_names: BTreeMap::new(),
            sound_names: BTreeMap::new(),
            components: BTreeMap::new(),
            attribute_names: BTreeMap::new(),
            entity_names: BTreeMap::new(),
            block_entity_names: BTreeMap::new(),
            particle_names: BTreeMap::new(),
            rgba: Arc::new(vec![255; 4]),
            width: 1,
            height: 1,
            texture_layers: 1,
            presentation_unsupported: 0,
            source: ResourceSource {
                roots: vec![PathBuf::from(".")],
                indexed: BTreeMap::new(),
                loader: BTreeMap::new(),
                limits: ResourceLimits::default(),
            },
            textures: BTreeMap::new(),
            texture_materials: BTreeMap::new(),
            grass_colormap: vec![[255; 3]; 65536],
            foliage_colormap: vec![[255; 3]; 65536],
            dry_foliage_colormap: vec![[255; 3]; 65536],
            swamp_noise: Simplex2::new(2345),
            shapes: Vec::new(),
            terrain: vec![TerrainFact::fixture(false), TerrainFact::fixture(true)],
            properties: vec![BTreeMap::new(); 2],
            property_domains: BTreeMap::new(),
            required_features: FeatureRequirements::default(),
            animations: Vec::new(),
            item_defaults: BTreeMap::new(),
            block_interactions: Vec::new(),
            item_interactions: BTreeMap::new(),
            placement: placement::PlacementFacts::default(),
            block_states: vec![vec![0], vec![1]],
            creative_tabs_report: Arc::from([]),
            font_ids: Vec::new(),
            fuel_rules: Vec::new(),
            fuel_items: Vec::new(),
            fuel_members: BTreeSet::new(),
            enabled_features: Arc::new(BTreeSet::new()),
            static_registries: BTreeMap::new(),
            registries: BTreeMap::new(),
            tags: Arc::new(BTreeMap::new()),
            data_root: PathBuf::from("."),
            mips: Arc::new(Vec::new()),
            dynamic: Mutex::new(DynamicAtlas {
                first_layer: 0,
                entries: BTreeMap::new(),
                sequence: 0,
            }),
            loader_base: None,
            loader_items: BTreeMap::new(),
            loader_blocks: Vec::new(),
            loader_carriers: BTreeMap::new(),
            loader_model_base: None,
        }
    }
    /// Appends a non-occluding leaves state (`minecraft:oak_leaves`, cutout
    /// cube model) to the test cube content and returns its state id.
    #[cfg(test)]
    pub(crate) fn add_test_leaves(&mut self) -> u32 {
        let mut model = self.models[0].clone();
        for quad in &mut model.quads {
            quad.layer = RenderLayer::Cutout;
        }
        self.models.push(model);
        let id = self.states.len() as u32;
        let mut info = self.states[1].clone();
        info.block = id as usize;
        info.registry_id = id;
        info.model = Some(self.models.len() - 1);
        info.occludes = [false; 6];
        info.can_occlude = false;
        info.solid_render = false;
        info.view_blocking = false;
        info.light_dampening = 1;
        info.sturdy_faces = [false; 6];
        self.states.push(info);
        self.names.push("minecraft:oak_leaves".into());
        self.terrain.push(TerrainFact::fixture(false));
        self.properties.push(BTreeMap::new());
        self.block_states.push(vec![id]);
        id
    }
    /// Appends a source water state (no model, `fluid` set) to the test cube
    /// content and returns its state id.
    #[cfg(test)]
    pub(crate) fn add_test_water(&mut self) -> u32 {
        let id = self.states.len() as u32;
        let mut info = self.states[0].clone();
        info.block = id as usize;
        info.registry_id = id;
        info.is_air = false;
        info.fluid = Some(FluidState {
            kind: FluidKind::Water,
            amount: 8,
            falling: false,
            waterlogged: false,
        });
        self.states.push(info);
        self.names.push("minecraft:water".into());
        self.terrain.push(TerrainFact::fixture(false));
        self.properties.push(BTreeMap::new());
        self.block_states.push(vec![id]);
        id
    }
}
fn parse_color(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|v| *v <= 0xffffffff)
        .map(|v| v as u32 & 0xffffff)
        .or_else(|| {
            value
                .as_str()
                .and_then(|v| u32::from_str_radix(v.trim_start_matches('#'), 16).ok())
                .map(|v| v & 0xffffff)
        })
}
fn rgb(value: u32) -> [f32; 3] {
    [
        ((value >> 16) & 255) as f32 / 255.,
        ((value >> 8) & 255) as f32 / 255.,
        (value & 255) as f32 / 255.,
    ]
}
fn packed(c: [f32; 3]) -> u32 {
    ((c[0] * 255.).round() as u32) << 16
        | ((c[1] * 255.).round() as u32) << 8
        | (c[2] * 255.).round() as u32
}
fn color_from_map(map: &[[u8; 3]], temperature: f32, downfall: f32) -> [f32; 3] {
    let temperature = f64::from(temperature.clamp(0., 1.));
    let rainfall = f64::from(downfall.clamp(0., 1.)) * temperature;
    let x = ((1. - temperature) * 255.) as usize;
    let y = ((1. - rainfall) * 255.) as usize;
    map[y * 256 + x].map(|v| f32::from(v) / 255.)
}
fn attribute_color(biome: &Value, key: &str, default: u32) -> Result<[f32; 3]> {
    let v = &biome["attributes"][key];
    if v.is_null() {
        return Ok(rgb(default));
    }
    if let Some(c) = parse_color(v) {
        return Ok(rgb(c));
    }
    let modifier = v["modifier"].as_str().unwrap_or("minecraft:override");
    if modifier != "minecraft:override" && modifier != "override" {
        return Err(format!(
            "biome color attribute modifier {key}/{modifier} requires environment stack evaluation"
        ));
    }
    Ok(rgb(
        parse_color(&v["value"]).ok_or_else(|| format!("invalid color attribute {key}"))?
    ))
}
fn colormap(source: &ResourceSource, name: &str) -> Result<Vec<[u8; 3]>> {
    let image = decode_texture(source, &format!("minecraft:colormap/{name}"))?;
    if image.width != 256 || image.height != 256 {
        return Err(format!("{name} colormap dimensions"));
    }
    Ok(image
        .rgba
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2]])
        .collect())
}
pub(crate) fn sprite_dimensions([width, height]: [u32; 2], metadata: &Value) -> Result<[u32; 2]> {
    let a = &metadata["animation"];
    let size = if a.is_null() {
        [width, height]
    } else {
        let smallest = width.min(height);
        [
            u32::try_from(
                a["width"]
                    .as_u64()
                    .unwrap_or(u64::from(if a["height"].is_null() {
                        smallest
                    } else {
                        width
                    })),
            )
            .map_err(|_| "animation width")?,
            u32::try_from(
                a["height"]
                    .as_u64()
                    .unwrap_or(u64::from(if a["width"].is_null() {
                        smallest
                    } else {
                        height
                    })),
            )
            .map_err(|_| "animation height")?,
        ]
    };
    if size.contains(&0) || width % size[0] != 0 || height % size[1] != 0 {
        return Err("animation frame dimensions do not tile sprite image".into());
    }
    Ok(size)
}
pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}
fn missing_texture() -> Image {
    let mut rgba = vec![0; 16 * 16 * 4];
    for (index, pixel) in rgba.chunks_exact_mut(4).enumerate() {
        let shade = if (index / 16 < 8) ^ (index % 16 < 8) {
            248
        } else {
            0
        };
        pixel.copy_from_slice(&[shade, 0, shade, 255]);
    }
    Image {
        width: 16,
        height: 16,
        rgba,
    }
}
pub(crate) fn decode_texture(source: &ResourceSource, name: &str) -> Result<Image> {
    let (ns, path) = split_id(name)?;
    let relative = format!("assets/{ns}/textures/{path}.png");
    let bytes = source.bytes(&relative)?;
    let loader_texture = source.loader.contains_key(&relative);
    if loader_texture {
        loader_resources::validate_png_header(&bytes, source.limits, name)?;
    }
    let mut decoder = png::Decoder::new(BufReader::new(Cursor::new(bytes)));
    if loader_texture {
        decoder.set_limits(png::Limits {
            bytes: source.limits.max_decoded_bytes.min(32 << 20),
        });
    }
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("{name}: {e}"))?;
    let info = reader.info();
    let width = info.width;
    let height = info.height;
    let bound = u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or("PNG dimension multiplication overflow")?;
    if width == 0
        || height == 0
        || width > source.limits.max_texture_dimension
        || height > source.limits.max_texture_dimension
        || bound > 32 << 20
    {
        return Err(format!(
            "high-resolution texture dimensions/decoded budget exceeded {name} {width}x{height}"
        ));
    }
    let size = reader
        .output_buffer_size()
        .filter(|n| *n <= 32 << 20)
        .ok_or("PNG decoded buffer bound")?;
    let mut bytes = vec![0; size];
    let frame = reader
        .next_frame(&mut bytes)
        .map_err(|e| format!("{name}: {e}"))?;
    let channels = match frame.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        _ => return Err("unexpanded PNG palette".into()),
    };
    let mut rgba = Vec::with_capacity(bound as usize);
    for p in bytes[..frame.buffer_size()].chunks_exact(channels) {
        let c = match channels {
            4 => [p[0], p[1], p[2], p[3]],
            3 => [p[0], p[1], p[2], 255],
            2 => [p[0], p[0], p[0], p[1]],
            _ => [p[0], p[0], p[0], 255],
        };
        rgba.extend_from_slice(&c);
    }
    Ok(Image {
        width,
        height,
        rgba,
    })
}
struct Animation {
    layer: u32,
    origin: [u32; 2],
    size: [u32; 2],
    frames: Vec<Vec<Arc<Vec<u8>>>>,
    timeline: Vec<(usize, u64)>,
    duration: u64,
    interpolate: bool,
}
impl Animation {
    fn frame(&self, tick: u64) -> (usize, u64, u64, usize) {
        let mut elapsed = tick % self.duration;
        for (i, &(frame, duration)) in self.timeline.iter().enumerate() {
            if elapsed < duration {
                return (
                    frame,
                    if self.interpolate { elapsed } else { 0 },
                    duration,
                    self.timeline[(i + 1) % self.timeline.len()].0,
                );
            }
            elapsed -= duration;
        }
        unreachable!("validated animation timeline")
    }
}
#[derive(Default)]
struct Atlas {
    rgba: Vec<u8>,
    mips: Vec<AtlasMip>,
    dynamic_first_layer: u32,
    textures: BTreeMap<String, TextureRegion>,
    materials: BTreeMap<String, RenderLayer>,
    animations: Vec<Animation>,
    side: u32,
    layers: u32,
}
impl Atlas {
    fn build(source: &ResourceSource, names: &BTreeSet<String>) -> Result<Self> {
        const SIDE: u32 = 2048;
        const GUTTER: u32 = 16;
        let page_bytes = (SIDE * SIDE * 4) as usize;
        let mip_bytes = (0..=4)
            .map(|level| page_bytes >> (level * 2))
            .sum::<usize>();
        if 2 * mip_bytes > source.limits.max_atlas_bytes {
            return Err("initial atlas plus reserved dynamic mip budget exceeded".into());
        }
        let mut pages = (0..=4)
            .map(|level| vec![0; page_bytes >> (level * 2)])
            .collect::<Vec<_>>();
        let (mut layers, mut x, mut y, mut row) = (1u32, 0u32, 0u32, 0u32);
        let mut textures = BTreeMap::new();
        let mut materials = BTreeMap::new();
        let mut animations = Vec::new();
        let mut animation_bytes = 0usize;
        for name in ["solaris:white", "minecraft:missingno"].into_iter().chain(
            names
                .iter()
                .map(String::as_str)
                .filter(|name| *name != "solaris:white" && *name != "minecraft:missingno"),
        ) {
            let image = match name {
                "solaris:white" => Image {
                    width: 1,
                    height: 1,
                    rgba: vec![255; 4],
                },
                "minecraft:missingno" => missing_texture(),
                _ => decode_texture(source, name)?,
            };
            if image
                .rgba
                .len()
                .checked_mul(5)
                .and_then(|v| v.checked_add(animation_bytes + (layers as usize + 1) * mip_bytes))
                .is_none_or(|v| v > source.limits.max_decoded_bytes)
            {
                return Err("sprite preprocessing CPU peak byte budget exceeded".into());
            }
            let (_, path) = split_id(name)?;
            let metadata = if name == "solaris:white" || name == "minecraft:missingno" {
                Value::Null
            } else {
                source.texture_metadata(name)?
            };
            let a = &metadata["animation"];
            let animated = !a.is_null();
            let [width, height] = sprite_dimensions([image.width, image.height], &metadata)?;
            if width == 0
                || height == 0
                || width > SIDE - 32
                || height > SIDE - 32
                || image.width % width != 0
                || image.height % height != 0
            {
                return Err(format!(
                    "high-resolution atlas sprite/animation bounds {name} {width}x{height}; maximum2016x2016"
                ));
            }
            let frames_count = (image.width / width) * (image.height / height);
            let default_time = a["frametime"].as_u64().unwrap_or(1);
            if default_time == 0 || default_time > 100000 {
                return Err(format!("invalid animation frame time {name}"));
            }
            let mut timeline = Vec::new();
            if let Some(frames) = a["frames"].as_array() {
                if frames.is_empty() || frames.len() > 16384 {
                    return Err("animation frame count bound".into());
                }
                for f in frames {
                    let index =
                        f.as_u64()
                            .or_else(|| f["index"].as_u64())
                            .filter(|v| *v < u64::from(frames_count))
                            .ok_or("animation frame index bound")? as usize;
                    let duration = f["time"].as_u64().unwrap_or(default_time);
                    if duration == 0 || duration > 100000 {
                        return Err("animation duration bound".into());
                    }
                    timeline.push((index, duration));
                }
            } else {
                timeline = (0..frames_count as usize)
                    .map(|i| (i, default_time))
                    .collect();
            }
            let duration = timeline.iter().map(|v| v.1).sum();
            let padded_width = (width + 32).div_ceil(16) * 16;
            let padded_height = (height + 32).div_ceil(16) * 16;
            if x + padded_width > SIDE {
                x = 0;
                y += row;
                row = 0;
            }
            if y + padded_height > SIDE {
                layers += 1;
                if (layers as usize + 1) * mip_bytes > source.limits.max_atlas_bytes {
                    return Err(
                        "high-resolution resource pack atlas mip CPU/GPU byte budget exceeded"
                            .into(),
                    );
                }
                for (level, page) in pages.iter_mut().enumerate() {
                    page.resize(layers as usize * (page_bytes >> (level * 2)), 0);
                }
                x = 0;
                y = 0;
                row = 0;
            }
            let origin = [x, y];
            let size = [padded_width, padded_height];
            let layer = layers - 1;
            let transparent = image.rgba.chunks_exact(4).any(|p| p[3] == 0);
            let has_alpha = image.rgba.chunks_exact(4).any(|p| p[3] != 255);
            let intermediate = image.rgba.chunks_exact(4).any(|p| p[3] != 0 && p[3] != 255);
            let strategy =
                super::models::mipmaps::Strategy::parse(&metadata["texture"], transparent)?;
            let bias = metadata["texture"]["alpha_cutoff_bias"]
                .as_f64()
                .unwrap_or(0.) as f32;
            let columns = image.width / width;
            let images =
                super::models::mipmaps::generate(image, strategy, bias, path.starts_with("item/"))?;
            let mut frames = Vec::with_capacity(frames_count as usize);
            for frame in 0..frames_count {
                let mut levels = Vec::with_capacity(5);
                for (level, image) in images.iter().enumerate() {
                    let pw = padded_width >> level;
                    let ph = padded_height >> level;
                    let fw = (width >> level).max(1);
                    let fh = (height >> level).max(1);
                    let gutter = GUTTER >> level;
                    let fx = (frame % columns * width) >> level;
                    let fy = (frame / columns * height) >> level;
                    let mut pixels = vec![0; (pw * ph * 4) as usize];
                    for py in 0..ph {
                        for px in 0..pw {
                            let ix =
                                (px.saturating_sub(gutter).min(fw - 1) + fx).min(image.width - 1);
                            let iy =
                                (py.saturating_sub(gutter).min(fh - 1) + fy).min(image.height - 1);
                            let src = ((iy * image.width + ix) * 4) as usize;
                            let dst = ((py * pw + px) * 4) as usize;
                            pixels[dst..dst + 4].copy_from_slice(&image.rgba[src..src + 4]);
                        }
                    }
                    levels.push(Arc::new(pixels));
                }
                frames.push(levels);
            }
            for (level, page) in pages.iter_mut().enumerate() {
                let pw = padded_width >> level;
                let ph = padded_height >> level;
                let side = SIDE >> level;
                let first = &frames[timeline[0].0][level];
                let px = x >> level;
                let py = y >> level;
                for row in 0..ph {
                    let dst = layer as usize * (page_bytes >> (level * 2))
                        + (((py + row) * side + px) * 4) as usize;
                    let src = (row * pw * 4) as usize;
                    page[dst..dst + (pw * 4) as usize]
                        .copy_from_slice(&first[src..src + (pw * 4) as usize]);
                }
            }
            materials.insert(
                name.to_owned(),
                if intermediate {
                    RenderLayer::Translucent
                } else if has_alpha {
                    RenderLayer::Cutout
                } else {
                    RenderLayer::Opaque
                },
            );
            textures.insert(
                name.to_owned(),
                TextureRegion {
                    uv: [
                        (x + 16) as f32 / SIDE as f32,
                        (y + 16) as f32 / SIDE as f32,
                        (x + 16 + width) as f32 / SIDE as f32,
                        (y + 16 + height) as f32 / SIDE as f32,
                    ],
                    layer,
                    width,
                    height,
                },
            );
            if animated && (frames_count > 1 || timeline.len() > 1) {
                animation_bytes += frames
                    .iter()
                    .flat_map(|levels| levels.iter())
                    .map(|p| p.len())
                    .sum::<usize>();
                if animation_bytes + (layers as usize + 1) * mip_bytes
                    > source.limits.max_decoded_bytes
                {
                    return Err("animated sprite mip CPU decoded memory budget exceeded".into());
                }
                animations.push(Animation {
                    layer,
                    origin,
                    size,
                    frames,
                    timeline,
                    duration,
                    interpolate: a["interpolate"].as_bool().unwrap_or(false),
                });
            }
            x += padded_width;
            row = row.max(padded_height);
        }
        let dynamic_first_layer = layers;
        layers += 1;
        if layers as usize * mip_bytes > source.limits.max_atlas_bytes
            || animation_bytes + layers as usize * mip_bytes > source.limits.max_decoded_bytes
        {
            return Err("reserved dynamic atlas and mip memory budget exceeded".into());
        }
        for (level, page) in pages.iter_mut().enumerate() {
            page.resize(layers as usize * (page_bytes >> (level * 2)), 0);
        }
        let rgba = pages.remove(0);
        let mips = pages
            .into_iter()
            .enumerate()
            .map(|(index, rgba)| AtlasMip {
                level: index as u32 + 1,
                width: SIDE >> (index + 1),
                height: SIDE >> (index + 1),
                rgba: Arc::new(rgba),
            })
            .collect();
        Ok(Self {
            rgba,
            mips,
            dynamic_first_layer,
            textures,
            materials,
            animations,
            side: SIDE,
            layers,
        })
    }
}
struct DynamicAtlas {
    first_layer: u32,
    entries: BTreeMap<String, TextureRegion>,
    sequence: u64,
}
struct ShapeTable {
    state_ids: Vec<usize>,
    fingerprints: Vec<u64>,
    shapes: Vec<Vec<[f64; 6]>>,
}
impl ShapeTable {
    fn load(path: &Path) -> Result<Self> {
        let b = bounded_bytes(path, 32 << 20)?;
        if b.len() < 32 || &b[..8] != b"SOLCOLL1" {
            return Err("collision table magic/header mismatch".into());
        }
        let u16_at = |i| u16::from_be_bytes([b[i], b[i + 1]]);
        let u32_at = |i| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as usize;
        let states = u32_at(12);
        let count = u32_at(16);
        let boxes = u32_at(20);
        if u16_at(8) != 4096
            || u32_at(24) != 1
            || u32_at(28) != 0
            || states > 1000000
            || count > 65535
            || boxes > 1000000
        {
            return Err("collision table format/count bound".into());
        }
        let fingerprints_offset = 32 + states * 2;
        let offsets = fingerprints_offset + states * 8;
        let coordinates = offsets + (count + 1) * 4;
        if b.len() != coordinates + boxes * 12 || u32_at(offsets + count * 4) != boxes {
            return Err("collision table length/terminal offset mismatch".into());
        }
        let mut state_ids = Vec::with_capacity(states);
        let mut fingerprints = Vec::with_capacity(states);
        for i in 0..states {
            let shape = u16_at(32 + i * 2) as usize;
            if shape >= count {
                return Err(format!("missing collision state {i}"));
            }
            state_ids.push(shape);
            fingerprints.push(u64::from_be_bytes(
                b[fingerprints_offset + i * 8..fingerprints_offset + i * 8 + 8]
                    .try_into()
                    .unwrap(),
            ));
        }
        let mut shapes = Vec::with_capacity(count);
        for i in 0..count {
            let start = u32_at(offsets + i * 4);
            let end = u32_at(offsets + (i + 1) * 4);
            if start > end || end > boxes {
                return Err("collision shape offset bound".into());
            }
            let mut shape = Vec::with_capacity(end - start);
            for index in start..end {
                let c = std::array::from_fn(|n| {
                    f64::from(i16::from_be_bytes([
                        b[coordinates + index * 12 + n * 2],
                        b[coordinates + index * 12 + n * 2 + 1],
                    ])) / 4096.
                });
                if (0..3).any(|axis| c[axis] >= c[axis + 3]) {
                    return Err("invalid collision box".into());
                }
                shape.push(c);
            }
            shapes.push(shape);
        }
        Ok(Self {
            state_ids,
            fingerprints,
            shapes,
        })
    }
    fn normalize_offsets(&mut self, terrain: &[TerrainFact]) {
        let mut shapes = Vec::new();
        let mut cache = BTreeMap::<Vec<u64>, usize>::new();
        for (id, index) in self.state_ids.iter_mut().enumerate() {
            let mut boxes = self.shapes[*index].clone();
            let offset = terrain[id].offset.at([0; 3]);
            for b in &mut boxes {
                for axis in 0..3 {
                    b[axis] -= offset[axis];
                    b[axis + 3] -= offset[axis];
                }
            }
            let key = boxes.iter().flat_map(|b| b.map(f64::to_bits)).collect();
            *index = *cache.entry(key).or_insert_with(|| {
                let index = shapes.len();
                shapes.push(boxes);
                index
            });
        }
        self.shapes = shapes;
    }
    fn verify(&self, id: usize, name: &str, p: &BTreeMap<String, String>) -> Result<()> {
        let mut hash = 0xcbf29ce484222325u64;
        let mut feed = |bytes: &[u8]| {
            for &b in bytes {
                hash = (hash ^ u64::from(b)).wrapping_mul(0x100000001b3);
            }
        };
        feed(name.as_bytes());
        for (k, v) in p {
            feed(b"\0");
            feed(k.as_bytes());
            feed(b"=");
            feed(v.as_bytes());
        }
        if self.fingerprints[id] != hash {
            return Err(format!(
                "26.1.2 collision state fingerprint mismatch {id}/{name}"
            ));
        }
        Ok(())
    }
}
pub(crate) struct Simplex2 {
    permutation: [u8; 256],
}
impl Simplex2 {
    pub(crate) fn new(seed: i64) -> Self {
        Self::from_random(&mut JavaRandom::new(seed))
    }
    pub(crate) fn from_random(random: &mut JavaRandom) -> Self {
        for _ in 0..3 {
            random.next_double();
        }
        let mut permutation = std::array::from_fn(|i| i as u8);
        for i in 0..256 {
            let j = i + random.next_int((256 - i) as u32) as usize;
            permutation.swap(i, j);
        }
        Self { permutation }
    }
    pub(crate) fn value(&self, x: f64, y: f64) -> f64 {
        let f = (3f64.sqrt() - 1.) / 2.;
        let g = (3. - 3f64.sqrt()) / 6.;
        let skew = (x + y) * f;
        let i = (x + skew).floor() as i32;
        let j = (y + skew).floor() as i32;
        let unskew = f64::from(i + j) * g;
        let x0 = x - (f64::from(i) - unskew);
        let y0 = y - (f64::from(j) - unskew);
        let (ix, iy) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let p = |i: i32| i32::from(self.permutation[(i & 255) as usize]);
        let gradients = [
            [1., 1.],
            [-1., 1.],
            [1., -1.],
            [-1., -1.],
            [1., 0.],
            [-1., 0.],
            [1., 0.],
            [-1., 0.],
            [0., 1.],
            [0., -1.],
            [0., 1.],
            [0., -1.],
        ];
        let corner = |dx: f64, dy: f64, gradient: usize| {
            let t = 0.5 - dx * dx - dy * dy;
            if t < 0. {
                0.
            } else {
                let t = t * t;
                t * t * (gradients[gradient][0] * dx + gradients[gradient][1] * dy)
            }
        };
        70. * (corner(x0, y0, (p(i + p(j)) % 12) as usize)
            + corner(
                x0 - f64::from(ix) + g,
                y0 - f64::from(iy) + g,
                (p(i + ix + p(j + iy)) % 12) as usize,
            )
            + corner(
                x0 - 1. + 2. * g,
                y0 - 1. + 2. * g,
                (p(i + 1 + p(j + 1)) % 12) as usize,
            ))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn received_tag_order_drives_menu_indices_without_changing_membership() {
        let mut content = Content::test_cube();
        content.static_registries.insert(
            "minecraft:block".into(),
            (0..10)
                .map(|id| (format!("test:block_{id}"), Value::Null))
                .collect(),
        );
        content
            .set_tags(BTreeMap::from([(
                "minecraft:block".into(),
                BTreeMap::from([("test:selection".into(), vec![7, 2, 9, 2])]),
            )]))
            .unwrap();
        assert_eq!(
            content
                .registry_tag("minecraft:block", "test:selection")
                .unwrap(),
            &[7, 2, 9, 2]
        );
        assert!(
            content
                .registry_tag_contains("minecraft:block", "test:selection", 2)
                .unwrap()
        );
        assert!(
            !content
                .registry_tag_contains("minecraft:block", "test:selection", 3)
                .unwrap()
        );
        assert_eq!(
            content
                .registry_tag_optional("minecraft:block", "test:absent")
                .unwrap(),
            None
        );
        assert!(
            content
                .registry_tag_optional("test:unobserved_registry", "test:absent")
                .is_err()
        );
        assert!(
            content
                .set_tags(BTreeMap::from([(
                    "minecraft:block".into(),
                    BTreeMap::from([("test:selection".into(), vec![10])]),
                )]))
                .is_err()
        );
        assert_eq!(
            content
                .registry_tag("minecraft:block", "test:selection")
                .unwrap()[0],
            7
        );
    }
    #[test]
    fn namespace_and_path_boundaries_reject_escape() {
        for id in [
            "foo:../escape",
            "foo:/absolute",
            "foo:a//b",
            "foo:a\\b",
            "FOO:a",
            "foo:a:b",
        ] {
            assert!(normalize_id(id).is_err(), "{id}");
        }
        assert_eq!(
            normalize_id("custom:block/stone").unwrap(),
            "custom:block/stone"
        );
    }
    #[test]
    fn cold_and_dry_biomes_use_clamped_color_map_coordinates() {
        let map = (0..65536)
            .map(|i| [(i % 256) as u8, (i / 256) as u8, 0])
            .collect::<Vec<_>>();
        assert_eq!(color_from_map(&map, -0.5, 0.8), [1., 1., 0.]);
        assert_eq!(color_from_map(&map, 2., -1.), [0., 1., 0.]);
    }
    #[test]
    fn biome_override_and_dark_forest_modifier_compose() {
        let c = Content::test_cube();
        let b = serde_json::json!({"temperature":0.5,"downfall":0.5,"effects":{"grass_color":"#80a040","grass_color_modifier":"dark_forest","foliage_color":1122867,"dry_foliage_color":"#aabbcc","water_color":"#3f76e4"},"attributes":{"minecraft:visual/sky_color":"#112233"}});
        let color = c.biome_colors(&b, 0., 0.).unwrap();
        assert_eq!(color.grass, rgb(((0x80a040 & 0xfefefe) + 0x28340a) >> 1));
        assert_eq!(color.foliage, rgb(0x112233));
        assert_eq!(color.sky, rgb(0x112233));
    }
    #[test]
    fn partial_faces_do_not_hide_full_cube_surface() {
        let mut c = Content::test_cube();
        let mut slab = TerrainFact::fixture(true);
        slab.faces[2] = Arc::new(vec![[0., 0., 1., 0.5]]);
        c.terrain.push(slab);
        assert!(c.face_visible(1, 2, 3).unwrap());
        assert!(!c.face_visible(2, 1, 2).unwrap());
        assert!(c.face_visible(1, 0, 2).unwrap());
    }
    #[test]
    fn animation_timeline_preserves_variable_duration_and_wrap() {
        let a = Animation {
            layer: 0,
            origin: [0; 2],
            size: [1; 2],
            frames: vec![vec![Arc::new(vec![0; 4])]; 3],
            timeline: vec![(2, 3), (0, 1)],
            duration: 4,
            interpolate: false,
        };
        assert_eq!(a.frame(2).0, 2);
        assert_eq!(a.frame(3).0, 0);
        assert_eq!(a.frame(4).0, 2);
    }
}
