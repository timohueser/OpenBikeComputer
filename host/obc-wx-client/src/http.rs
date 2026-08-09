//! The client's HTTP seam.
//!
//! One trait, three implementations: the real blocking `ureq` client, a fixture client that
//! answers from checked-in bytes (CI never touches the network), and a wrapper that injects the
//! WX14 failure controls — latency, HTTP status, truncation, corruption, offline. Everything the
//! client does above this seam is byte-identical whichever one is underneath, which is the whole
//! point: the failure fixtures exercise the *production* decode/validate path.

use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

/// One HTTP answer, reduced to what a weather client needs.
#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub expires: Option<String>,
    pub retry_after: Option<String>,
}

impl Response {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn is_not_modified(&self) -> bool {
        self.status == 304
    }
}

/// A conditional-GET request. `range` is inclusive-inclusive, exactly as the `Range` header
/// spells it.
#[derive(Debug, Clone, Default)]
pub struct Request {
    pub url: String,
    pub range: Option<(u64, u64)>,
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
}

impl Request {
    pub fn get(url: impl Into<String>) -> Self {
        Self { url: url.into(), ..Self::default() }
    }

    pub fn range(url: impl Into<String>, start: u64, end_inclusive: u64) -> Self {
        Self { url: url.into(), range: Some((start, end_inclusive)), ..Self::default() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// No answer at all: DNS, TLS, timeout, airplane mode. The client's `offline` control lands
    /// here too, so "offline" is not a second code path.
    Transport(String),
    /// An answer the client refuses to treat as weather.
    Status { code: u16, retry_after: Option<String> },
    /// A body that exceeded the caller's cap before it was read.
    TooLarge { cap: u64 },
    /// A `Range` request the server did not honor.
    RangeNotHonoured(String),
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HttpError::Transport(message) => write!(f, "transport: {message}"),
            HttpError::Status { code, .. } => write!(f, "status {code}"),
            HttpError::TooLarge { cap } => write!(f, "body exceeds the {cap}-byte cap"),
            HttpError::RangeNotHonoured(message) => write!(f, "range not honoured: {message}"),
        }
    }
}

/// Everything the client fetches goes through here.
pub trait Http {
    fn perform(&mut self, request: &Request, cap: u64) -> Result<Response, HttpError>;

    /// Requests issued so far, for the diagnostics panel. Counters, never control flow.
    fn requests(&self) -> u32 {
        0
    }

    /// Body bytes moved so far.
    fn bytes(&self) -> u64 {
        0
    }
}

/// Per-call byte caps. A weather client that streams an unbounded body is a bug, not a slow day.
pub const MANIFEST_CAP: u64 = 4 * 1024 * 1024;
pub const MET_CAP: u64 = 4 * 1024 * 1024;
/// One corridor read. Generous next to the ~6 KiB directory pages and ~512 B tiles the launch
/// products emit, tight enough that a mis-planned range can never pull a whole 100 KB frame.
pub const RANGE_CAP: u64 = 512 * 1024;

// ── the real client ────────────────────────────────────────────────────────────────────────

/// The identifying `User-Agent`. MET's terms require an app-identifying agent with a contact
/// address, and the OBC service wants its own traffic distinguishable from the phone's — so the
/// simulator says *simulator*. The shape mirrors the iOS agent
/// (`OpenBikeComputer/<version> github.com/timohueser/OpenBikeComputer`).
pub const USER_AGENT: &str =
    concat!("OpenBikeComputer-sim/", env!("CARGO_PKG_VERSION"), " github.com/timohueser/OpenBikeComputer");

pub struct UreqHttp {
    agent: ureq::Agent,
    requests: u32,
    bytes: u64,
}

impl Default for UreqHttp {
    fn default() -> Self {
        Self::new()
    }
}

impl UreqHttp {
    pub fn new() -> Self {
        let config =
            ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(30))).user_agent(USER_AGENT).build();
        Self { agent: config.into(), requests: 0, bytes: 0 }
    }
}

impl Http for UreqHttp {
    fn perform(&mut self, request: &Request, cap: u64) -> Result<Response, HttpError> {
        self.requests += 1;
        let mut call = self.agent.get(&request.url).header("Accept-Encoding", "gzip");
        if let Some((start, end)) = request.range {
            call = call.header("Range", &format!("bytes={start}-{end}"));
        }
        if let Some(etag) = &request.if_none_match {
            call = call.header("If-None-Match", etag);
        }
        if let Some(since) = &request.if_modified_since {
            call = call.header("If-Modified-Since", since);
        }
        let response = match call.call() {
            Ok(response) => response,
            Err(ureq::Error::StatusCode(304)) => {
                return Ok(Response {
                    status: 304,
                    body: Vec::new(),
                    etag: None,
                    last_modified: None,
                    expires: None,
                    retry_after: None,
                })
            }
            Err(ureq::Error::StatusCode(code)) => return Err(HttpError::Status { code, retry_after: None }),
            Err(error) => return Err(HttpError::Transport(error.to_string())),
        };
        let (parts, body) = response.into_parts();
        let status = parts.status.as_u16();
        let header = |name: &str| parts.headers.get(name).and_then(|value| value.to_str().ok()).map(str::to_string);
        let retry_after = header("retry-after");
        if status == 304 {
            return Ok(Response {
                status,
                body: Vec::new(),
                etag: None,
                last_modified: None,
                expires: None,
                retry_after,
            });
        }
        if !(200..300).contains(&status) {
            return Err(HttpError::Status { code: status, retry_after });
        }
        // A range request answered 200 means the server ignored `Range` and is streaming the
        // whole object. Legal HTTP — but reading the head as if it were the middle would be
        // silent corruption, so the caller slices it (below) after proving the length.
        let mut bytes = Vec::new();
        let mut reader = body.into_reader().take(cap + 1);
        if let Err(error) = reader.read_to_end(&mut bytes) {
            return Err(HttpError::Transport(format!("body read: {error}")));
        }
        if bytes.len() as u64 > cap {
            return Err(HttpError::TooLarge { cap });
        }
        self.bytes += bytes.len() as u64;
        Ok(Response {
            status,
            body: bytes,
            etag: header("etag"),
            last_modified: header("last-modified"),
            expires: header("expires"),
            retry_after,
        })
    }

    fn requests(&self) -> u32 {
        self.requests
    }

    fn bytes(&self) -> u64 {
        self.bytes
    }
}

// ── fixtures ───────────────────────────────────────────────────────────────────────────────

/// An in-memory origin: whole objects by URL, sliced on `Range` exactly as a real one would.
/// This is what CI runs — the fetch path is exercised end to end without a socket.
/// `(etag, last-modified, expires)` for one fixture object.
type FixtureHeaders = (Option<String>, Option<String>, Option<String>);

#[derive(Debug, Default, Clone)]
pub struct FixtureHttp {
    objects: BTreeMap<String, Vec<u8>>,
    /// Every URL+range asked for, in order — the request-accounting ledger the OBCG §7 read
    /// pattern is pinned against.
    pub ledger: Vec<(String, Option<(u64, u64)>)>,
    /// Per-URL headers handed back with a 200.
    headers: BTreeMap<String, FixtureHeaders>,
}

impl FixtureHttp {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_object(mut self, url: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        self.objects.insert(url.into(), bytes.into());
        self
    }

    pub fn with_headers(
        mut self,
        url: impl Into<String>,
        etag: Option<&str>,
        last_modified: Option<&str>,
        expires: Option<&str>,
    ) -> Self {
        self.headers.insert(
            url.into(),
            (etag.map(str::to_string), last_modified.map(str::to_string), expires.map(str::to_string)),
        );
        self
    }

    /// Total bytes the ledger's reads would move.
    pub fn fetched_bytes(&self) -> u64 {
        self.ledger
            .iter()
            .map(|(url, range)| match range {
                Some((start, end)) => end.saturating_sub(*start) + 1,
                None => self.objects.get(url).map(|o| o.len() as u64).unwrap_or(0),
            })
            .sum()
    }
}

impl Http for FixtureHttp {
    fn perform(&mut self, request: &Request, cap: u64) -> Result<Response, HttpError> {
        self.ledger.push((request.url.clone(), request.range));
        let Some(object) = self.objects.get(&request.url) else {
            return Err(HttpError::Status { code: 404, retry_after: None });
        };
        let body = match request.range {
            None => object.clone(),
            Some((start, end)) => {
                let start = usize::try_from(start).map_err(|_| HttpError::RangeNotHonoured("offset".into()))?;
                let end = usize::try_from(end).map_err(|_| HttpError::RangeNotHonoured("offset".into()))?;
                if start > end || end >= object.len() {
                    return Err(HttpError::RangeNotHonoured(format!("bytes={start}-{end} outside the object")));
                }
                object[start..=end].to_vec()
            }
        };
        if body.len() as u64 > cap {
            return Err(HttpError::TooLarge { cap });
        }
        let (etag, last_modified, expires) = self.headers.get(&request.url).cloned().unwrap_or_default();
        // ETag revalidation, so the manifest's 304 path is reachable from fixtures.
        if let (Some(sent), Some(have)) = (&request.if_none_match, &etag) {
            if sent == have {
                return Ok(Response {
                    status: 304,
                    body: Vec::new(),
                    etag: None,
                    last_modified: None,
                    expires: expires.clone(),
                    retry_after: None,
                });
            }
        }
        Ok(Response { status: 200, body, etag, last_modified, expires, retry_after: None })
    }

    fn requests(&self) -> u32 {
        self.ledger.len() as u32
    }

    fn bytes(&self) -> u64 {
        self.fetched_bytes()
    }
}

// ── failure controls ───────────────────────────────────────────────────────────────────────

/// What the simulator's failure knobs do to the wire. Each is a *transport-level* fault: none of
/// them lets the client skip a validation step, which is the point — a corrupted tile must be
/// caught by the production CRC check, not by a simulator branch.
#[derive(Debug, Clone, Default)]
pub struct FailureControls {
    /// Every request fails as if the radio were off.
    pub offline: bool,
    /// Added to every request.
    pub latency: Duration,
    /// Answer the Nth request (0-based) and every later one with this status.
    pub fail_from: Option<(u32, u16)>,
    /// Cut the Nth request's body in half (the interrupted-fetch fixture).
    pub truncate_request: Option<u32>,
    /// Flip one bit in the Nth request's body (the corrupt-tile / corrupt-page fixture).
    pub corrupt_request: Option<u32>,
}

impl FailureControls {
    pub fn is_active(&self) -> bool {
        self.offline
            || !self.latency.is_zero()
            || self.fail_from.is_some()
            || self.truncate_request.is_some()
            || self.corrupt_request.is_some()
    }
}

/// Wraps any [`Http`] with [`FailureControls`].
pub struct FaultyHttp<H: Http> {
    inner: H,
    controls: FailureControls,
    seen: u32,
}

impl<H: Http> FaultyHttp<H> {
    pub fn new(inner: H, controls: FailureControls) -> Self {
        Self { inner, controls, seen: 0 }
    }

    pub fn into_inner(self) -> H {
        self.inner
    }

    pub fn inner(&self) -> &H {
        &self.inner
    }
}

impl<H: Http> Http for FaultyHttp<H> {
    fn perform(&mut self, request: &Request, cap: u64) -> Result<Response, HttpError> {
        let index = self.seen;
        self.seen += 1;
        if !self.controls.latency.is_zero() {
            std::thread::sleep(self.controls.latency);
        }
        if self.controls.offline {
            return Err(HttpError::Transport("simulated offline".into()));
        }
        if let Some((from, code)) = self.controls.fail_from {
            if index >= from {
                return Err(HttpError::Status { code, retry_after: None });
            }
        }
        let mut response = self.inner.perform(request, cap)?;
        if self.controls.truncate_request == Some(index) {
            let keep = response.body.len() / 2;
            response.body.truncate(keep);
        }
        if self.controls.corrupt_request == Some(index) {
            if let Some(byte) = response.body.last_mut() {
                *byte ^= 0x01;
            }
        }
        Ok(response)
    }

    fn requests(&self) -> u32 {
        self.seen
    }

    fn bytes(&self) -> u64 {
        self.inner.bytes()
    }
}
