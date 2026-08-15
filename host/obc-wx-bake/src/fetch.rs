//! The upstream HTTP seam: conditional bounded fetches with bounded retry, behind a trait so a
//! cycle is drivable byte-for-byte from checked-in fixtures.
//!
//! WX1's failure rules, verbatim: retry timeouts and 5xx with bounded backoff and honor
//! `Retry-After`; a 4xx, oversize body or schema surprise is a contract failure, never retried
//! into "successful weather". Every fetch is size-capped **before** the body is read.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

/// One retrieved upstream object plus the validators a later cycle short-circuits on.
#[derive(Debug, Clone)]
pub struct Fetched {
    pub bytes: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// What a conditional fetch concluded.
#[derive(Debug, Clone)]
pub enum FetchOutcome {
    /// The upstream object changed (or no validator was offered): fresh bytes.
    Body(Fetched),
    /// `304 Not Modified` against the supplied validator: the previous bake still stands.
    Unchanged,
}

pub trait Upstream {
    /// GET `url` with a hard byte cap. `if_none_match` enables the unchanged-upstream
    /// short-circuit. Implementations must count fetched bytes into their ledger.
    fn fetch(&mut self, url: &str, cap: u64, if_none_match: Option<&str>) -> Result<FetchOutcome, String>;

    /// HEAD `url`: `Ok(Some(length))` if it exists, `Ok(None)` on 404. Used for run discovery and
    /// for the object length a `.idx` byte-range selection needs to bound its final record.
    fn content_length(&mut self, url: &str) -> Result<Option<u64>, String>;

    /// GET the inclusive byte range `[start, end]` of `url` — the `.idx` fast path that fetches
    /// one GRIB message out of a multi-gigabyte NOAA object. Implementations MUST prove the
    /// server honored the range (206 plus a matching length) rather than accepting a whole body.
    fn fetch_range(&mut self, url: &str, start: u64, end_inclusive: u64, cap: u64) -> Result<Fetched, String>;

    /// Total upstream bytes fetched so far (bodies only), for the cycle report.
    fn fetched_bytes(&self) -> u64;

    /// HEAD `url`: `Ok(true)` if it exists, `Ok(false)` on 404 — used for run discovery only.
    fn exists(&mut self, url: &str) -> Result<bool, String> {
        Ok(self.content_length(url)?.is_some())
    }
}

/// The production client: blocking rustls `ureq`, bounded exponential backoff.
pub struct HttpUpstream {
    agent: ureq::Agent,
    attempts: u32,
    fetched: u64,
}

const USER_AGENT: &str = "obc-wx-bake/0.1 https://github.com/timohueser/OpenBikeComputer";

impl HttpUpstream {
    pub fn new() -> Self {
        let config =
            ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(120))).user_agent(USER_AGENT).build();
        Self { agent: config.into(), attempts: 3, fetched: 0 }
    }

    /// Retry only what WX1 allows: transport errors and 5xx/429, with `Retry-After` honored up
    /// to a sane ceiling and exponential backoff otherwise.
    fn with_retries<T>(
        &mut self,
        url: &str,
        mut once: impl FnMut(&mut Self) -> Result<T, RetryClass>,
    ) -> Result<T, String> {
        let mut delay = Duration::from_secs(1);
        for attempt in 1..=self.attempts {
            match once(self) {
                Ok(value) => return Ok(value),
                Err(RetryClass::Fatal(message)) => return Err(format!("{url}: {message}")),
                Err(RetryClass::Retryable(message, retry_after)) => {
                    if attempt == self.attempts {
                        return Err(format!("{url}: {message} (after {attempt} attempts)"));
                    }
                    let wait = retry_after.unwrap_or(delay).min(Duration::from_secs(60));
                    std::thread::sleep(wait);
                    delay *= 4;
                }
            }
        }
        unreachable!("the retry loop always returns");
    }
}

enum RetryClass {
    Retryable(String, Option<Duration>),
    Fatal(String),
}

fn retry_after(headers: &ureq::http::response::Parts) -> Option<Duration> {
    headers
        .headers
        .get("retry-after")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

impl Upstream for HttpUpstream {
    fn fetch(&mut self, url: &str, cap: u64, if_none_match: Option<&str>) -> Result<FetchOutcome, String> {
        let outcome = self.with_retries(url, |this| {
            let mut request = this.agent.get(url);
            if let Some(etag) = if_none_match {
                request = request.header("If-None-Match", etag);
            }
            let response = match request.call() {
                Ok(response) => response,
                Err(ureq::Error::StatusCode(304)) => return Ok(FetchOutcome::Unchanged),
                Err(ureq::Error::StatusCode(code)) if code == 429 || code >= 500 => {
                    return Err(RetryClass::Retryable(format!("upstream status {code}"), None));
                }
                Err(ureq::Error::StatusCode(code)) => {
                    return Err(RetryClass::Fatal(format!("upstream status {code}")));
                }
                Err(error) => return Err(RetryClass::Retryable(format!("transport: {error}"), None)),
            };
            let (parts, body) = response.into_parts();
            if parts.status.as_u16() == 304 {
                return Ok(FetchOutcome::Unchanged);
            }
            if parts.status.as_u16() == 429 || parts.status.as_u16() >= 500 {
                return Err(RetryClass::Retryable(format!("upstream status {}", parts.status), retry_after(&parts)));
            }
            if parts.status.as_u16() != 200 {
                return Err(RetryClass::Fatal(format!("upstream status {}", parts.status)));
            }
            if let Some(length) = parts.headers.get("content-length").and_then(|v| v.to_str().ok()) {
                if length.trim().parse::<u64>().is_ok_and(|length| length > cap) {
                    return Err(RetryClass::Fatal(format!("announced {length} bytes exceeds the {cap}-byte cap")));
                }
            }
            let header = |name: &str| {
                parts.headers.get(name).and_then(|value| value.to_str().ok()).map(|value| value.to_string())
            };
            let mut bytes = Vec::new();
            let mut reader = body.into_reader().take(cap + 1);
            if let Err(error) = reader.read_to_end(&mut bytes) {
                return Err(RetryClass::Retryable(format!("body read: {error}"), None));
            }
            if bytes.len() as u64 > cap {
                return Err(RetryClass::Fatal(format!("body exceeds the {cap}-byte cap")));
            }
            Ok(FetchOutcome::Body(Fetched { bytes, etag: header("etag"), last_modified: header("last-modified") }))
        })?;
        if let FetchOutcome::Body(fetched) = &outcome {
            self.fetched += fetched.bytes.len() as u64;
        }
        Ok(outcome)
    }

    fn content_length(&mut self, url: &str) -> Result<Option<u64>, String> {
        self.with_retries(url, |this| match this.agent.head(url).call() {
            Ok(response) if response.status().as_u16() == 200 => {
                let length = response
                    .headers()
                    .get("content-length")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.trim().parse::<u64>().ok());
                match length {
                    Some(length) => Ok(Some(length)),
                    None => Err(RetryClass::Fatal("HEAD announced no usable Content-Length".into())),
                }
            }
            Ok(response) => Err(RetryClass::Fatal(format!("upstream status {}", response.status()))),
            Err(ureq::Error::StatusCode(404)) => Ok(None),
            Err(ureq::Error::StatusCode(code)) if code == 429 || code >= 500 => {
                Err(RetryClass::Retryable(format!("upstream status {code}"), None))
            }
            Err(ureq::Error::StatusCode(code)) => Err(RetryClass::Fatal(format!("upstream status {code}"))),
            Err(error) => Err(RetryClass::Retryable(format!("transport: {error}"), None)),
        })
    }

    fn fetch_range(&mut self, url: &str, start: u64, end_inclusive: u64, cap: u64) -> Result<Fetched, String> {
        if end_inclusive < start {
            return Err(format!("{url}: empty byte range {start}-{end_inclusive}"));
        }
        let wanted = end_inclusive - start + 1;
        if wanted > cap {
            return Err(format!("{url}: range of {wanted} bytes exceeds the {cap}-byte cap"));
        }
        let fetched = self.with_retries(url, |this| {
            let response = match this.agent.get(url).header("Range", &format!("bytes={start}-{end_inclusive}")).call() {
                Ok(response) => response,
                Err(ureq::Error::StatusCode(code)) if code == 429 || code >= 500 => {
                    return Err(RetryClass::Retryable(format!("upstream status {code}"), None));
                }
                Err(ureq::Error::StatusCode(code)) => {
                    return Err(RetryClass::Fatal(format!("upstream status {code}")));
                }
                Err(error) => return Err(RetryClass::Retryable(format!("transport: {error}"), None)),
            };
            let (parts, body) = response.into_parts();
            // A 200 here means the server ignored the range and is about to stream the whole
            // multi-hundred-megabyte object: a contract failure, never a slow success.
            if parts.status.as_u16() != 206 {
                return Err(RetryClass::Fatal(format!("range request answered with status {}", parts.status)));
            }
            let content_range = parts
                .headers
                .get("content-range")
                .and_then(|value| value.to_str().ok())
                .map(str::trim)
                .unwrap_or_default()
                .to_string();
            let expected_prefix = format!("bytes {start}-{end_inclusive}/");
            if !content_range.starts_with(&expected_prefix) {
                return Err(RetryClass::Fatal(format!("Content-Range {content_range:?} is not the requested range")));
            }
            let header = |name: &str| {
                parts.headers.get(name).and_then(|value| value.to_str().ok()).map(|value| value.to_string())
            };
            let mut bytes = Vec::new();
            let mut reader = body.into_reader().take(wanted + 1);
            if let Err(error) = reader.read_to_end(&mut bytes) {
                return Err(RetryClass::Retryable(format!("body read: {error}"), None));
            }
            if bytes.len() as u64 != wanted {
                return Err(RetryClass::Fatal(format!("range returned {} bytes, expected {wanted}", bytes.len())));
            }
            Ok(Fetched { bytes, etag: header("etag"), last_modified: header("last-modified") })
        })?;
        self.fetched += fetched.bytes.len() as u64;
        Ok(fetched)
    }

    fn fetched_bytes(&self) -> u64 {
        self.fetched
    }
}

impl Default for HttpUpstream {
    fn default() -> Self {
        Self::new()
    }
}

/// A deterministic upstream serving checked-in fixture bytes: the cycle tests' whole network.
/// Unknown URLs are 404 for `exists` and a loud error for `fetch`, so a test also proves the
/// baker asks only for what its contract names.
#[derive(Default)]
pub struct FixtureUpstream {
    objects: BTreeMap<String, Fetched>,
    /// Exact byte ranges of objects too large to check in whole (the NOAA `.idx` fast path):
    /// `(url, start, end_inclusive)` → the captured message bytes.
    ranges: BTreeMap<(String, u64, u64), Vec<u8>>,
    /// Declared `Content-Length` of range-served objects, so an adapter's range arithmetic is
    /// driven by the real upstream object length.
    lengths: BTreeMap<String, u64>,
    fetched: u64,
    /// Every URL fetched or probed, in order — request accounting for the cycle tests.
    pub requests: Vec<String>,
}

impl FixtureUpstream {
    pub fn insert(&mut self, url: impl Into<String>, bytes: Vec<u8>, etag: Option<&str>) {
        let url = url.into();
        self.lengths.insert(url.clone(), bytes.len() as u64);
        self.objects.insert(url, Fetched { bytes, etag: etag.map(str::to_string), last_modified: None });
    }

    /// Declare that an object exists with a given upstream length, without checking in a body:
    /// enough for discovery probes and for the range arithmetic of an object far too large to
    /// store (a 500 MB GFS or 200 MB HRRR file).
    pub fn declare(&mut self, url: impl Into<String>, object_len: u64) {
        self.lengths.insert(url.into(), object_len);
    }

    /// Declare a range-served object: its real upstream length plus the one captured range. A
    /// request for any other range (or for the whole body) then fails loudly, which is exactly
    /// the request-accounting property the byte-range adapters must prove.
    pub fn insert_range(&mut self, url: impl Into<String>, object_len: u64, start: u64, bytes: Vec<u8>) {
        let url = url.into();
        let end = start + bytes.len() as u64 - 1;
        self.declare(url.clone(), object_len);
        self.ranges.insert((url, start, end), bytes);
    }
}

impl Upstream for FixtureUpstream {
    fn fetch(&mut self, url: &str, cap: u64, if_none_match: Option<&str>) -> Result<FetchOutcome, String> {
        self.requests.push(url.to_string());
        let object = self.objects.get(url).ok_or_else(|| format!("{url}: no fixture"))?;
        if object.bytes.len() as u64 > cap {
            return Err(format!("{url}: fixture exceeds the {cap}-byte cap"));
        }
        if let (Some(offered), Some(stored)) = (if_none_match, object.etag.as_deref()) {
            if offered == stored {
                return Ok(FetchOutcome::Unchanged);
            }
        }
        self.fetched += object.bytes.len() as u64;
        Ok(FetchOutcome::Body(object.clone()))
    }

    fn content_length(&mut self, url: &str) -> Result<Option<u64>, String> {
        self.requests.push(format!("HEAD {url}"));
        Ok(self.lengths.get(url).copied())
    }

    fn fetch_range(&mut self, url: &str, start: u64, end_inclusive: u64, cap: u64) -> Result<Fetched, String> {
        self.requests.push(format!("{url}#{start}-{end_inclusive}"));
        if end_inclusive < start || end_inclusive - start + 1 > cap {
            return Err(format!("{url}: range {start}-{end_inclusive} is empty or exceeds the {cap}-byte cap"));
        }
        let bytes = self
            .ranges
            .get(&(url.to_string(), start, end_inclusive))
            .ok_or_else(|| format!("{url}: no fixture for byte range {start}-{end_inclusive}"))?
            .clone();
        self.fetched += bytes.len() as u64;
        Ok(Fetched { bytes, etag: None, last_modified: None })
    }

    fn fetched_bytes(&self) -> u64 {
        self.fetched
    }
}
