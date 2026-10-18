use super::{
    catalog::Catalog,
    playback::{SharedSamples, StreamControl, stream_source},
};
use crate::protocol::{Result, content::json_file};
use rodio::{Decoder, Source};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::{
    collections::BTreeMap,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicU64},
};

const PCM_BUDGET: usize = 96 << 20;
const STATIC_SAMPLES: usize = 4_194_304;
struct Cached {
    data: Arc<Vec<f32>>,
    channels: u16,
    rate: u32,
    last: u64,
}
pub(super) struct Resources {
    pub catalog: Catalog,
    index: Value,
    assets: PathBuf,
    cache: BTreeMap<String, Cached>,
    clock: u64,
    pub catalog_hash: String,
}
impl Resources {
    pub fn load(decoded: &AtomicU64) -> Result<Self> {
        let path = PathBuf::from(
            std::env::var("SOLARIS_CLIENT_ASSET_INDEX")
                .map_err(|_| "verified sound asset index missing")?,
        );
        let index = json_file(&path, 16 << 20)?;
        let assets = PathBuf::from(
            std::env::var("SOLARIS_CLIENT_ASSET_ROOT").map_err(|_| "sound asset root missing")?,
        );
        let bytes = verified_asset(&index, &assets, "minecraft/sounds.json", 16 << 20)?;
        let catalog_hash = format!("{:x}", Sha1::digest(&bytes));
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let objects = index["objects"]
            .as_object()
            .ok_or("sound asset index object table missing")?;
        let mut namespaces = 0;
        for key in objects
            .keys()
            .filter(|k| k.ends_with("/sounds.json") && k.as_str() != "minecraft/sounds.json")
        {
            let namespace = key
                .strip_suffix("/sounds.json")
                .ok_or("sound catalog path")?;
            if namespace.contains('/') {
                continue;
            }
            namespaces += 1;
            if namespaces > 64 {
                return Err("sound namespace count bound".into());
            }
            let other: Value =
                serde_json::from_slice(&verified_asset(&index, &assets, key, 16 << 20)?)
                    .map_err(|e| e.to_string())?;
            for (name, event) in other
                .as_object()
                .ok_or("invalid namespaced sound catalog")?
            {
                let name = if name.contains(':') {
                    name.clone()
                } else {
                    format!("{namespace}:{name}")
                };
                if value
                    .as_object_mut()
                    .ok_or("invalid primary sound catalog")?
                    .insert(name, event.clone())
                    .is_some()
                {
                    return Err("duplicate indexed sound event".into());
                }
            }
        }
        let catalog = Catalog::parse(&value)?;
        let preloads: Vec<String> = catalog.preloads().map(str::to_owned).collect();
        let mut resources = Self {
            catalog,
            index,
            assets,
            cache: BTreeMap::new(),
            clock: 0,
            catalog_hash,
        };
        for name in preloads {
            resources.static_source(&name, false, decoded)?;
        }
        Ok(resources)
    }
    pub fn source(
        &mut self,
        name: &str,
        stream: bool,
        looping: bool,
        decoded: Arc<AtomicU64>,
        underruns: Arc<AtomicU64>,
    ) -> Result<(Box<dyn Source<Item = f32> + Send>, Option<StreamControl>)> {
        if stream {
            let bytes = Arc::<[u8]>::from(self.bytes(name, 32 << 20)?);
            let source = stream_source(bytes, looping, decoded, underruns)?;
            let control = source.control();
            Ok((Box::new(source), Some(control)))
        } else {
            Ok((Box::new(self.static_source(name, looping, &decoded)?), None))
        }
    }
    fn bytes(&self, name: &str, limit: u64) -> Result<Vec<u8>> {
        let (namespace, path) = name
            .split_once(':')
            .ok_or("sound resource must be namespaced")?;
        verified_asset(
            &self.index,
            &self.assets,
            &format!("{namespace}/sounds/{path}.ogg"),
            limit,
        )
    }
    fn static_source(
        &mut self,
        name: &str,
        looping: bool,
        decoded: &AtomicU64,
    ) -> Result<SharedSamples> {
        self.clock = self.clock.wrapping_add(1);
        if !self.cache.contains_key(name) {
            let bytes = self.bytes(name, 8 << 20)?;
            let mut decoder = Decoder::try_from(Cursor::new(bytes)).map_err(|e| e.to_string())?;
            let channels = decoder.channels();
            let rate = decoder.sample_rate();
            validate_format(channels, rate)?;
            let limit = STATIC_SAMPLES.min(rate as usize * channels as usize * 60);
            // Reserve before decode, including pinned voices. A reload keeps the old catalog
            // until this bounded staging succeeds; no old source can point at a new resource.
            self.reserve(limit * 4)?;
            let mut data = Vec::with_capacity(4096.min(limit));
            for sample in decoder.by_ref() {
                if data.len() == limit {
                    return Err("static sound decoded sample bound".into());
                }
                if data.len() == data.capacity() {
                    data.reserve_exact(data.capacity().min(limit - data.capacity()));
                }
                data.push(sample);
            }
            if data.is_empty() || data.len() % channels as usize != 0 {
                return Err(
                    "static sound duration/frame bound (use a streamed catalog entry for music)"
                        .into(),
                );
            }
            if data.iter().any(|v| !v.is_finite()) {
                return Err("nonfinite decoded sound".into());
            }
            decoded.fetch_add(data.len() as u64, std::sync::atomic::Ordering::Relaxed);
            self.cache.insert(
                name.to_owned(),
                Cached {
                    data: Arc::new(data),
                    channels,
                    rate,
                    last: self.clock,
                },
            );
        }
        let cached = self.cache.get_mut(name).ok_or("sound cache missing")?;
        cached.last = self.clock;
        Ok(SharedSamples::new(
            cached.data.clone(),
            cached.channels,
            cached.rate,
            looping,
        ))
    }
    fn reserve(&mut self, bytes: usize) -> Result<()> {
        loop {
            let used: usize = self.cache.values().map(|c| c.data.capacity() * 4).sum();
            if used + bytes <= PCM_BUDGET && self.cache.len() < 256 {
                return Ok(());
            }
            let victim = self
                .cache
                .iter()
                .filter(|(_, c)| Arc::strong_count(&c.data) == 1)
                .min_by_key(|(_, c)| c.last)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    self.cache.remove(&k);
                }
                None => return Err("sound PCM budget pinned by active voices".into()),
            }
        }
    }
    pub fn cache_bytes(&self) -> usize {
        self.cache.values().map(|c| c.data.capacity() * 4).sum()
    }
}
pub(super) fn validate_format(channels: u16, rate: u32) -> Result<()> {
    if !(1..=2).contains(&channels) || !(8000..=192000).contains(&rate) {
        return Err("sound channel/sample rate bound".into());
    }
    Ok(())
}
fn verified_asset(index: &Value, assets: &Path, key: &str, maximum: u64) -> Result<Vec<u8>> {
    let object = &index["objects"][key];
    let hash = object["hash"]
        .as_str()
        .ok_or_else(|| format!("missing verified sound asset {key}"))?;
    if hash.len() != 40 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid sound object digest".into());
    }
    let path = assets.join("objects").join(&hash[..2]).join(hash);
    let mut file = fs::File::open(&path).map_err(|e| e.to_string())?;
    let size = file.metadata().map_err(|e| e.to_string())?.len();
    if size > maximum || object["size"].as_u64() != Some(size) {
        return Err(format!("sound asset size bound/mismatch: {key}"));
    }
    use std::io::Read;
    let mut bytes = Vec::with_capacity(size as usize);
    file.by_ref()
        .take(maximum + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 != size || format!("{:x}", Sha1::digest(&bytes)) != hash {
        return Err(format!("sound asset SHA1/size mismatch: {key}"));
    }
    Ok(bytes)
}
