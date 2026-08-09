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

    /// HEAD `url`: `Ok(true)` if it exists, `Ok(false)` on 404 — used for run discovery only.
    fn exists(&mut self, url: &str) -> Result<bool, String>;

    /// Total upstream bytes fetched so far (bodies only), for the cycle report.
    fn fetched_bytes(&self) -> u64;
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
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(120)))
            .user_agent(USER_AGENT)
            .build();
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
            Ok(FetchOutcome::Body(Fetched {
                bytes,
                etag: header("etag"),
                last_modified: header("last-modified"),
            }))
        })?;
        if let FetchOutcome::Body(fetched) = &outcome {
            self.fetched += fetched.bytes.len() as u64;
        }
        Ok(outcome)
    }

    fn exists(&mut self, url: &str) -> Result<bool, String> {
        self.with_retries(url, |this| match this.agent.head(url).call() {
            Ok(response) if response.status().as_u16() == 200 => Ok(true),
            Ok(response) => Err(RetryClass::Fatal(format!("upstream status {}", response.status()))),
            Err(ureq::Error::StatusCode(404)) => Ok(false),
            Err(ureq::Error::StatusCode(code)) if code == 429 || code >= 500 => {
                Err(RetryClass::Retryable(format!("upstream status {code}"), None))
            }
            Err(ureq::Error::StatusCode(code)) => Err(RetryClass::Fatal(format!("upstream status {code}"))),
            Err(error) => Err(RetryClass::Retryable(format!("transport: {error}"), None)),
        })
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
    fetched: u64,
    /// Every URL fetched or probed, in order — request accounting for the cycle tests.
    pub requests: Vec<String>,
}

impl FixtureUpstream {
    pub fn insert(&mut self, url: impl Into<String>, bytes: Vec<u8>, etag: Option<&str>) {
        self.objects
            .insert(url.into(), Fetched { bytes, etag: etag.map(str::to_string), last_modified: None });
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

    fn exists(&mut self, url: &str) -> Result<bool, String> {
        self.requests.push(format!("HEAD {url}"));
        Ok(self.objects.contains_key(url))
    }

    fn fetched_bytes(&self) -> u64 {
        self.fetched
    }
}
