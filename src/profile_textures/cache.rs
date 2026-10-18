use super::{ResolvedTexture, Result, hex};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};
const MAX_ORIGINAL: usize = 2 << 20;
const MAX_DISK: u64 = 128 << 20;
const MAX_ENTRIES: usize = 256;
const MAX_MEMORY: usize = 64 << 20;
pub(super) struct TextureCache {
    root: PathBuf,
    decoded: VecDeque<(String, ResolvedTexture)>,
    bytes: usize,
}
impl TextureCache {
    pub(super) fn new(root: PathBuf) -> Self {
        Self {
            root,
            decoded: VecDeque::new(),
            bytes: 0,
        }
    }
    pub(super) fn decoded(&mut self, key: &str) -> Option<ResolvedTexture> {
        let index = self.decoded.iter().position(|(name, _)| name == key)?;
        let entry = self.decoded.remove(index)?;
        let texture = entry.1.clone();
        self.decoded.push_back(entry);
        Some(texture)
    }
    pub(super) fn insert(&mut self, key: String, texture: ResolvedTexture) {
        if let Some(index) = self.decoded.iter().position(|(name, _)| name == &key) {
            self.bytes -= self.decoded.remove(index).unwrap().1.rgba.len();
        }
        while self.bytes + texture.rgba.len() > MAX_MEMORY || self.decoded.len() >= MAX_ENTRIES {
            let Some((_, old)) = self.decoded.pop_front() else {
                break;
            };
            self.bytes -= old.rgba.len();
        }
        self.bytes += texture.rgba.len();
        self.decoded.push_back((key, texture));
    }
    fn path(&self, url: &str) -> PathBuf {
        self.root
            .join(format!("{}.asset", hex(&Sha256::digest(url.as_bytes()))))
    }
    fn lock_disk(&self) -> Result<File> {
        fs::create_dir_all(&self.root).map_err(|e| format!("profile cache directory: {e}"))?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("cache.lock"))
            .map_err(|e| format!("profile cache lock: {e}"))?;
        lock.lock()
            .map_err(|e| format!("profile cache interprocess lock: {e}"))?;
        Ok(lock)
    }
    pub(super) fn read_original(&self, url: &str) -> Result<Option<Vec<u8>>> {
        let _lock = self.lock_disk()?;
        let path = self.path(url);
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("profile cache {}: {e}", path.display())),
        };
        let mut checksum = [0; 32];
        let header = file.read_exact(&mut checksum);
        let mut bytes = Vec::new();
        if header.is_ok() {
            file.take(MAX_ORIGINAL as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(|e| format!("profile cache read: {e}"))?;
        }
        // SHA256 catches damaged local cache bytes; only the fixed TLS origin supplies a miss.
        if header.is_err()
            || bytes.len() > MAX_ORIGINAL
            || Sha256::digest(&bytes).as_slice() != checksum
        {
            fs::remove_file(path).map_err(|e| format!("profile corrupt cache removal: {e}"))?;
            return Ok(None);
        }
        Ok(Some(bytes))
    }
    pub(super) fn publish_original(&self, url: &str, bytes: &[u8]) -> Result<()> {
        if bytes.len() > MAX_ORIGINAL {
            return Err("profile cache original byte bound".into());
        }
        let _lock = self.lock_disk()?;
        let path = self.path(url);
        if path.is_file() {
            return Ok(());
        }
        self.prune(bytes.len() as u64 + 32)?;
        let temporary = self.root.join(format!("{}.partial", uuid::Uuid::new_v4()));
        let publish = || -> Result<()> {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|e| format!("profile cache staging: {e}"))?;
            file.write_all(&Sha256::digest(bytes))
                .and_then(|_| file.write_all(bytes))
                .and_then(|_| file.sync_all())
                .map_err(|e| format!("profile cache write: {e}"))?;
            fs::rename(&temporary, &path)
                .map_err(|e| format!("profile cache atomic publication: {e}"))?;
            Ok(())
        };
        let result = publish();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    fn prune(&self, incoming: u64) -> Result<()> {
        let mut entries = Vec::new();
        let mut total = 0;
        for entry in
            fs::read_dir(&self.root).map_err(|e| format!("profile cache enumeration: {e}"))?
        {
            let entry = entry.map_err(|e| format!("profile cache entry: {e}"))?;
            if entry.path().extension().and_then(|x| x.to_str()) != Some("asset") {
                continue;
            }
            let meta = entry
                .metadata()
                .map_err(|e| format!("profile cache metadata: {e}"))?;
            if !meta.is_file() {
                continue;
            }
            total += meta.len();
            entries.push((meta.modified().ok(), meta.len(), entry.path()));
        }
        entries.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        let mut count = entries.len();
        for (_, size, path) in entries {
            if total + incoming <= MAX_DISK && count < MAX_ENTRIES {
                break;
            }
            fs::remove_file(path).map_err(|e| format!("profile cache eviction: {e}"))?;
            total -= size;
            count -= 1;
        }
        Ok(())
    }
}
