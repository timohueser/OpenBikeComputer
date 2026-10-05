//! One file over HTTP into the store: retries, a `.part` file that the next attempt resumes with a
//! `Range` request, and a digest check before the file becomes an object.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::date;
use crate::store::{self, Lock, Store};

/// What a download must be.
#[derive(Default)]
pub struct Expect<'a> {
    /// The SHA-256 of the bytes.
    pub sha256: Option<&'a str>,
    /// The latest `Last-Modified` day, `YYYY-MM-DD`. A response without the header counts as
    /// modified today.
    pub modified_by: Option<&'a str>,
}

pub struct Downloaded {
    pub object: PathBuf,
    pub sha256: String,
    pub size: u64,
}

/// Failed tries in a row before a download gives up. A try that adds bytes to a resumed part
/// resets the count; a try that starts the file again does not, so a file that cannot resume
/// still gives up.
const ATTEMPTS: u32 = 4;
/// The longest time one try receives a body: a stalled transfer fails, and the next try resumes it.
const BODY: Duration = Duration::from_secs(15 * 60);
/// The longest time a HEAD request or a small document may take.
const SMALL: Duration = Duration::from_secs(15);

pub struct Http {
    agent: ureq::Agent,
    backoff: Duration,
}

enum Failure {
    Retry(String),
    /// Retry, after a try that added bytes to a resumed part.
    Grew(String),
    Final(String),
}

impl Http {
    pub fn new() -> Self {
        Self::with_backoff(Duration::from_secs(2))
    }

    /// The first retry waits `backoff`; each later one waits twice as long as the one before.
    pub fn with_backoff(backoff: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(concat!("OpenBikeComputer obc-data/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(BODY))
            .build();
        Self { agent: config.into(), backoff }
    }

    /// The `Last-Modified` day of `url`, after redirects.
    pub fn modified(&self, url: &str) -> Result<Option<String>, String> {
        let request = self.agent.head(url).config().timeout_global(Some(SMALL)).build();
        let response = request.call().map_err(|e| format!("HEAD {url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("HEAD {url}: HTTP {}", response.status().as_u16()));
        }
        Ok(header(&response, "last-modified").and_then(|value| date::from_http(&value)))
    }

    /// Where `url` redirects to, from one HEAD request.
    pub fn location(&self, url: &str) -> Result<String, String> {
        let request = self.agent.head(url).config().timeout_global(Some(SMALL)).max_redirects(0).build();
        let response = request.call().map_err(|e| format!("HEAD {url}: {e}"))?;
        header(&response, "location").ok_or_else(|| format!("HEAD {url}: HTTP {} and no redirect", response.status()))
    }

    /// A small document, such as an index or an API answer.
    pub fn text(&self, url: &str, accept: &str) -> Result<String, String> {
        let request = self.agent.get(url).header("accept", accept).config().timeout_global(Some(SMALL)).build();
        let mut response = request.call().map_err(|e| format!("GET {url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("GET {url}: HTTP {}", response.status().as_u16()));
        }
        response.body_mut().read_to_string().map_err(|e| format!("GET {url}: {e}"))
    }

    /// The lock that a download of `url` needs. One process at a time downloads a URL.
    pub fn lock(store: &Store, url: &str) -> Result<Lock, String> {
        store.lock(&format!("download-{}", key(url)))
    }

    /// Download `url` into the store; the caller holds [`Http::lock`]. A failed try keeps its
    /// `.part` file, so the next try, or the next run, asks only for the rest. A digest mismatch
    /// removes it.
    pub fn download(&self, store: &Store, url: &str, expect: &Expect) -> Result<Downloaded, String> {
        if let Some(object) = expect.sha256.map(|sha256| store.object(sha256)).filter(|object| object.is_file()) {
            let size = fs::metadata(&object).map_err(|e| format!("{}: {e}", object.display()))?.len();
            return Ok(Downloaded { object, sha256: expect.sha256.unwrap_or_default().into(), size });
        }
        let part = store.partial(&format!("{}.part", key(url)));
        let validator = store.partial(&format!("{}.validator", key(url)));
        let partial = part.parent().unwrap_or(store.root());
        fs::create_dir_all(partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        let mut failures = 0;
        loop {
            let (why, grew) = match self.attempt(url, &part, &validator, expect) {
                Ok(()) => break,
                Err(Failure::Final(why)) => return Err(why),
                Err(Failure::Retry(why)) => (why, false),
                Err(Failure::Grew(why)) => (why, true),
            };
            failures = if grew { 1 } else { failures + 1 };
            if failures == ATTEMPTS {
                return Err(why);
            }
            eprintln!("obc data: {why} — retrying");
            std::thread::sleep(self.backoff * 2u32.pow(failures - 1));
        }
        let _ = fs::remove_file(&validator);
        let (sha256, size) = store::hash_file(&part)?;
        if let Some(want) = expect.sha256.filter(|want| *want != sha256) {
            let _ = fs::remove_file(&part);
            return Err(format!("{url}: the SHA-256 is {sha256}, not {want}"));
        }
        let object = store.insert(&part, &sha256)?;
        Ok(Downloaded { object, sha256, size })
    }

    fn attempt(&self, url: &str, part: &Path, validator: &Path, expect: &Expect) -> Result<(), Failure> {
        let have = fs::metadata(part).map_or(0, |metadata| metadata.len());
        let saved = fs::read_to_string(validator).ok();
        // Without a validator or a digest, nothing would tell a changed file from the rest of the old one.
        let resume = have > 0 && (saved.is_some() || expect.sha256.is_some());
        // The bytes as upstream stores them: a decoded body has no ranges and another digest.
        let mut request = self.agent.get(url).header("accept-encoding", "identity");
        if resume {
            request = request.header("range", format!("bytes={have}-"));
            if let Some(saved) = &saved {
                request = request.header("if-range", saved);
            }
        }
        let mut response = request.call().map_err(|e| Failure::Retry(format!("GET {url}: {e}")))?;
        let status = response.status().as_u16();
        let modified_by = |response: &ureq::http::Response<_>| {
            let modified = header(response, "last-modified").and_then(|value| date::from_http(&value));
            let modified = modified.unwrap_or_else(|| date::format(date::today()));
            match expect.modified_by.filter(|by| modified.as_str() > *by) {
                Some(by) => Err(Failure::Final(format!("{url} changed on {modified}, after the version {by}"))),
                None => Ok(()),
            }
        };
        let range = header(&response, "content-range").unwrap_or_default();
        let total = range.rsplit_once('/').and_then(|(_, total)| total.parse::<u64>().ok());
        let (append, total) = match status {
            206 if resume => {
                let start = range.strip_prefix("bytes ").and_then(|r| r.split_once('-')).map(|(start, _)| start);
                let changed = matches!((&saved, strong(&response)), (Some(saved), Some(now)) if *saved != now);
                if start != Some(have.to_string().as_str()) || changed {
                    let _ = fs::remove_file(part);
                    return Err(Failure::Retry(format!("GET {url}: answered `{range}` to a resume at {have}")));
                }
                modified_by(&response)?;
                (true, total)
            }
            200 => {
                modified_by(&response)?;
                (false, header(&response, "content-length").and_then(|v| v.parse().ok()))
            }
            // `bytes */T`: the part already holds all T bytes.
            416 if resume && total == Some(have) => return modified_by(&response),
            416 => {
                let _ = fs::remove_file(part);
                return Err(Failure::Retry(format!("GET {url}: HTTP 416 to a resume at {have}")));
            }
            408 | 429 | 500..=599 => return Err(Failure::Retry(format!("GET {url}: HTTP {status}"))),
            _ => return Err(Failure::Final(format!("GET {url}: HTTP {status}"))),
        };
        let file = OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(part);
        let mut file = file.map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
        if !append {
            match strong(&response) {
                Some(value) => store::write_atomic(validator, value.as_bytes()).map_err(Failure::Final)?,
                None => drop(fs::remove_file(validator)),
            }
        }
        let mut reader = response.body_mut().as_reader();
        let mut buffer = vec![0; 1 << 16];
        let mut grew = false;
        let retry = |grew: bool, why: String| if append && grew { Failure::Grew(why) } else { Failure::Retry(why) };
        loop {
            let read = reader.read(&mut buffer).map_err(|e| retry(grew, format!("GET {url}: {e}")))?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
            grew = true;
        }
        file.sync_all().map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
        let length = fs::metadata(part).map_or(0, |metadata| metadata.len());
        match total {
            Some(total) if total != length => {
                Err(retry(grew, format!("GET {url}: the connection closed at {length} of {total} bytes")))
            }
            _ => Ok(()),
        }
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

/// The name of the files that a download of `url` keeps in `partial/`.
fn key(url: &str) -> String {
    store::sha256_hex(url.as_bytes())[..32].to_string()
}

/// A validator that can guard a range: a strong `ETag`, or else `Last-Modified`.
fn strong<B>(response: &ureq::http::Response<B>) -> Option<String> {
    header(response, "etag").filter(|etag| !etag.starts_with("W/")).or_else(|| header(response, "last-modified"))
}

fn header<B>(response: &ureq::http::Response<B>, name: &str) -> Option<String> {
    response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
}
