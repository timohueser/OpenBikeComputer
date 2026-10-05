//! One file over HTTP into the store: retries, a `.part` file that the next attempt resumes with a
//! `Range` request, and a digest check before the file becomes an object.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ureq::ResponseExt;

use crate::date;
use crate::store::{self, Store};

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
    /// The `Last-Modified` day of the response, or today when it has none.
    pub modified: String,
}

/// The answer to a HEAD request, after redirects.
pub struct Head {
    pub url: String,
    pub modified: Option<String>,
}

/// Tries of one download: the first, and three retries.
const ATTEMPTS: u32 = 4;

pub struct Http {
    agent: ureq::Agent,
    backoff: Duration,
}

enum Failure {
    Retry(String),
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
            .build();
        Self { agent: config.into(), backoff }
    }

    pub fn head(&self, url: &str) -> Result<Head, String> {
        let response = self.agent.head(url).call().map_err(|e| format!("HEAD {url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("HEAD {url}: HTTP {}", response.status().as_u16()));
        }
        let modified = header(&response, "last-modified").and_then(|value| date::from_http(&value));
        Ok(Head { url: response.get_uri().to_string(), modified })
    }

    /// A small document, such as an index or an API answer.
    pub fn text(&self, url: &str, accept: &str) -> Result<String, String> {
        let mut response =
            self.agent.get(url).header("accept", accept).call().map_err(|e| format!("GET {url}: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("GET {url}: HTTP {}", response.status().as_u16()));
        }
        response.body_mut().read_to_string().map_err(|e| format!("GET {url}: {e}"))
    }

    /// Download `url` into the store. A failed attempt keeps its `.part` file, so the next attempt,
    /// or the next run, asks only for the rest. A digest mismatch removes it.
    pub fn download(&self, store: &Store, url: &str, expect: &Expect) -> Result<Downloaded, String> {
        let key = &store::sha256_hex(url.as_bytes())[..32];
        let _lock = store.lock(&format!("download-{key}"))?;
        let part = store.partial(&format!("{key}.part"));
        let validator = store.partial(&format!("{key}.validator"));
        let mut modified = Err(String::new());
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                eprintln!("obc data: {} — retrying", modified.as_ref().unwrap_err());
                std::thread::sleep(self.backoff * 2u32.pow(attempt - 1));
            }
            match self.attempt(url, &part, &validator, expect) {
                Ok(day) => {
                    modified = Ok(day);
                    break;
                }
                Err(Failure::Retry(why)) => modified = Err(why),
                Err(Failure::Final(why)) => return Err(why),
            }
        }
        let modified = modified?;
        let _ = fs::remove_file(&validator);
        let (sha256, size) = store::hash_file(&part)?;
        if let Some(want) = expect.sha256.filter(|want| *want != sha256) {
            let _ = fs::remove_file(&part);
            return Err(format!("{url}: the SHA-256 is {sha256}, not {want}"));
        }
        let object = store.insert(&part, &sha256)?;
        Ok(Downloaded { object, sha256, size, modified })
    }

    fn attempt(&self, url: &str, part: &Path, validator: &Path, expect: &Expect) -> Result<String, Failure> {
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
        let modified = header(&response, "last-modified")
            .and_then(|value| date::from_http(&value))
            .unwrap_or_else(|| date::format(date::today()));
        let (append, total): (bool, Option<u64>) = match status {
            206 if resume => {
                let range = header(&response, "content-range").unwrap_or_default();
                let (start, total) = range.strip_prefix("bytes ").and_then(|r| r.split_once('-')).unzip();
                if start != Some(have.to_string().as_str()) {
                    let _ = fs::remove_file(part);
                    return Err(Failure::Retry(format!("GET {url}: answered `{range}` to a resume at {have}")));
                }
                (true, total.and_then(|t| t.split_once('/')).and_then(|(_, t)| t.parse().ok()))
            }
            200 => {
                let content_length = header(&response, "content-length").and_then(|v| v.parse().ok());
                (false, content_length)
            }
            416 => {
                let _ = fs::remove_file(part);
                return Err(Failure::Retry(format!("GET {url}: HTTP 416 to a resume at {have}")));
            }
            408 | 429 | 500..=599 => return Err(Failure::Retry(format!("GET {url}: HTTP {status}"))),
            _ => return Err(Failure::Final(format!("GET {url}: HTTP {status}"))),
        };
        if let Some(by) = expect.modified_by.filter(|by| modified.as_str() > *by) {
            return Err(Failure::Final(format!("{url} changed on {modified}, after the version {by}")));
        }
        if !append {
            match header(&response, "etag").or_else(|| header(&response, "last-modified")) {
                Some(value) => store::write_atomic(validator, value.as_bytes()).map_err(Failure::Final)?,
                None => drop(fs::remove_file(validator)),
            }
        }
        let file = OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(part);
        let mut file = file.map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
        let mut reader = response.body_mut().as_reader();
        let mut buffer = vec![0; 1 << 16];
        loop {
            let read = reader.read(&mut buffer).map_err(|e| Failure::Retry(format!("GET {url}: {e}")))?;
            if read == 0 {
                break;
            }
            file.write_all(&buffer[..read]).map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
        }
        file.sync_all().map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
        let length = fs::metadata(part).map_or(0, |metadata| metadata.len());
        match total {
            Some(total) if total != length => {
                Err(Failure::Retry(format!("GET {url}: the connection closed at {length} of {total} bytes")))
            }
            _ => Ok(modified),
        }
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

fn header<B>(response: &ureq::http::Response<B>, name: &str) -> Option<String> {
    response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
}
