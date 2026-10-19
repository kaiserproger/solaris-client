//! Resumable, hash-verified, parallel downloads.
use crate::progress::{Cancelled, Reporter};
use anyhow::{Context, Result, bail};
use reqwest::{StatusCode, blocking::Client, header};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Clone, Debug)]
pub enum Check {
    Sha1(String),
    Sha256(String),
}

#[derive(Clone, Debug)]
pub struct Job {
    pub url: String,
    pub dest: PathBuf,
    pub check: Check,
    pub size: u64,
}

pub fn client() -> Result<Client> {
    Client::builder()
        .user_agent(concat!("solaris-launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        // blocking reqwest has no per-read timeout; bound each request instead
        // (an interrupted attempt resumes from its `.partial` file).
        .timeout(Duration::from_secs(20 * 60))
        .build()
        .context("building HTTP client")
}

pub fn hash_file(path: &Path, check: &Check) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buffer = vec![0u8; 256 * 1024];
    macro_rules! digest {
        ($hasher:expr) => {{
            let mut hasher = $hasher;
            loop {
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buffer[..n]);
            }
            hex(&hasher.finalize())
        }};
    }
    Ok(match check {
        Check::Sha1(_) => digest!(Sha1::new()),
        Check::Sha256(_) => digest!(Sha256::new()),
    })
}

fn expected(check: &Check) -> &str {
    match check {
        Check::Sha1(h) | Check::Sha256(h) => h,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// True when `path` already holds exactly the expected bytes.
pub fn is_valid(path: &Path, size: u64, check: &Check) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == size)
        && hash_file(path, check).is_ok_and(|h| h.eq_ignore_ascii_case(expected(check)))
}

pub fn fetch_bytes(client: &Client, url: &str, check: Option<&Check>) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url}"))?;
    let bytes = response.bytes().with_context(|| format!("reading {url}"))?.to_vec();
    if let Some(check) = check {
        let got = match check {
            Check::Sha1(_) => hex(&Sha1::digest(&bytes)),
            Check::Sha256(_) => hex(&Sha256::digest(&bytes)),
        };
        if !got.eq_ignore_ascii_case(expected(check)) {
            bail!("checksum mismatch for {url}: expected {} got {got}", expected(check));
        }
    }
    Ok(bytes)
}

/// Marker error so callers can tell a user cancel from a network failure.
#[derive(Debug)]
pub struct UserCancelled;
impl std::fmt::Display for UserCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}
impl std::error::Error for UserCancelled {}
impl From<Cancelled> for UserCancelled {
    fn from(_: Cancelled) -> Self {
        UserCancelled
    }
}

/// One attempt at one file. Bytes already counted into the reporter are tracked
/// in `counted` so a failed attempt can roll its contribution back.
fn attempt(client: &Client, job: &Job, rep: &Reporter, counted: &mut u64) -> Result<()> {
    if let Some(parent) = job.dest.parent() {
        fs::create_dir_all(parent)?;
    }
    if is_valid(&job.dest, job.size, &job.check) {
        rep.add_done(job.size);
        *counted += job.size;
        return Ok(());
    }
    let _ = fs::remove_file(&job.dest);
    let mut partial_name = job.dest.file_name().unwrap().to_os_string();
    partial_name.push(".partial");
    let partial = job.dest.with_file_name(partial_name);
    let mut have = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
    if have > job.size {
        let _ = fs::remove_file(&partial);
        have = 0;
    }
    if have < job.size || job.size == 0 {
        let mut request = client.get(&job.url);
        if have > 0 {
            request = request.header(header::RANGE, format!("bytes={have}-"));
        }
        let mut response = request.send().with_context(|| format!("GET {}", job.url))?;
        let status = response.status();
        if status == StatusCode::RANGE_NOT_SATISFIABLE {
            let _ = fs::remove_file(&partial);
            bail!("server rejected resume range for {}", job.url);
        }
        let mut out = if status == StatusCode::PARTIAL_CONTENT && have > 0 {
            OpenOptions::new().append(true).open(&partial)?
        } else if status.is_success() {
            have = 0;
            File::create(&partial)?
        } else {
            bail!("GET {} -> HTTP {status}", job.url);
        };
        rep.add_done(have);
        *counted += have;
        let mut buffer = vec![0u8; 128 * 1024];
        loop {
            rep.check_cancel().map_err(|_| UserCancelled)?;
            let n = response.read(&mut buffer).with_context(|| format!("reading {}", job.url))?;
            if n == 0 {
                break;
            }
            out.write_all(&buffer[..n])?;
            rep.add_done(n as u64);
            *counted += n as u64;
        }
        out.flush()?;
    } else {
        rep.add_done(have);
        *counted += have;
    }
    let got = hash_file(&partial, &job.check)?;
    if !got.eq_ignore_ascii_case(expected(&job.check)) {
        let _ = fs::remove_file(&partial);
        bail!(
            "checksum mismatch for {}: expected {} got {got}",
            job.url,
            expected(&job.check)
        );
    }
    fs::rename(&partial, &job.dest).with_context(|| format!("finalizing {}", job.dest.display()))?;
    Ok(())
}

/// Download one file with up to three attempts (resuming partial data).
pub fn download(client: &Client, job: &Job, rep: &Reporter) -> Result<()> {
    let mut last = None;
    for try_index in 0..3 {
        let mut counted = 0;
        match attempt(client, job, rep, &mut counted) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if error.downcast_ref::<UserCancelled>().is_some() {
                    return Err(error);
                }
                // Keep the .partial for resume; only un-count what this try added.
                rep.sub_done(counted);
                rep.log(format!("retry {}/3 for {}: {error:#}", try_index + 1, job.url));
                last = Some(error);
                std::thread::sleep(Duration::from_millis(600 * (try_index + 1) as u64));
            }
        }
    }
    Err(last.unwrap())
}

/// Run all jobs on `threads` workers; the first failure stops the rest.
pub fn download_all(client: &Client, jobs: Vec<Job>, threads: usize, rep: &Reporter) -> Result<()> {
    rep.set_total(jobs.iter().map(|j| j.size).sum());
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..threads.clamp(1, 32) {
            scope.spawn(|| {
                loop {
                    if failure.lock().unwrap().is_some() {
                        return;
                    }
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(job) = jobs.get(index) else { return };
                    if let Err(error) = download(client, job, rep) {
                        failure.lock().unwrap().get_or_insert(error);
                        return;
                    }
                }
            });
        }
    });
    match failure.into_inner().unwrap() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::Arc,
    };

    /// Tiny HTTP/1.1 server: serves `body`, honouring `Range: bytes=N-`; records ranges.
    fn serve(body: Arc<Vec<u8>>, ranges: Arc<Mutex<Vec<u64>>>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/file", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut start = 0u64;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = range.trim().trim_end_matches('-').parse().unwrap();
                        ranges.lock().unwrap().push(start);
                    }
                }
                let mut out = stream;
                let (status, slice) = if start > 0 {
                    ("206 Partial Content", &body[start as usize..])
                } else {
                    ("200 OK", &body[..])
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len());
                let _ = out.write_all(head.as_bytes());
                let _ = out.write_all(slice);
            }
        });
        url
    }

    fn job(url: String, dest: PathBuf, body: &[u8]) -> Job {
        Job {
            url,
            dest,
            check: Check::Sha256(hex(&Sha256::digest(body))),
            size: body.len() as u64,
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sl-net-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn downloads_resumes_and_verifies() {
        let body: Arc<Vec<u8>> = Arc::new((0..300_000u32).map(|i| (i % 251) as u8).collect());
        let ranges = Arc::new(Mutex::new(Vec::new()));
        let url = serve(body.clone(), ranges.clone());
        let dir = temp("ok");
        let client = client().unwrap();
        let rep = Reporter::new(None, false);

        // Fresh download.
        let dest = dir.join("a/fresh.bin");
        download(&client, &job(url.clone(), dest.clone(), &body), &rep).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), *body);

        // A partial file resumes with a Range request and ends up identical.
        let dest = dir.join("resumed.bin");
        fs::write(dir.join("resumed.bin.partial"), &body[..100_000]).unwrap();
        download(&client, &job(url.clone(), dest.clone(), &body), &rep).unwrap();
        assert_eq!(fs::read(&dest).unwrap(), *body);
        assert_eq!(*ranges.lock().unwrap(), vec![100_000]);

        // An already-valid file is not fetched again.
        let before = ranges.lock().unwrap().len();
        download(&client, &job(url, dest, &body), &rep).unwrap();
        assert_eq!(ranges.lock().unwrap().len(), before);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn corrupt_download_is_rejected_and_not_installed() {
        let body = Arc::new(vec![7u8; 4096]);
        let url = serve(body.clone(), Arc::new(Mutex::new(Vec::new())));
        let dir = temp("bad");
        let dest = dir.join("bad.bin");
        let mut bad = job(url, dest.clone(), &body);
        bad.check = Check::Sha256("0".repeat(64));
        let error = download(&client().unwrap(), &bad, &Reporter::new(None, false)).unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"), "{error:#}");
        assert!(!dest.exists());
        let _ = fs::remove_dir_all(dir);
    }
}
