//! Mojang cosmetics are verified and decoded on bounded workers, never on the event loop.
//! Source: authlib 7.0.63 services/property/URL contracts and 26.1.2 SkinTextureDownloader.
use crate::entities::{PlayerModel, ProfileProperty, ProfileTextureRequest};
use base64::{Engine, engine::general_purpose::STANDARD};
use parking_lot::{Mutex, RwLock};
use reqwest::{Url, blocking::Client, redirect::Policy};
use rsa::{
    RsaPublicKey,
    pkcs1v15::{Signature, VerifyingKey},
    pkcs8::DecodePublicKey,
    signature::hazmat::PrehashVerifier,
    traits::PublicKeyParts,
};
use serde_json::Value;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    io::Read,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    time::{Duration, Instant},
};
mod cache;
mod image;
#[cfg(test)]
mod tests;

pub type Result<T, E = String> = std::result::Result<T, E>;
const PUBLIC_KEYS_URL: &str = "https://api.minecraftservices.com/publickeys";
const MAX_PENDING: usize = 32;
const WORKERS: usize = 2;
const MAX_PROPERTY: usize = 32767;
const MAX_IMAGE_BYTES: usize = 2 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureTrust {
    Signed,
    Unsigned,
    Invalid,
}
#[derive(Debug)]
pub struct ServiceKeySet {
    pub(crate) profile_property: Vec<VerifyingKey<Sha1>>,
    pub(crate) player_certificate: Vec<VerifyingKey<Sha1>>,
    pub(crate) revision: u64,
}
impl ServiceKeySet {
    pub fn profile_key_count(&self) -> usize {
        self.profile_property.len()
    }
    pub fn certificate_key_count(&self) -> usize {
        self.player_certificate.len()
    }
    pub fn classify_property(&self, property: &ProfileProperty) -> TextureTrust {
        let Some(signature) = &property.signature else {
            return TextureTrust::Unsigned;
        };
        if property.value.len() > MAX_PROPERTY || signature.len() > 8192 {
            return TextureTrust::Invalid;
        }
        let Ok(signature) = STANDARD.decode(signature) else {
            return TextureTrust::Invalid;
        };
        let Ok(signature) = Signature::try_from(signature.as_slice()) else {
            return TextureTrust::Invalid;
        };
        let digest = Sha1::digest(property.value.as_bytes());
        if self
            .profile_property
            .iter()
            .any(|key| key.verify_prehash(&digest, &signature).is_ok())
        {
            TextureTrust::Signed
        } else {
            TextureTrust::Invalid
        }
    }
    fn parse(bytes: &[u8], revision: u64) -> Result<Self> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|e| format!("Mojang publickeys JSON: {e}"))?;
        fn keys(value: &Value, field: &str) -> Result<Vec<VerifyingKey<Sha1>>> {
            let Some(entries) = value.get(field).filter(|value| !value.is_null()) else {
                return Ok(Vec::new());
            };
            let entries = entries
                .as_array()
                .ok_or_else(|| format!("Mojang publickeys malformed {field}"))?;
            if entries.len() > 16 {
                return Err(format!("Mojang publickeys {field} count bound"));
            }
            entries
                .iter()
                .map(|entry| {
                    let text = entry
                        .get("publicKey")
                        .and_then(Value::as_str)
                        .ok_or("Mojang publickeys missing publicKey")?;
                    if text.len() > 8192 {
                        return Err("Mojang publickeys DER bound".into());
                    }
                    let der = STANDARD
                        .decode(text)
                        .map_err(|e| format!("Mojang publickey base64: {e}"))?;
                    let key = RsaPublicKey::from_public_key_der(&der)
                        .map_err(|e| format!("Mojang X509 RSA publickey: {e}"))?;
                    if !(2048..=8192).contains(&key.n().bits()) {
                        return Err("Mojang RSA key size bound".into());
                    }
                    Ok(VerifyingKey::new(key))
                })
                .collect()
        }
        Ok(Self {
            profile_property: keys(&value, "profilePropertyKeys")?,
            player_certificate: keys(&value, "playerCertificateKeys")?,
            revision,
        })
    }
}
#[derive(Clone, Default)]
pub struct MojangServicesKeys(Arc<RwLock<Option<Arc<ServiceKeySet>>>>);
impl MojangServicesKeys {
    /// A snapshot only: callers must never perform certificate-key HTTP on the event loop.
    pub fn snapshot(&self) -> Option<Arc<ServiceKeySet>> {
        self.0.read().clone()
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedTexture {
    pub id: String,
    pub rgba: Arc<[u8]>,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug)]
pub struct ResolvedProfileTextures {
    pub skin: Option<ResolvedTexture>,
    pub cape: Option<ResolvedTexture>,
    pub elytra: Option<ResolvedTexture>,
    /// None preserves the recipient's source UUID-hash default when no skin is supplied.
    pub model: Option<PlayerModel>,
    pub secure: bool,
}
#[derive(Debug)]
pub struct ProfileTextureCompletion {
    pub uuid: String,
    pub generation: u64,
    pub resource_generation: u64,
    pub result: Result<ResolvedProfileTextures>,
}
#[derive(Debug)]
pub enum ResolverCompletion {
    ServicesKeys(Result<()>),
    Texture(ProfileTextureCompletion),
}
#[derive(Debug)]
pub enum SubmitError {
    Full(ProfileTextureRequest),
    Closed(ProfileTextureRequest),
    Stale(ProfileTextureRequest),
}
struct Job {
    request: ProfileTextureRequest,
    resource_generation: u64,
}
enum Work {
    Keys,
    Texture(Job),
}
#[derive(Default)]
struct KeyFetchState {
    next: Option<Instant>,
    failures: u8,
    error: Option<String>,
}
struct Shared {
    completions: Mutex<VecDeque<ResolverCompletion>>,
    pending: AtomicUsize,
    resource_generation: AtomicU64,
    stopped: AtomicBool,
    wake: Arc<dyn Fn() + Send + Sync>,
    keys: MojangServicesKeys,
    key_fetch: Mutex<KeyFetchState>,
    keys_queued: AtomicBool,
    cache: Mutex<cache::TextureCache>,
}
impl Shared {
    fn complete(&self, result: ResolverCompletion) {
        // Every queued completion owns one of MAX_PENDING admission tokens; no result drops.
        self.completions.lock().push_back(result);
        (self.wake)();
    }
    fn current(&self, generation: u64) -> bool {
        !self.stopped.load(Ordering::Acquire)
            && self.resource_generation.load(Ordering::Acquire) == generation
    }
    fn fetch_keys(&self, client: &Client) -> Result<Arc<ServiceKeySet>> {
        let mut fetched = self.key_fetch.lock();
        if fetched.next.is_some_and(|next| next > Instant::now()) {
            return self.keys.snapshot().ok_or_else(|| {
                fetched
                    .error
                    .clone()
                    .unwrap_or_else(|| "Mojang services keys unavailable".into())
            });
        }
        let revision = self
            .keys
            .snapshot()
            .map_or(1, |keys| keys.revision.wrapping_add(1));
        let result = https_bytes(client, Url::parse(PUBLIC_KEYS_URL).unwrap(), 128 << 10)
            .and_then(|bytes| ServiceKeySet::parse(&bytes, revision))
            .map(Arc::new);
        match &result {
            Ok(keys) => {
                *self.keys.0.write() = Some(keys.clone());
                fetched.error = None;
                fetched.next = Some(Instant::now() + Duration::from_secs(24 * 60 * 60));
            }
            Err(error) => {
                fetched.error = Some(error.clone());
                // authlib retains the last TLS-verified key set on refresh failure. A missing
                // first key set backs off 5 * 2^min(failureCount,6) minutes, never busy-retries.
                let delay = if self.keys.snapshot().is_some() {
                    24 * 60 * 60
                } else {
                    let delay = 5 * 60 * (1u64 << fetched.failures.min(6));
                    fetched.failures = fetched.failures.saturating_add(1);
                    delay
                };
                fetched.next = Some(Instant::now() + Duration::from_secs(delay));
            }
        }
        // The completion preserves actual HTTP/parse errors even when an older key set remains.
        result
    }
    fn resolve(&self, client: &Client, job: &Job) -> Result<ResolvedProfileTextures> {
        if !self.current(job.resource_generation) {
            return Err("profile texture resource generation cancelled".into());
        }
        let property = &job.request.property;
        if property.name != "textures" || property.value.len() > MAX_PROPERTY {
            return Err("profile texture property name/size invalid".into());
        }
        let trust = if property.signature.is_some() {
            match self.keys.snapshot() {
                Some(keys) => keys,
                None => self.fetch_keys(client)?,
            }
            .classify_property(property)
        } else {
            TextureTrust::Unsigned
        };
        if trust == TextureTrust::Invalid {
            return Err("invalid Mojang profile property signature".into());
        }
        if job.request.require_secure && trust != TextureTrust::Signed {
            return Err("unsigned remote profile texture rejected".into());
        }
        let bytes = STANDARD
            .decode(&property.value)
            .map_err(|e| format!("profile texture payload base64: {e}"))?;
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|e| format!("profile texture payload JSON: {e}"))?;
        let textures = value
            .get("textures")
            .and_then(Value::as_object)
            .ok_or("profile texture payload missing textures")?;
        if textures.len() > 3 {
            return Err("profile texture type count bound".into());
        }
        // Deliberately do not compare payload.profileId with the recipient: NPC copied skins are valid.
        for texture in textures.values() {
            allowed_texture_url(
                texture
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or("profile texture missing URL")?,
            )?;
        }
        let load = |kind: &str, skin: bool| -> Result<Option<ResolvedTexture>> {
            let Some(texture) = textures.get(kind) else {
                return Ok(None);
            };
            if !self.current(job.resource_generation) {
                return Err("profile texture resource generation cancelled".into());
            }
            let url = allowed_texture_url(
                texture
                    .get("url")
                    .and_then(Value::as_str)
                    .ok_or("profile texture missing URL")?,
            )?;
            self.load_texture(client, url, skin).map(Some)
        };
        let skin = load("SKIN", true)?;
        let cape = load("CAPE", false)?;
        let elytra = load("ELYTRA", false)?;
        let model = textures.get("SKIN").map(|skin| {
            if skin.pointer("/metadata/model").and_then(Value::as_str) == Some("slim") {
                PlayerModel::Slim
            } else {
                PlayerModel::Wide
            }
        });
        if !self.current(job.resource_generation) {
            return Err("profile texture resource generation cancelled".into());
        }
        Ok(ResolvedProfileTextures {
            skin,
            cape,
            elytra,
            model,
            secure: trust == TextureTrust::Signed,
        })
    }
    fn load_texture(&self, client: &Client, url: Url, skin: bool) -> Result<ResolvedTexture> {
        let key = format!("{}:{}", if skin { "skin-v1" } else { "image-v1" }, url);
        if let Some(image) = self.cache.lock().decoded(&key) {
            return Ok(image);
        }
        let cached = self.cache.lock().read_original(url.as_str())?;
        let bytes = match cached {
            Some(bytes) => bytes,
            None => https_bytes(client, url.clone(), MAX_IMAGE_BYTES)?,
        };
        let mut decoded = image::decode_png(&bytes, skin)?;
        let digest = hex(&Sha256::digest(&bytes));
        decoded.id = format!(
            "solaris:profile/{}_{digest}",
            if skin { "skin" } else { "texture" }
        );
        let mut cache = self.cache.lock();
        cache.publish_original(url.as_str(), &bytes)?;
        cache.insert(key, decoded.clone());
        Ok(decoded)
    }
}

pub struct Resolver {
    sender: Option<SyncSender<Work>>,
    shared: Arc<Shared>,
}
impl Resolver {
    /// Starts two fixed workers. The callback must post an event-loop user event, not block.
    pub fn new(cache_root: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(MAX_PENDING);
        let receiver = Arc::new(Mutex::new(receiver));
        let shared = Arc::new(Shared {
            completions: Mutex::new(VecDeque::new()),
            pending: AtomicUsize::new(1),
            resource_generation: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
            wake,
            keys: MojangServicesKeys::default(),
            key_fetch: Mutex::new(KeyFetchState::default()),
            keys_queued: AtomicBool::new(true),
            cache: Mutex::new(cache::TextureCache::new(cache_root)),
        });
        for worker in 0..WORKERS {
            let receiver = receiver.clone();
            let state = shared.clone();
            if let Err(error) = std::thread::Builder::new()
                .name(format!("profile-texture-{worker}"))
                .spawn(move || {
                    let client = Client::builder()
                        .https_only(true)
                        .redirect(Policy::none())
                        .connect_timeout(Duration::from_secs(5))
                        .timeout(Duration::from_secs(15))
                        .user_agent("SolarisNativeClient/26.1.2")
                        .build()
                        .map_err(|e| format!("profile texture TLS client: {e}"));
                    loop {
                        let work = match receiver.lock().recv() {
                            Ok(work) => work,
                            Err(_) => break,
                        };
                        if state.stopped.load(Ordering::Acquire) {
                            break;
                        }
                        match work {
                            Work::Keys => {
                                let result = match &client {
                                    Ok(client) => state.fetch_keys(client).map(|_| ()),
                                    Err(e) => Err(e.clone()),
                                };
                                state.keys_queued.store(false, Ordering::Release);
                                state.complete(ResolverCompletion::ServicesKeys(result));
                            }
                            Work::Texture(job) => {
                                let result = match &client {
                                    Ok(client) => state.resolve(client, &job),
                                    Err(e) => Err(e.clone()),
                                };
                                state.complete(ResolverCompletion::Texture(
                                    ProfileTextureCompletion {
                                        uuid: job.request.uuid,
                                        generation: job.request.generation,
                                        resource_generation: job.resource_generation,
                                        result,
                                    },
                                ));
                            }
                        }
                    }
                })
            {
                shared.stopped.store(true, Ordering::Release);
                return Err(format!("profile texture worker: {error}"));
            }
        }
        sender
            .try_send(Work::Keys)
            .map_err(|_| "profile services startup queue unavailable")?;
        Ok(Self {
            sender: Some(sender),
            shared,
        })
    }
    pub fn services_keys(&self) -> MojangServicesKeys {
        self.shared.keys.clone()
    }
    /// Call on incoming chat/profile-key events, not from a periodic event-loop poll.
    /// Fresh keys (24h source interval), already queued work and backpressure return false.
    pub fn request_services_keys(&self) -> Result<bool> {
        if self.shared.stopped.load(Ordering::Acquire) {
            return Err("profile resolver closed".into());
        }
        let Some(fetched) = self.shared.key_fetch.try_lock() else {
            return Ok(false);
        };
        if fetched.next.is_some_and(|next| next > Instant::now()) {
            return Ok(false);
        }
        drop(fetched);
        if self.shared.keys_queued.swap(true, Ordering::AcqRel) {
            return Ok(false);
        }
        if self
            .shared
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_PENDING).then_some(n + 1)
            })
            .is_err()
        {
            self.shared.keys_queued.store(false, Ordering::Release);
            return Ok(false);
        }
        match self
            .sender
            .as_ref()
            .ok_or("profile resolver closed")?
            .try_send(Work::Keys)
        {
            Ok(()) => Ok(true),
            Err(error) => {
                self.shared.pending.fetch_sub(1, Ordering::AcqRel);
                self.shared.keys_queued.store(false, Ordering::Release);
                match error {
                    TrySendError::Full(_) => Ok(false),
                    TrySendError::Disconnected(_) => Err("profile resolver worker closed".into()),
                }
            }
        }
    }
    /// Parent advances this on resource reload and connection teardown before submitting new jobs.
    /// A connection change must also use a new resource generation (or replace the resolver).
    pub fn set_fence(&self, resource_generation: u64) {
        self.shared
            .resource_generation
            .store(resource_generation, Ordering::Release);
    }
    pub fn submit(
        &self,
        request: ProfileTextureRequest,
        resource_generation: u64,
    ) -> std::result::Result<(), SubmitError> {
        if !self.shared.current(resource_generation) {
            return Err(SubmitError::Stale(request));
        }
        // Key refresh admission is event-driven and never waits for the worker's fetch lock.
        let _ = self.request_services_keys();
        let Some(sender) = &self.sender else {
            return Err(SubmitError::Closed(request));
        };
        if self
            .shared
            .pending
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < MAX_PENDING).then_some(n + 1)
            })
            .is_err()
        {
            return Err(SubmitError::Full(request));
        }
        match sender.try_send(Work::Texture(Job {
            request,
            resource_generation,
        })) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.shared.pending.fetch_sub(1, Ordering::AcqRel);
                match error {
                    TrySendError::Full(Work::Texture(job)) => Err(SubmitError::Full(job.request)),
                    TrySendError::Disconnected(Work::Texture(job)) => {
                        Err(SubmitError::Closed(job.request))
                    }
                    _ => unreachable!(),
                }
            }
        }
    }
    /// Drain only in response to the producer's wake event; this is not a polling API.
    pub fn try_completion(&self) -> Option<ResolverCompletion> {
        let result = self.shared.completions.lock().pop_front()?;
        self.shared.pending.fetch_sub(1, Ordering::AcqRel);
        Some(result)
    }
}
impl Drop for Resolver {
    fn drop(&mut self) {
        // Never join network workers on the UI thread. Closing admission wakes blocked receivers;
        // in-flight bounded HTTP may finish, but stopped/resource fences prevent installation.
        self.shared.stopped.store(true, Ordering::Release);
        self.sender.take();
    }
}

pub fn allowed_texture_url(text: &str) -> Result<Url> {
    // authlib's URI checker is exact-host and case-sensitive before URL normalization.
    let (scheme, rest) = text
        .split_once("://")
        .ok_or("profile texture URL scheme missing")?;
    if scheme != "http" && scheme != "https" {
        return Err("profile texture URL scheme rejected".into());
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority
        .rsplit('@')
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    if host != "textures.minecraft.net" {
        return Err("profile texture URL domain rejected".into());
    }
    let mut url = Url::parse(text).map_err(|e| format!("profile texture URL: {e}"))?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|port| port != 80 && port != 443)
    {
        return Err("profile texture URL credentials/port rejected".into());
    }
    if url.host_str() != Some("textures.minecraft.net") {
        return Err("profile texture URL domain rejected".into());
    }
    url.set_scheme("https")
        .map_err(|_| "profile texture HTTPS upgrade failed")?;
    url.set_port(None)
        .map_err(|_| "profile texture HTTPS port upgrade failed")?;
    url.set_fragment(None);
    Ok(url)
}
fn https_bytes(client: &Client, url: Url, bound: usize) -> Result<Vec<u8>> {
    let response = client
        .get(url.clone())
        .send()
        .map_err(|e| format!("HTTPS {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "HTTPS {url}: status {} (redirects forbidden)",
            response.status()
        ));
    }
    if response.content_length().is_some_and(|n| n > bound as u64) {
        return Err(format!("HTTPS {url}: content length bound"));
    }
    let mut bytes = Vec::new();
    response
        .take(bound as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("HTTPS {url}: body {e}"))?;
    if bytes.len() > bound {
        return Err(format!("HTTPS {url}: streamed body bound"));
    }
    Ok(bytes)
}
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
