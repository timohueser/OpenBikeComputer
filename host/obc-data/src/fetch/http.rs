//! One file over HTTP into the store: retries, a `.part` file that the next attempt resumes with a
//! `Range` request, and a digest check before the file becomes an object.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    pub etag: Option<&'a str>,
    pub bytes: Option<u64>,
}

#[derive(Debug)]
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

/// Whether an error of this module is an HTTP 404: upstream has no such file. Every status error
/// here ends with `HTTP <status>`.
pub fn not_found(error: &str) -> bool {
    error.ends_with("HTTP 404")
}

/// Whether an error of [`Http::modified`] or [`Http::text`] says that the connection failed or
/// timed out: upstream could not be asked.
pub fn unreachable(error: &str) -> bool {
    error.contains(UNREACHABLE)
}

const UNREACHABLE: &str = ": unreachable: ";

/// Set when the run of this process stops: a download ends within one read and keeps its part.
static STOPPED: AtomicBool = AtomicBool::new(false);

pub fn stop() {
    STOPPED.store(true, Ordering::Relaxed);
}

fn stopped(url: &str) -> Result<(), Failure> {
    match STOPPED.load(Ordering::Relaxed) {
        true => Err(Failure::Final(format!("GET {url}: the run stopped; the next fetch resumes the download"))),
        false => Ok(()),
    }
}

/// The error of a request that got no answer, marked when no connection was made or it timed out.
fn no_answer(method: &str, url: &str, error: ureq::Error) -> String {
    use ureq::Error::{ConnectProxyFailed, ConnectionFailed, HostNotFound, Io, Timeout};
    match error {
        Io(_) | Timeout(_) | HostNotFound | ConnectionFailed | ConnectProxyFailed(_) => {
            format!("{method} {url}{UNREACHABLE}{error}")
        }
        error => format!("{method} {url}: {error}"),
    }
}

pub struct Http {
    agent: ureq::Agent,
    bearer: Option<(String, String)>,
    backoff: Duration,
    bytes_per_second: Option<u64>,
    requests: AtomicU64,
    transferred: AtomicU64,
}

enum Failure {
    Retry(String),
    Wait(String, Duration),
    /// Retry, after a try that added bytes to a resumed part.
    Grew(String),
    Final(String),
}

impl Http {
    /// A client that sends each request for the HTTPS form of `actual` to `actual`, a test server.
    #[cfg(test)]
    pub(crate) fn loopback(actual: &str) -> Self {
        assert!(actual.starts_with("http://127.0.0.1:"));
        let declared: ureq::http::Uri = actual.replacen("http://", "https://", 1).parse().unwrap();
        let actual: ureq::http::Uri = actual.parse().unwrap();
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .middleware(
                move |mut request: ureq::http::Request<ureq::SendBody<'_>>,
                      next: ureq::middleware::MiddlewareNext<'_>| {
                    assert_eq!(request.uri(), &declared, "the fixture cannot contact another endpoint");
                    *request.uri_mut() = actual.clone();
                    next.handle(request)
                },
            )
            .build();
        Self {
            agent: config.into(),
            bearer: None,
            backoff: Duration::ZERO,
            bytes_per_second: None,
            requests: AtomicU64::new(0),
            transferred: AtomicU64::new(0),
        }
    }

    pub(super) fn metrics(&self) -> (u64, u64) {
        (self.requests.load(Ordering::Relaxed), self.transferred.load(Ordering::Relaxed))
    }

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
        Self {
            agent: config.into(),
            bearer: None,
            backoff,
            bytes_per_second: None,
            requests: AtomicU64::new(0),
            transferred: AtomicU64::new(0),
        }
    }

    /// Authorize only requests to this HTTPS origin. Redirects strip authorization by default.
    pub fn with_bearer(mut self, origin: &str, token: String) -> Self {
        self.bearer = Some((format!("{}/", origin.trim_end_matches('/')), token));
        self
    }

    fn authorize(
        &self,
        url: &str,
        request: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    ) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        match &self.bearer {
            Some((origin, token)) if url.starts_with(origin) => {
                request.header("authorization", format!("Bearer {token}"))
            }
            _ => request,
        }
    }

    /// Size and strong object identity from headers only.
    pub fn headers(&self, url: &str) -> Result<(u64, Option<String>), String> {
        let response = self
            .authorize(url, self.agent.head(url))
            .config()
            .timeout_global(Some(SMALL))
            .build()
            .call()
            .map_err(|e| no_answer("HEAD", url, e))?;
        if !response.status().is_success() {
            return Err(format!("HEAD {url}: HTTP {}", response.status().as_u16()));
        }
        let bytes = header(&response, "content-length")
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| format!("HEAD {url}: no Content-Length"))?;
        let etag = header(&response, "etag").filter(|v| !v.starts_with("W/"));
        Ok((bytes, etag))
    }

    /// A small JSON POST. Errors omit the response body, which can contain credentials.
    pub fn post_json(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, String> {
        let mut response = self
            .agent
            .post(url)
            .header("content-type", "application/json")
            .config()
            .timeout_global(Some(SMALL))
            .max_redirects(0)
            .build()
            .send(body)
            .map_err(|_| format!("POST {url}: request failed"))?;
        if !response.status().is_success() {
            return Err(format!("POST {url}: HTTP {}", response.status().as_u16()));
        }
        response.body_mut().read_to_vec().map_err(|_| format!("POST {url}: invalid response"))
    }

    /// Bound the transfer rate of this serial downloader.
    pub fn limited(mut self, bytes_per_second: u64) -> Self {
        self.bytes_per_second = Some(bytes_per_second);
        self
    }

    /// The `Last-Modified` day of `url`, after redirects. A 404 is an error that [`not_found`] knows.
    pub fn modified(&self, url: &str) -> Result<Option<String>, String> {
        let request = self.agent.head(url).config().timeout_global(Some(SMALL)).build();
        let response = request.call().map_err(|e| no_answer("HEAD", url, e))?;
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
        String::from_utf8(self.bytes(url, accept)?).map_err(|e| format!("GET {url}: {e}"))
    }

    /// A small metadata file, with its exact bytes.
    pub fn bytes(&self, url: &str, accept: &str) -> Result<Vec<u8>, String> {
        match self.answer(url, accept)? {
            (200..=299, body) => Ok(body),
            (status, _) => Err(format!("GET {url}: HTTP {status}")),
        }
    }

    /// The status and the body of a small answer, also of one that is no success.
    pub fn answer(&self, url: &str, accept: &str) -> Result<(u16, Vec<u8>), String> {
        let request = self
            .authorize(url, self.agent.get(url))
            .header("accept", accept)
            .config()
            .timeout_global(Some(SMALL))
            .build();
        let mut response = request.call().map_err(|e| no_answer("GET", url, e))?;
        let status = response.status().as_u16();
        match response.body_mut().read_to_vec() {
            Ok(body) => Ok((status, body)),
            // The status of an answer that is no success says enough.
            Err(_) if !(200..=299).contains(&status) => Ok((status, Vec::new())),
            Err(e) => Err(format!("GET {url}: {e}")),
        }
    }

    /// The lock that a download of `url` needs. One process at a time downloads a URL.
    pub fn lock(store: &Store, url: &str) -> Result<Lock, String> {
        store.lock(&format!("download-{}", key(url)))
    }

    /// A revision bracket cannot prove bytes read before its first witness.
    pub(super) fn fresh_download(&self, store: &Store, url: &str) -> Result<Downloaded, String> {
        for suffix in ["part", "validator"] {
            match fs::remove_file(store.partial(&format!("{}.{suffix}", key(url)))) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.to_string()),
            }
        }
        self.download(store, url, &Expect::default())
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
        let (sha256, size) = self.transfer(url, &part, &validator, expect)?;
        let object = store.insert(&part, &sha256)?;
        Ok(Downloaded { object, sha256, size })
    }

    /// Download an archive in place. Keep its partial file across stopped runs.
    pub fn download_to(&self, path: &Path, url: &str, expect: &Expect) -> Result<Downloaded, String> {
        let parent = path.parent().ok_or("archive has no parent directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path.with_extension("lock"))
            .map_err(|e| e.to_string())?;
        lock.lock().map_err(|e| e.to_string())?;
        if path.is_file() {
            let (sha256, size) = store::hash_file(path)?;
            if expect.bytes.is_some_and(|want| want != size) || expect.sha256.is_some_and(|want| want != sha256) {
                return Err("cached archive differs from the expected size or SHA-256".into());
            }
            return Ok(Downloaded { object: path.into(), sha256, size });
        }
        let part = path.with_extension("part");
        let validator = path.with_extension("validator");
        let have = fs::metadata(&part).map_or(0, |m| m.len());
        store::check_free(parent, expect.bytes.unwrap_or(0).saturating_sub(have))?;
        let (sha256, size) = self.transfer(url, &part, &validator, expect)?;
        fs::rename(&part, path).map_err(|e| e.to_string())?;
        Ok(Downloaded { object: path.into(), sha256, size })
    }

    fn transfer(&self, url: &str, part: &Path, validator: &Path, expect: &Expect) -> Result<(String, u64), String> {
        let partial = part.parent().ok_or("download path has no parent")?;
        fs::create_dir_all(partial).map_err(|e| format!("{}: {e}", partial.display()))?;
        let mut failures = 0;
        loop {
            let (why, grew, delay) = match self.attempt(url, part, validator, expect) {
                Ok(()) => break,
                Err(Failure::Final(why)) => return Err(why),
                Err(Failure::Retry(why)) => (why, false, Duration::ZERO),
                Err(Failure::Wait(why, delay)) => (why, false, delay),
                Err(Failure::Grew(why)) => (why, true, Duration::ZERO),
            };
            failures = if grew { 1 } else { failures + 1 };
            if failures == ATTEMPTS {
                return Err(why);
            }
            eprintln!("obc data: {why} — retrying");
            std::thread::sleep(delay.max(self.backoff * 2u32.pow(failures - 1)));
        }
        let _ = fs::remove_file(validator);
        let (sha256, size) = store::hash_file(part)?;
        if let Some(want) = expect.sha256.filter(|want| *want != sha256) {
            let _ = fs::remove_file(part);
            return Err(format!("{url}: the SHA-256 is {sha256}, not {want}"));
        }
        if expect.bytes.is_some_and(|want| want != size) {
            let _ = fs::remove_file(part);
            return Err(format!("{url}: downloaded size differs from the reviewed input"));
        }
        Ok((sha256, size))
    }

    fn attempt(&self, url: &str, part: &Path, validator: &Path, expect: &Expect) -> Result<(), Failure> {
        stopped(url)?;
        let have = fs::metadata(part).map_or(0, |metadata| metadata.len());
        let saved = fs::read_to_string(validator).ok();
        // Without a validator or a digest, nothing would tell a changed file from the rest of the old one.
        let resume = have > 0
            && (saved.is_some() || expect.sha256.is_some())
            && expect.etag.is_none_or(|want| saved.as_deref() == Some(want));
        // The bytes as upstream stores them: a decoded body has no ranges and another digest.
        let mut request = self.authorize(url, self.agent.get(url)).header("accept-encoding", "identity");
        if let Some(etag) = expect.etag {
            request = request.header("if-match", etag);
        }
        if resume {
            request = request.header("range", format!("bytes={have}-"));
            if let Some(saved) = &saved {
                request = request.header("if-range", saved);
            }
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
        let mut response = request.call().map_err(|e| Failure::Retry(format!("GET {url}: {e}")))?;
        let status = response.status().as_u16();
        if response.status().is_success()
            && expect.etag.is_some_and(|want| header(&response, "etag").as_deref() != Some(want))
        {
            return Err(Failure::Final(format!("{url}: upstream object changed since review; review again")));
        }
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
            408 | 429 | 500..=599 => {
                let why = format!("GET {url}: HTTP {status}");
                return Err(match header(&response, "retry-after").and_then(|value| retry_after(&value)) {
                    Some(delay) => Failure::Wait(why, delay),
                    None => Failure::Retry(why),
                });
            }
            _ => return Err(Failure::Final(format!("GET {url}: HTTP {status}"))),
        };
        if expect.bytes.is_some_and(|want| total.is_some_and(|total| total != want)) {
            return Err(Failure::Final(format!("{url}: upstream size changed since review")));
        }
        let needed = total.map_or(0, |total| total.saturating_sub(if append { have } else { 0 }));
        store::check_free(part, needed).map_err(Failure::Final)?;
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
        let started = std::time::Instant::now();
        let mut received = 0u64;
        let mut reported = started;
        let retry = |grew: bool, why: String| if append && grew { Failure::Grew(why) } else { Failure::Retry(why) };
        loop {
            stopped(url)?;
            let read = reader.read(&mut buffer).map_err(|e| retry(grew, format!("GET {url}: {e}")))?;
            if read == 0 {
                break;
            }
            let current = received + read as u64 + if append { have } else { 0 };
            if expect.bytes.is_some_and(|want| current > want) {
                return Err(Failure::Final(format!("{url}: download exceeds its reviewed size")));
            }
            if reported.elapsed() >= Duration::from_secs(30) {
                let done = crate::engine::runs::bytes(current);
                match expect.bytes.or(total) {
                    Some(whole) => {
                        eprintln!("obc data: {url}: {done} / {} downloaded", crate::engine::runs::bytes(whole))
                    }
                    None => eprintln!("obc data: {url}: {done} downloaded"),
                }
                reported = std::time::Instant::now();
            }
            file.write_all(&buffer[..read]).map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
            grew = true;
            self.transferred.fetch_add(read as u64, Ordering::Relaxed);
            received += read as u64;
            if let Some(rate) = self.bytes_per_second {
                let required = Duration::from_secs_f64(received as f64 / rate.max(1) as f64);
                if let Some(delay) = required.checked_sub(started.elapsed()) {
                    std::thread::sleep(delay);
                }
            }
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

fn retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse() {
        return Some(Duration::from_secs(seconds));
    }
    let day = date::from_http(value)?;
    let time = value.split_whitespace().nth(4)?;
    let seconds = date::seconds(&format!("{day}T{time}Z"))?;
    Some(Duration::from_secs(seconds.saturating_sub(date::now())))
}

/// The name of the files that a download of `url` keeps in `partial/`.
pub(crate) fn key(url: &str) -> String {
    store::sha256_hex(url.as_bytes())[..32].to_string()
}

/// A validator that can guard a range: a strong `ETag`, or else `Last-Modified`.
fn strong<B>(response: &ureq::http::Response<B>) -> Option<String> {
    header(response, "etag").filter(|etag| !etag.starts_with("W/")).or_else(|| header(response, "last-modified"))
}

fn header<B>(response: &ureq::http::Response<B>, name: &str) -> Option<String> {
    response.headers().get(name).and_then(|value| value.to_str().ok()).map(str::to_string)
}
