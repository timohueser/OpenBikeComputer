//! The S3 wire: SigV4-signed requests to Cloudflare R2 over the HTTP client this crate already
//! has, so a published object costs **one request on a pooled connection** rather than one
//! `rclone` process.
//!
//! ## Why this exists rather than a subprocess (#1279)
//!
//! The measured cycle on the production box was 220 s wall against 155 s of CPU across 4 cores —
//! ~18 % utilization, moving 16.3 MB at an effective ~90 KB/s. The same cycle to a local directory
//! is 36–39 s. Every one of those seconds was a process: `rclone` was spawned once per object, and
//! each spawn paid a fork, a config parse, a TLS handshake and an S3 auth exchange to move ~75 KB.
//! 217 puts + 217 `size` proofs + up to 216 deletes ≈ 650 spawns.
//!
//! Signing the requests here keeps one TLS connection pool alive for the whole cycle and lets the
//! upload phase run several requests at once. It also buys the thing no amount of process
//! parallelism could: **a typed answer**. `rclone` reports a missing object by writing "not found"
//! somewhere on stderr, which is a text format that changed under us — Debian's rclone 1.60.1
//! exits `0` with empty output for a missing object, so the previous backend read an absent
//! manifest as `Some(vec![])` and a fresh prefix could never bootstrap. Here absence is `404`, an
//! integer defined by the protocol, and an empty object is `200` with a zero-length body. The two
//! can no longer be confused on any client version.
//!
//! ## What is deliberately not here
//!
//! No `list`. `crate::sweep` derives every key it deletes from a manifest, and the store seam
//! having no way to enumerate a prefix is what keeps that structural rather than disciplined.
//!
//! Credentials live in this struct and reach the network only inside an `Authorization` header
//! computed per request; they are never in a command line, an environment passed to a child, or a
//! config file — there is no child and no file. [`S3::redact`] stays as the backstop for the one
//! remaining path, an error message quoting a response body.

use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};

use crate::manifest_v2::hex;

/// SHA-256 of the empty body, which every GET/HEAD/DELETE signs.
const EMPTY_PAYLOAD_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";

/// Objects are shards of a 648 M-cell mosaic; the largest observed is far under this, and the cap
/// is here so a torn or hostile response cannot be read into memory unbounded.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// A single request's ceiling. A ~75 KB object over a working connection is milliseconds, so this
/// is three orders of magnitude of slack — and it is deliberately not larger, because it multiplies
/// by [`S3::attempts`] into the worst case one black-holed key can cost.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The longest any single retry may wait, and the ceiling a `Retry-After` is clamped to. A weather
/// cycle that is already late has nothing to gain by honouring a five-minute back-off.
const MAX_BACKOFF: Duration = Duration::from_secs(8);

/// **The one header this client must not let the HTTP layer choose.**
///
/// `ureq` advertises `accept-encoding: gzip` by default and transparently decodes what comes back —
/// and a decoded body has no `Content-Length`, because the header described the *compressed*
/// stream. Two things break at once when that happens, and both silently:
///
/// * [`S3::get`]'s torn-body compare no-ops. That check is the property #1280 paid for, and it
///   exists because this bucket demonstrably tears bodies mid-stream. A short read would reach the
///   caller as a valid-looking short document.
/// * [`S3::head`] finds no `Content-Length`, calls it an anomaly, and burns its whole retry ladder
///   on every single key — measured at 4 attempts / 5.2 s each, which is 19 minutes across a
///   generation.
///
/// Asking for `identity` costs nothing (OBCG objects are already DEFLATE-compressed; there is no
/// second compression to win) and keeps the length the protocol states describing the bytes the
/// caller receives. `a_gzipped_response_never_reaches_the_caller_as_a_short_document` fails if this
/// is ever dropped.
const ACCEPT_ENCODING: &str = "identity";

const USER_AGENT: &str = "obc-wx-bake/0.1 https://github.com/timohueser/OpenBikeComputer";

/// An S3-compatible bucket, path-style, addressed with SigV4.
///
/// Cloned freely: [`ureq::Agent`] is a handle onto a shared connection pool, so every clone
/// dispatches over the same keep-alive connections. That is what makes the concurrent upload phase
/// in [`crate::publish`] reuse handshakes instead of repeating them.
#[derive(Clone)]
pub struct S3 {
    agent: ureq::Agent,
    /// `https://host[:port]`, no path, no trailing slash.
    endpoint: String,
    /// Exactly the `Host` header value the signature covers.
    host: String,
    bucket: String,
    region: String,
    access_key_id: String,
    secret_access_key: String,
    attempts: u32,
    /// When the phase this client is serving must be over, whatever it has achieved.
    ///
    /// Without it the arithmetic is grim: `attempts` x [`REQUEST_TIMEOUT`] plus back-off is the
    /// worst case for **one** key, and the head phase walks 217 of them sequentially. The only
    /// backstop was systemd's `TimeoutStartSec=600`, and being SIGKILLed mid-publish is precisely
    /// the moment the ordering guarantees are least legible — the manifest may or may not have
    /// swapped, and nothing got to say so. A deadline turns that into an ordinary `Err` before the
    /// swap, which is a state the whole design already knows how to be in.
    deadline: Option<Instant>,
}

/// How a failed attempt should be treated, the same split [`crate::fetch`] applies upstream:
/// transport faults and 5xx/429/408 are the network being the network, any other 4xx is a contract
/// failure and retrying it just spends the cycle's clock arriving at the same answer.
enum Attempt<T> {
    Done(T),
    /// Retryable, with the server's own `Retry-After` when it offered one.
    Retry(String, Option<Duration>),
    Fatal(String),
}

impl S3 {
    pub fn new(
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        region: impl Into<String>,
        access_key_id: impl Into<String>,
        secret_access_key: impl Into<String>,
        concurrency: usize,
    ) -> Result<Self, String> {
        let endpoint = endpoint.into();
        let endpoint = endpoint.trim_end_matches('/').to_string();
        let host = host_of(&endpoint)?;
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(REQUEST_TIMEOUT))
            .user_agent(USER_AGENT)
            // The whole point: keep every worker's connection alive between objects. Left at the
            // default 3, the upload phase would re-handshake for most of its requests and give
            // most of the win back.
            .max_idle_connections_per_host(concurrency.max(1))
            .max_idle_connections(concurrency.max(1) * 2)
            // Statuses are values here, not errors — 404 is this module's whole absence story and
            // must not arrive as a string to be pattern-matched.
            .http_status_as_error(false)
            // See [`ACCEPT_ENCODING`]. This is a correctness setting, not a performance one.
            .accept_encoding(ACCEPT_ENCODING)
            .build();
        Ok(Self {
            agent: config.into(),
            endpoint,
            host,
            bucket: bucket.into(),
            region: region.into(),
            access_key_id: access_key_id.into(),
            secret_access_key: secret_access_key.into(),
            attempts: 4,
            deadline: None,
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// Every request from now until [`Self::clear_deadline`] must finish before `at`.
    ///
    /// It bounds a *phase*, not a request: the point is that a publish which cannot finish inside
    /// its budget fails cleanly, before the manifest swap, rather than being killed by systemd at
    /// an unknown point. It is checked before each attempt and before each back-off sleep, so the
    /// overrun is bounded by one request timeout rather than by one ladder.
    pub fn set_deadline(&mut self, at: Instant) {
        self.deadline = Some(at);
    }

    pub fn clear_deadline(&mut self) {
        self.deadline = None;
    }

    fn out_of_time(&self) -> bool {
        self.deadline.is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Defensive backstop: a secret echoed back in a response body must never reach a log.
    pub fn redact(&self, text: &str) -> String {
        if self.secret_access_key.is_empty() {
            return text.to_string();
        }
        text.replace(&self.secret_access_key, "***")
    }

    /// Store `bytes` at `key`. Returns only after the service has acknowledged the write.
    pub fn put(&self, key: &str, bytes: &[u8], cache_control: &str, content_type: &str) -> Result<(), String> {
        let payload_hash = sha256_hex(bytes);
        self.with_retries(key, |_| {
            let signed = self.sign(
                "PUT",
                key,
                &payload_hash,
                &[("cache-control", cache_control), ("content-type", content_type)],
            );
            let mut request = self.agent.put(&signed.url);
            for (name, value) in &signed.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            match request.send(bytes) {
                Ok(response) => match response.status().as_u16() {
                    200 | 201 | 204 => Attempt::Done(()),
                    status => self.classify(status, response),
                },
                Err(error) => Attempt::Retry(format!("{error}"), None),
            }
        })
    }

    /// The length of the object at `key`, or `None` if there is nothing there.
    ///
    /// **`None` means 404 and nothing else.** A zero-length object answers `Some(0)`.
    pub fn head(&self, key: &str) -> Result<Option<u64>, String> {
        self.with_retries(key, |_| {
            let signed = self.sign("HEAD", key, EMPTY_PAYLOAD_SHA256, &[]);
            let mut request = self.agent.head(&signed.url);
            for (name, value) in &signed.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            match request.call() {
                Ok(response) => match response.status().as_u16() {
                    200 => {
                        let length = response
                            .headers()
                            .get("content-length")
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.parse::<u64>().ok());
                        match length {
                            Some(length) => Attempt::Done(Some(length)),
                            // A 200 with no length is a broken proxy, not an absence, and calling
                            // it either would be a lie. Retry it: the next attempt gets a header.
                            None => Attempt::Retry("200 with no Content-Length".to_string(), None),
                        }
                    }
                    404 => Attempt::Done(None),
                    status => self.classify(status, response),
                },
                Err(error) => Attempt::Retry(format!("{error}"), None),
            }
        })
    }

    /// Read the object at `key` back, or `None` if there is nothing there.
    ///
    /// The distinction the bootstrap bug turned on: an absent key is `Ok(None)`, an object that
    /// exists and is empty is `Ok(Some(vec![]))`.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        self.with_retries(key, |_| {
            let signed = self.sign("GET", key, EMPTY_PAYLOAD_SHA256, &[]);
            let mut request = self.agent.get(&signed.url);
            for (name, value) in &signed.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            match request.call() {
                Ok(response) => match response.status().as_u16() {
                    200 => {
                        let declared = response
                            .headers()
                            .get("content-length")
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.parse::<u64>().ok());
                        match response.into_body().with_config().limit(MAX_RESPONSE_BYTES).read_to_vec() {
                            // **A short body is a torn read, not a short document** (#1280). This
                            // bucket has a recorded history of tearing bodies mid-stream, and
                            // unchecked it reaches the caller as a valid-looking truncated
                            // document — for the manifest, a JSON parse error blamed on the wrong
                            // thing, which is exactly how a cycle wedges itself.
                            Ok(bytes) if declared.is_some_and(|declared| declared != bytes.len() as u64) => {
                                Attempt::Retry(
                                    format!(
                                        "read back {} bytes but the response declared {} — a torn or truncated body",
                                        bytes.len(),
                                        declared.unwrap_or_default()
                                    ),
                                    None,
                                )
                            }
                            Ok(bytes) => Attempt::Done(Some(bytes)),
                            // Retrying it is why the retry ladder survived the move off rclone,
                            // which retried low-level errors internally and no longer does it for us.
                            Err(error) => Attempt::Retry(format!("body: {error}"), None),
                        }
                    }
                    404 => Attempt::Done(None),
                    status => self.classify(status, response),
                },
                Err(error) => Attempt::Retry(format!("{error}"), None),
            }
        })
    }

    /// Remove the object at `key`.
    ///
    /// `DeleteObject` is idempotent by definition: S3 answers `204` whether or not anything was
    /// there, and R2 charges nothing for it either way. So this cannot report whether the key had
    /// an object — see [`crate::publish::Deleted::existed`], which says so in its type rather than
    /// guessing.
    pub fn delete(&self, key: &str) -> Result<(), String> {
        self.with_retries(key, |_| {
            let signed = self.sign("DELETE", key, EMPTY_PAYLOAD_SHA256, &[]);
            let mut request = self.agent.delete(&signed.url);
            for (name, value) in &signed.headers {
                request = request.header(name.as_str(), value.as_str());
            }
            match request.call() {
                Ok(response) => match response.status().as_u16() {
                    // 404 is the outcome this call was asking for.
                    200 | 202 | 204 | 404 => Attempt::Done(()),
                    status => self.classify(status, response),
                },
                Err(error) => Attempt::Retry(format!("{error}"), None),
            }
        })
    }

    /// A status this call did not expect, with as much of the body as is useful for a journal line.
    ///
    /// The split is stated once, here, and every verb defers to it: **`408`, `429` and `5xx` are
    /// the network; every other status is the contract.** `408 Request Timeout` belongs with the
    /// first group and used to sit with the second — a server saying "you were too slow, ask again"
    /// is the one 4xx that is literally a request to retry.
    fn classify<T>(&self, status: u16, response: ureq::http::Response<ureq::Body>) -> Attempt<T> {
        let retry_after = retry_after(response.headers());
        let body = response
            .into_body()
            .with_config()
            .limit(4096)
            .read_to_vec()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .unwrap_or_default();
        let detail = self
            .redact(&format!("status {status}{}", if body.is_empty() { String::new() } else { format!(": {body}") }));
        if status == 408 || status == 429 || status >= 500 {
            Attempt::Retry(detail, retry_after)
        } else {
            Attempt::Fatal(detail)
        }
    }

    /// The ladder: bounded attempts, exponential back-off, the server's own `Retry-After` when it
    /// offered one, jitter so eight workers that hit one rate limit together do not come back
    /// together, and the phase deadline over all of it.
    fn with_retries<T>(&self, key: &str, mut once: impl FnMut(u32) -> Attempt<T>) -> Result<T, String> {
        let mut delay = Duration::from_millis(400);
        let mut last = String::new();
        for attempt in 1..=self.attempts {
            // Checked *before* the attempt, so an expired budget costs zero further requests rather
            // than one more request timeout per remaining key.
            if self.out_of_time() {
                return Err(format!(
                    "{key}: the publish phase ran out of its time budget{} — failing the cycle here, before \
                     anything irreversible, rather than being killed part-way through",
                    if last.is_empty() { String::new() } else { format!(" (last: {})", self.redact(&last)) }
                ));
            }
            match once(attempt) {
                Attempt::Done(value) => return Ok(value),
                Attempt::Fatal(message) => return Err(format!("{key}: {}", self.redact(&message))),
                Attempt::Retry(message, retry_after) => {
                    last = message;
                    if attempt < self.attempts {
                        // Jitter: +0..25 % of the wait, keyed off the key and the attempt rather
                        // than a random source, so a failing cycle is still reproducible.
                        let base = retry_after.unwrap_or(delay).min(MAX_BACKOFF);
                        let spread = base.mul_f64(0.25 * jitter_fraction(key, attempt));
                        let wait = base + spread;
                        // A sleep that would outlast the budget is a sleep nobody is waiting for.
                        if self.deadline.is_some_and(|deadline| Instant::now() + wait >= deadline) {
                            return Err(format!(
                                "{key}: {} — and the retry would outlast the publish phase's time budget",
                                self.redact(&last)
                            ));
                        }
                        std::thread::sleep(wait);
                        delay = (delay * 3).min(MAX_BACKOFF);
                    }
                }
            }
        }
        Err(format!("{key}: {} (after {} attempts)", self.redact(&last), self.attempts))
    }

    /// The SigV4 computation, in the order the specification states it.
    fn sign(&self, method: &str, key: &str, payload_sha256: &str, extra: &[(&str, &str)]) -> SignedRequest {
        let now = chrono::Utc::now();
        let timestamp = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date = now.format("%Y%m%d").to_string();
        let canonical_uri = format!("/{}/{}", uri_encode(&self.bucket, true), uri_encode(key, false));
        let url = format!("{}{canonical_uri}", self.endpoint);

        // Canonical headers: lowercase names, trimmed values, sorted, one per line. Everything this
        // client sends is signed; anything the HTTP layer adds on its own (`user-agent`,
        // `content-length`) is not, which is what `SignedHeaders` exists to say.
        let mut headers: Vec<(String, String)> = vec![
            ("host".to_string(), self.host.clone()),
            ("x-amz-content-sha256".to_string(), payload_sha256.to_string()),
            ("x-amz-date".to_string(), timestamp.clone()),
        ];
        for (name, value) in extra {
            headers.push((name.to_ascii_lowercase(), (*value).trim().to_string()));
        }
        headers.sort_by(|left, right| left.0.cmp(&right.0));

        let signed_headers = headers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(";");
        let canonical_headers =
            headers.iter().map(|(name, value)| format!("{name}:{value}\n")).collect::<Vec<_>>().concat();
        let canonical_request =
            format!("{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_sha256}");

        let scope = format!("{date}/{}/{SERVICE}/aws4_request", self.region);
        let string_to_sign = format!("{ALGORITHM}\n{timestamp}\n{scope}\n{}", sha256_hex(canonical_request.as_bytes()));
        let signature =
            hex(&hmac_sha256(&signing_key(&self.secret_access_key, &date, &self.region), string_to_sign.as_bytes()));

        // The `host` header is set by the HTTP layer from the URL and must not be sent twice.
        let mut wire: Vec<(String, String)> = headers.into_iter().filter(|(name, _)| name != "host").collect();
        wire.push((
            "authorization".to_string(),
            format!(
                "{ALGORITHM} Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                self.access_key_id
            ),
        ));
        SignedRequest { url, headers: wire }
    }
}

struct SignedRequest {
    url: String,
    headers: Vec<(String, String)>,
}

/// `kSigning` = HMAC(HMAC(HMAC(HMAC("AWS4"+secret, date), region), "s3"), "aws4_request").
fn signing_key(secret: &str, date: &str, region: &str) -> Vec<u8> {
    let date_key = hmac_sha256(format!("AWS4{secret}").as_bytes(), date.as_bytes());
    let region_key = hmac_sha256(&date_key, region.as_bytes());
    let service_key = hmac_sha256(&region_key, SERVICE.as_bytes());
    hmac_sha256(&service_key, b"aws4_request")
}

fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), data).as_ref().to_vec()
}

fn sha256_hex(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// `Retry-After` as a duration, whether the server sent seconds or an HTTP date. Anything longer
/// than [`MAX_BACKOFF`] is clamped by the caller: a weather cycle that is already late gains
/// nothing by honouring a five-minute back-off it will be replaced during.
fn retry_after(headers: &ureq::http::HeaderMap) -> Option<Duration> {
    let value = headers.get("retry-after")?.to_str().ok()?.trim().to_string();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let at = chrono::DateTime::parse_from_rfc2822(&value).ok()?;
    let seconds = (at.timestamp() - chrono::Utc::now().timestamp()).max(0);
    Some(Duration::from_secs(seconds as u64))
}

/// A deterministic 0.0..1.0 spread from the key and the attempt number.
///
/// Deterministic on purpose: eight workers that hit one rate limit at the same instant must not
/// come back at the same instant, but a failing cycle should still be reproducible from its log —
/// a random source would make the same failure take a different shape every run.
fn jitter_fraction(key: &str, attempt: u32) -> f64 {
    let digest = Sha256::digest(format!("{key}#{attempt}").as_bytes());
    f64::from(u16::from_be_bytes([digest[0], digest[1]])) / f64::from(u16::MAX)
}

/// RFC 3986 unreserved-set encoding, which is what SigV4's canonical URI is. `/` is a path
/// separator in a key and stays one unless the caller says otherwise (a bucket name is one segment).
fn uri_encode(input: &str, encode_slash: bool) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(byte)),
            b'/' if !encode_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The `Host` header the signature must cover: the authority of the endpoint, with the port only
/// when it is not the scheme's default (S3 signing rejects `:443` on an https URL).
///
/// **Userinfo is refused outright**, not stripped. `https://key:secret@host` is a legal URL, and
/// accepting one would put a credential inside `endpoint` — a field [`S3::describe`] prints into
/// the journal on every start, and which [`S3::redact`] would not catch because it only knows the
/// secret it was constructed with. A misconfigured `OBC_WX_R2_ENDPOINT` must fail loudly at
/// startup, not leak quietly forever.
fn host_of(endpoint: &str) -> Result<String, String> {
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| format!("{endpoint} is not an absolute URL (expected https://host)"))?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(format!("{endpoint} has no host"));
    }
    if authority.contains('@') {
        // Note what is *not* in this message: the endpoint. Quoting it back would print the very
        // credential the check exists to keep out of the journal.
        return Err("the endpoint carries userinfo (a `user:password@host` prefix) — refusing it rather than \
                    stripping it, because the endpoint is printed to the journal on every start. Put the \
                    credential in OBC_WX_R2_ACCESS_KEY_ID / OBC_WX_R2_SECRET_ACCESS_KEY and give \
                    OBC_WX_R2_ENDPOINT a bare origin"
            .to_string());
    }
    let default_port = match scheme {
        "https" => ":443",
        "http" => ":80",
        other => return Err(format!("{other}:// is not a supported endpoint scheme")),
    };
    Ok(authority.strip_suffix(default_port).unwrap_or(authority).to_string())
}

/// A single-process S3 stand-in, so the wire contract this module depends on is *executed* by the
/// tests rather than described by them. Test-only, and small on purpose: it answers the four verbs
/// this client speaks and nothing else — no listing, no multipart.
///
/// It is **not** a permissive stub. It rejects a request whose `Authorization` header is not a
/// well-formed SigV4 credential with a 64-hex signature over the three headers every request must
/// sign, so every test that touches the wire is also a signing test — a client that stopped signing
/// would fail all of them rather than passing quietly against a stub that never looked.
#[cfg(test)]
pub(crate) mod double {
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct Behaviour {
        /// Send one byte less than the object holds — the mid-stream tear this bucket has a
        /// recorded history of.
        truncate: AtomicBool,
        /// Honour `accept-encoding: gzip` when the client offers it. **This is a trap, deliberately**
        /// — see `ACCEPT_ENCODING`. A gzip response is close-delimited here, with no
        /// `Content-Length` at all, which is exactly the state a decoding client ends up in.
        gzip_if_allowed: AtomicBool,
        /// Keys whose every request answers `403`, so a batch can be made to fail at a chosen object.
        fail: Mutex<Vec<String>>,
        /// Requests served, by `<METHOD> <key>`.
        seen: Mutex<BTreeMap<String, usize>>,
        /// Requests that arrived without a well-formed SigV4 `Authorization`.
        unsigned: AtomicUsize,
    }

    pub struct Double {
        pub endpoint: String,
        objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        behaviour: Arc<Behaviour>,
        running: Arc<AtomicBool>,
    }

    impl Double {
        pub fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
            let objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::new(Mutex::new(BTreeMap::new()));
            let behaviour = Arc::new(Behaviour::default());
            let running = Arc::new(AtomicBool::new(true));
            let (thread_objects, thread_behaviour, thread_running) =
                (Arc::clone(&objects), Arc::clone(&behaviour), Arc::clone(&running));
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if !thread_running.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(stream) = stream else { return };
                    let (objects, behaviour) = (Arc::clone(&thread_objects), Arc::clone(&thread_behaviour));
                    // One thread per connection: the concurrency test needs several at once, which
                    // is the property being tested.
                    std::thread::spawn(move || serve(stream, &objects, &behaviour));
                }
            });
            Self { endpoint, objects, behaviour, running }
        }

        pub fn set_truncate_bodies(&self, truncate: bool) {
            self.behaviour.truncate.store(truncate, Ordering::Relaxed);
        }

        pub fn set_gzip_if_allowed(&self, gzip: bool) {
            self.behaviour.gzip_if_allowed.store(gzip, Ordering::Relaxed);
        }

        pub fn fail_key(&self, key: &str) {
            self.behaviour.fail.lock().expect("fail set").push(key.to_string());
        }

        pub fn insert(&self, key: &str, bytes: Vec<u8>) {
            self.objects.lock().expect("objects").insert(key.to_string(), bytes);
        }

        pub fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.objects.lock().expect("objects").get(key).cloned()
        }

        pub fn len(&self) -> usize {
            self.objects.lock().expect("objects").len()
        }

        /// How many requests this key saw for that verb — the measure of a retry ladder that ran
        /// when it should not have.
        pub fn requests(&self, method: &str, key: &str) -> usize {
            self.behaviour.seen.lock().expect("seen").get(&format!("{method} {key}")).copied().unwrap_or(0)
        }

        pub fn unsigned_requests(&self) -> usize {
            self.behaviour.unsigned.load(Ordering::Relaxed)
        }
    }

    impl Drop for Double {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Relaxed);
            // Unblock the accept loop so the thread can observe the flag and exit.
            let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://"));
        }
    }

    /// [`is_signed`], reachable from the test module so the check itself can be shown to have teeth.
    pub fn is_signed_for_tests(authorization: &str) -> bool {
        is_signed(authorization)
    }

    /// A SigV4 `Authorization` this double is willing to serve: the algorithm, a credential scoped
    /// to `s3/aws4_request`, the three headers every request must sign, and a 64-hex signature.
    fn is_signed(authorization: &str) -> bool {
        let Some(rest) = authorization.strip_prefix("AWS4-HMAC-SHA256 ") else { return false };
        let mut credential = None;
        let mut signed_headers = None;
        let mut signature = None;
        for part in rest.split(',') {
            let part = part.trim();
            if let Some(value) = part.strip_prefix("Credential=") {
                credential = Some(value);
            } else if let Some(value) = part.strip_prefix("SignedHeaders=") {
                signed_headers = Some(value);
            } else if let Some(value) = part.strip_prefix("Signature=") {
                signature = Some(value);
            }
        }
        let (Some(credential), Some(signed_headers), Some(signature)) = (credential, signed_headers, signature) else {
            return false;
        };
        let scope: Vec<&str> = credential.split('/').collect();
        scope.len() == 5
            && !scope[0].is_empty()
            && scope[3] == "s3"
            && scope[4] == "aws4_request"
            && ["host", "x-amz-content-sha256", "x-amz-date"]
                .iter()
                .all(|required| signed_headers.split(';').any(|name| name == *required))
            && signature.len() == 64
            && signature.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    fn serve(stream: TcpStream, objects: &Mutex<BTreeMap<String, Vec<u8>>>, behaviour: &Behaviour) {
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut request = String::new();
        if reader.read_line(&mut request).is_err() || request.is_empty() {
            return;
        }
        let mut parts = request.split_whitespace();
        let (method, path) =
            (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
        let mut length = 0usize;
        let mut authorization = String::new();
        let mut accepts_gzip = false;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
                } else if name.eq_ignore_ascii_case("authorization") {
                    authorization = value.trim().to_string();
                } else if name.eq_ignore_ascii_case("accept-encoding") {
                    accepts_gzip = value.to_ascii_lowercase().contains("gzip");
                }
            }
        }
        let mut body = vec![0u8; length];
        if length > 0 && reader.read_exact(&mut body).is_err() {
            return;
        }
        // Path-style: /<bucket>/<key...>. The key is everything after the bucket segment.
        let key =
            path.trim_start_matches('/').split_once('/').map(|(_bucket, key)| key).unwrap_or_default().to_string();
        *behaviour.seen.lock().expect("seen").entry(format!("{method} {key}")).or_default() += 1;

        // Auth first, exactly as a real endpoint does: an unsigned request never reaches the store.
        if !is_signed(&authorization) {
            behaviour.unsigned.fetch_add(1, Ordering::Relaxed);
            respond(stream, &method, "403 Forbidden", b"<Error><Code>SignatureDoesNotMatch</Code></Error>", false);
            return;
        }
        if behaviour.fail.lock().expect("fail set").iter().any(|failing| key.contains(failing.as_str())) {
            respond(stream, &method, "403 Forbidden", b"<Error><Code>AccessDenied</Code></Error>", false);
            return;
        }

        let mut store = objects.lock().expect("objects");
        let (status, payload) = match method.as_str() {
            "PUT" => {
                store.insert(key, body);
                ("200 OK", Vec::new())
            }
            "HEAD" => match store.get(&key) {
                Some(bytes) => ("200 OK", vec![0u8; bytes.len()]),
                None => ("404 Not Found", Vec::new()),
            },
            "GET" => match store.get(&key) {
                Some(bytes) => ("200 OK", bytes.clone()),
                None => ("404 Not Found", Vec::new()),
            },
            "DELETE" => {
                store.remove(&key);
                ("204 No Content", Vec::new())
            }
            _ => ("405 Method Not Allowed", Vec::new()),
        };
        drop(store);

        let truncate = behaviour.truncate.load(Ordering::Relaxed) && !payload.is_empty();
        let sent = if truncate { payload[..payload.len() - 1].to_vec() } else { payload.clone() };
        if accepts_gzip && behaviour.gzip_if_allowed.load(Ordering::Relaxed) && !sent.is_empty() {
            // Close-delimited and `Content-Encoding: gzip`, with **no `Content-Length`** — the state
            // a decoding client is in whether the header was stripped on the way through or never
            // sent. A client that accepts gzip therefore has nothing to compare a short body
            // against, which is the trap `a_gzipped_response_never_reaches_the_caller_as_a_short_
            // document` is set to spring.
            use std::io::Write as _;
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            let _ = encoder.write_all(&sent);
            let compressed = encoder.finish().unwrap_or_default();
            let mut out = stream;
            let head = format!("HTTP/1.1 {status}\r\nContent-Encoding: gzip\r\nConnection: close\r\n\r\n");
            let _ = out.write_all(head.as_bytes());
            if method != "HEAD" {
                let _ = out.write_all(&compressed);
            }
            let _ = out.flush();
            return;
        }
        // Identity: the declared length is the object's real length even when the body sent is
        // short, because that is what a torn response looks like.
        respond_with_length(stream, &method, status, &sent, payload.len());
    }

    fn respond(stream: TcpStream, method: &str, status: &str, payload: &[u8], _gzip: bool) {
        respond_with_length(stream, method, status, payload, payload.len());
    }

    fn respond_with_length(stream: TcpStream, method: &str, status: &str, sent: &[u8], declared: usize) {
        let mut out = stream;
        let head = format!("HTTP/1.1 {status}\r\nContent-Length: {declared}\r\nConnection: close\r\n\r\n");
        let _ = out.write_all(head.as_bytes());
        // A HEAD response carries the length header and no body, which is what makes `S3::head`
        // able to answer without transferring the object.
        if method != "HEAD" {
            let _ = out.write_all(sent);
        }
        let _ = out.flush();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn client() -> S3 {
        S3::new(
            "https://acct.r2.cloudflarestorage.com",
            "obc-wx",
            "auto",
            "AKIAIOSFODNN7EXAMPLE",
            "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            8,
        )
        .expect("a well-formed endpoint")
    }

    /// The SigV4 derivation ladder, pinned to **literal digests computed outside this code**.
    ///
    /// The first is AWS's own published `iam` vector, which is the only value in this file anyone
    /// can check against a document. The second and third are the `s3` ladder this client actually
    /// signs with, computed independently (python `hmac`/`hashlib`) rather than by re-running the
    /// expression under test — a test that re-derives the thing it is asserting proves only that
    /// the function is deterministic.
    #[test]
    fn the_signing_key_ladder_matches_independently_computed_digests() {
        const SECRET: &str = "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY";

        // AWS's published vector, for the `iam` service — the anchor the rest of the ladder hangs
        // off, since the first three rungs are shared with ours.
        let region_key = hmac_sha256(&hmac_sha256(format!("AWS4{SECRET}").as_bytes(), b"20150830"), b"us-east-1");
        assert_eq!(
            hex(&hmac_sha256(&hmac_sha256(&region_key, b"iam"), b"aws4_request")),
            "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9"
        );

        // The same date and region, service `s3` — what `signing_key` must produce.
        assert_eq!(
            hex(&signing_key(SECRET, "20150830", "us-east-1")),
            "32f78051dcde24c552811d654f4a769112bb834b03975cdd6b1fd7d16248c269"
        );
        // And in R2's own region, whose name really is "auto".
        assert_eq!(
            hex(&signing_key(SECRET, "20260811", "auto")),
            "4ffcce2ea7bd8827518e0498cd7b55c993d4f715d7ebc8ea7b94067bc17089fa"
        );
    }

    #[test]
    fn the_empty_payload_hash_is_the_constant_every_bodyless_request_signs() {
        assert_eq!(sha256_hex(b""), EMPTY_PAYLOAD_SHA256);
    }

    /// A shard key is path segments, not one escaped blob: `wx/v2/.../s3-2.obcg` must sign as a
    /// path or every request 403s on a canonical-URI mismatch.
    #[test]
    fn a_key_signs_as_a_path_and_a_bucket_signs_as_one_segment() {
        assert_eq!(uri_encode("wx/v2/20260810T1430Z/f45/s3-2.obcg", false), "wx/v2/20260810T1430Z/f45/s3-2.obcg");
        assert_eq!(uri_encode("obc-wx", true), "obc-wx");
        assert_eq!(uri_encode("a b/c", true), "a%20b%2Fc");
        assert_eq!(uri_encode("~-_.", true), "~-_.");
    }

    /// The signed `Host` is the authority without a default port, and the URL is path-style.
    #[test]
    fn the_signed_host_and_url_are_what_r2_expects() {
        assert_eq!(host_of("https://acct.r2.cloudflarestorage.com").expect("host"), "acct.r2.cloudflarestorage.com");
        assert_eq!(
            host_of("https://acct.r2.cloudflarestorage.com:443").expect("host"),
            "acct.r2.cloudflarestorage.com"
        );
        assert_eq!(host_of("http://127.0.0.1:9000").expect("host"), "127.0.0.1:9000");
        assert!(host_of("acct.r2.cloudflarestorage.com").is_err(), "a bare host is not an endpoint");
        assert!(host_of("ftp://host").is_err());

        // **Userinfo is refused, never stripped.** `endpoint` is printed to the journal by
        // `describe()` on every start, and `redact()` only knows the secret it was constructed
        // with — so a credential smuggled in through `OBC_WX_R2_ENDPOINT` would leak on every
        // single boot. It has to fail at startup instead.
        let error = host_of("https://AKIAEXAMPLE:hunter2@acct.r2.cloudflarestorage.com").expect_err("userinfo");
        assert!(error.contains("userinfo"), "{error}");
        // And the refusal must not quote the thing it is refusing: the endpoint *is* the credential
        // in this case, and `redact()` cannot help — it only knows the secret it was built with.
        assert!(!error.contains("hunter2"), "the refusal leaked the credential it exists to reject: {error}");
        assert!(host_of("https://user@host").is_err(), "userinfo without a password is still userinfo");
        assert!(
            S3::new("https://id:secret@acct.r2.cloudflarestorage.com", "obc-wx", "auto", "id", "secret", 8).is_err(),
            "the constructor must refuse it, not just the helper"
        );

        let signed =
            client().sign("PUT", "wx/v2/manifest.json", EMPTY_PAYLOAD_SHA256, &[("content-type", "application/json")]);
        assert_eq!(signed.url, "https://acct.r2.cloudflarestorage.com/obc-wx/wx/v2/manifest.json");
    }

    /// Nothing secret is on the wire except inside the signature, and nothing secret is in a
    /// header value we could log. There is no `argv` to leak into any more — there is no child.
    #[test]
    fn no_credential_reaches_a_header_value_or_a_log() {
        let store = client();
        let signed = store.sign("PUT", "wx/v2/manifest.json", EMPTY_PAYLOAD_SHA256, &[]);
        let names: Vec<&str> = signed.headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["x-amz-content-sha256", "x-amz-date", "authorization"]);
        for (name, value) in &signed.headers {
            assert!(!value.contains("wJalrXUtnFEMI"), "{name} leaked the secret");
        }
        // The authorization header names the *access key id*, which is not a secret, and carries
        // the signature — a value derived from the secret and unusable to recover it.
        let authorization = &signed.headers.iter().find(|(name, _)| name == "authorization").expect("signed").1;
        assert!(authorization.starts_with("AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/"), "{authorization}");
        assert!(authorization.contains("SignedHeaders=host;x-amz-content-sha256;x-amz-date"), "{authorization}");
        assert_eq!(store.redact("secret=wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY: 403"), "secret=***: 403");
    }

    /// **The bootstrap bug, executed** (#1279, and the task filed beside it).
    ///
    /// The rclone backend told absence from presence by matching stderr for "not found". Debian's
    /// rclone 1.60.1 exits `0` with empty output for a missing object, so `get` answered
    /// `Some(vec![])` for a key that was not there — and `carried_generations` read those zero
    /// bytes as a manifest it could not parse, refused to publish, and no fresh prefix could ever
    /// bootstrap. Here the two answers come from different status codes and cannot converge on any
    /// client version.
    #[test]
    fn an_absent_key_and_an_empty_object_are_different_answers() {
        let double = double::Double::start();
        let store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");

        // The bootstrap state: nothing at the manifest key at all.
        assert_eq!(store.get("wx/v2/manifest.json").expect("get"), None);
        assert_eq!(store.head("wx/v2/manifest.json").expect("head"), None);

        // An object that exists and happens to be empty. Same length as the bug's phantom answer,
        // categorically different meaning.
        double.insert("wx/v2/empty", Vec::new());
        assert_eq!(store.get("wx/v2/empty").expect("get"), Some(Vec::new()));
        assert_eq!(store.head("wx/v2/empty").expect("head"), Some(0));

        // And the live failure itself, end to end: the absent answer must reach
        // `carried_generations` as a bootstrap rather than as a manifest it refuses to publish
        // over. That refusal is correct (§10.4 — an empty chain deletes what clients are reading)
        // and it is why the wrong answer above wedged every cycle forever.
        let mut warnings = Vec::new();
        let previous = store.get("wx/v2/manifest.json").expect("get");
        let carried = crate::manifest_v2::carried_generations(previous.as_deref(), &mut warnings)
            .expect("a fresh wx/v2 prefix must bootstrap, not fail the cycle");
        assert!(!carried.had_predecessor(), "nothing was ever promised, so nothing may be swept");
        assert!(warnings.is_empty(), "a genuine bootstrap is not a warning: {warnings:?}");
    }

    /// A body shorter than the length the response declared is a torn read (#1280), and the client
    /// must not hand it up as a short document.
    #[test]
    fn a_truncated_body_is_an_error_and_never_a_short_document() {
        let double = double::Double::start();
        double.set_truncate_bodies(true);
        // One attempt: this test is about the classification, not about waiting out the ladder.
        let mut store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");
        store.attempts = 1;
        double.insert("wx/v2/manifest.json", vec![b'x'; 64]);

        // Two layers catch this and either is enough: the HTTP client notices the stream ended
        // short of the declared length, and the explicit compare in `get` catches a peer that
        // closes cleanly having sent fewer bytes. What must never happen is `Ok(Some(63 bytes))`.
        let error = store.get("wx/v2/manifest.json").expect_err("a torn body is not a document");
        assert!(error.starts_with("wx/v2/manifest.json: "), "{error}");
    }

    /// **The property, not the header** (#1282 review M1).
    ///
    /// The torn-body check compares the body read against the length the response declared. A
    /// content-coded response has no such length to compare against — the header described the
    /// compressed stream, and a decoding client is handed bytes it can no longer measure. So the
    /// check does not weaken when a response is gzipped: it **silently stops existing**.
    ///
    /// This test serves gzip to any client that asks for it, close-delimited and with no
    /// `Content-Length` at all, and truncates the object by a byte. It passes only because the
    /// client asks for `identity`. Drop [`ACCEPT_ENCODING`] — or let the HTTP layer choose again —
    /// and the assertion below fails with `Ok(Some(...))`, which is the bug reaching the caller.
    #[test]
    fn a_gzipped_response_never_reaches_the_caller_as_a_short_document() {
        let double = double::Double::start();
        double.set_gzip_if_allowed(true);
        double.set_truncate_bodies(true);
        let mut store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");
        store.attempts = 1;
        // Compressible, so a gzip round-trip would visibly "work" and hide the missing byte.
        double.insert("wx/v2/manifest.json", vec![b'a'; 4096]);

        match store.get("wx/v2/manifest.json") {
            Err(error) => assert!(error.starts_with("wx/v2/manifest.json: "), "{error}"),
            Ok(body) => panic!(
                "a torn body reached the caller as a {}-byte document — the response was content-coded, so the \
                 length compare in `get` had nothing to compare against. This is ACCEPT_ENCODING being dropped.",
                body.map(|bytes| bytes.len()).unwrap_or(0)
            ),
        }
    }

    /// The other half of M1: a content-coded response also costs `head` its whole retry ladder,
    /// because a decoded body has no `Content-Length` and `head` correctly refuses to call that an
    /// absence. Measured at 4 attempts / 5.2 s per key against the live bucket — 19 minutes across
    /// a generation, under a 600 s unit timeout.
    ///
    /// One request per key is the contract. The double counts them.
    #[test]
    fn a_head_costs_exactly_one_request() {
        let double = double::Double::start();
        double.set_gzip_if_allowed(true);
        let store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");
        double.insert("wx/v2/20260810T1430Z/f0/s0-0.obcg", vec![9u8; 2048]);

        let started = std::time::Instant::now();
        assert_eq!(store.head("wx/v2/20260810T1430Z/f0/s0-0.obcg").expect("head"), Some(2048));
        assert_eq!(double.requests("HEAD", "wx/v2/20260810T1430Z/f0/s0-0.obcg"), 1, "the ladder ran on a good answer");
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "took {:?}", started.elapsed());
    }

    /// Every request this client makes is signed, and the double refuses anything that is not. That
    /// makes each wire test above a signing test too — this one just states it.
    #[test]
    fn every_request_on_the_wire_carries_a_well_formed_signature() {
        let double = double::Double::start();
        let store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");
        let key = "wx/v2/20260810T1430Z/f0/s0-0.obcg";

        store.put(key, b"body", "immutable", "application/octet-stream").expect("put");
        store.head(key).expect("head");
        store.get(key).expect("get");
        store.delete(key).expect("delete");
        assert_eq!(double.unsigned_requests(), 0, "an unsigned request reached the endpoint");

        // And the check has teeth: the double really does reject a request it cannot verify.
        assert!(!double::is_signed_for_tests("Bearer hunter2"));
        assert!(!double::is_signed_for_tests(
            "AWS4-HMAC-SHA256 Credential=id/20260811/auto/s3/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        ));
    }

    /// Put, prove, read back, retire — the four operations a cycle performs, over the wire.
    #[test]
    fn an_object_round_trips_and_delete_is_idempotent() {
        let double = double::Double::start();
        let store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");
        let key = "wx/v2/20260810T1430Z/f45/s3-2.obcg";
        let bytes = vec![7u8; 1024];

        store.put(key, &bytes, "public, max-age=31536000, immutable", "application/octet-stream").expect("put");
        assert_eq!(double.get(key), Some(bytes.clone()), "the destination holds the bytes we sent");
        assert_eq!(store.head(key).expect("head"), Some(1024));
        assert_eq!(store.get(key).expect("get"), Some(bytes));

        store.delete(key).expect("delete");
        assert_eq!(store.head(key).expect("head"), None);
        // Idempotent: the second call asks for the same end state and gets it.
        store.delete(key).expect("delete again");
    }

    /// One `S3` shared across threads is the precondition the upload phase rests on: the client
    /// must be safe to call concurrently, with no key crossed with another's body. The *batch*
    /// orchestration on top of it is tested where it lives, in
    /// `publish::tests::a_concurrent_batch_lands_every_object_exactly_once`.
    #[test]
    fn one_client_is_safe_to_call_from_many_threads() {
        let double = double::Double::start();
        let store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 8).expect("client");
        let objects: Vec<(String, Vec<u8>)> =
            (0..64).map(|index| (format!("wx/v2/g/f0/s{index}-0.obcg"), vec![index as u8; index + 1])).collect();

        std::thread::scope(|scope| {
            for chunk in objects.chunks(8) {
                let store = &store;
                scope.spawn(move || {
                    for (key, bytes) in chunk {
                        store.put(key, bytes, "immutable", "application/octet-stream").expect("put");
                    }
                });
            }
        });

        assert_eq!(double.len(), objects.len());
        for (key, bytes) in &objects {
            assert_eq!(double.get(key).as_ref(), Some(bytes), "{key}");
        }
    }

    /// **The time budget** (#1282 review M3). A key the endpoint keeps refusing must not be able to
    /// spend the cycle's whole clock, and an expired budget must cost *zero* further requests
    /// rather than one more ladder per remaining key.
    #[test]
    fn an_expired_deadline_fails_immediately_and_issues_no_request() {
        let double = double::Double::start();
        let key = "wx/v2/20260810T1430Z/f0/s0-0.obcg";
        double.fail_key(key);
        let mut store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");
        store.set_deadline(std::time::Instant::now());

        let started = std::time::Instant::now();
        let error = store.head(key).expect_err("an expired budget is not an absence");
        assert!(error.contains("time budget"), "{error}");
        assert_eq!(double.requests("HEAD", key), 0, "an expired budget still issued a request");
        assert!(started.elapsed() < std::time::Duration::from_secs(1), "took {:?}", started.elapsed());

        // Cleared, the same call reaches the endpoint and fails on its merits instead.
        store.clear_deadline();
        let error = store.head(key).expect_err("403");
        assert!(error.contains("403"), "{error}");
        assert_eq!(double.requests("HEAD", key), 1, "a 403 is fatal, so the ladder must not run");
    }

    /// A 4xx that is not `408` ends the call; `408`/`429`/`5xx` are the network and retry. Getting
    /// this backwards either burns the budget on a permanent failure or gives up on a transient one.
    #[test]
    fn only_the_network_statuses_retry() {
        let double = double::Double::start();
        let key = "wx/v2/20260810T1430Z/f0/s1-0.obcg";
        double.fail_key(key);
        let store = S3::new(&double.endpoint, "obc-wx", "auto", "id", "secret", 4).expect("client");

        let error = store.put(key, b"body", "immutable", "application/octet-stream").expect_err("403");
        assert!(error.contains("AccessDenied"), "{error}");
        assert_eq!(double.requests("PUT", key), 1, "403 is a contract failure and must not be retried");
        assert!(!error.contains("after"), "a fatal error must not be reported as an exhausted ladder: {error}");
    }

    /// `Retry-After` is honoured but clamped, and the jitter that keeps eight workers from
    /// returning in lockstep is deterministic — a failing cycle must still be reproducible.
    #[test]
    fn backoff_honours_retry_after_within_a_ceiling_and_jitters_deterministically() {
        let mut headers = ureq::http::HeaderMap::new();
        headers.insert("retry-after", ureq::http::HeaderValue::from_static("2"));
        assert_eq!(retry_after(&headers), Some(Duration::from_secs(2)));
        headers.insert("retry-after", ureq::http::HeaderValue::from_static("not a number"));
        assert_eq!(retry_after(&headers), None);
        assert_eq!(retry_after(&ureq::http::HeaderMap::new()), None);
        // Whatever a server asks for, the ladder never waits longer than the ceiling.
        assert!(Duration::from_secs(600).min(MAX_BACKOFF) <= MAX_BACKOFF);

        let fraction = jitter_fraction("wx/v2/manifest.json", 2);
        assert!((0.0..=1.0).contains(&fraction), "{fraction}");
        assert_eq!(fraction, jitter_fraction("wx/v2/manifest.json", 2), "jitter must be reproducible");
        assert_ne!(fraction, jitter_fraction("wx/v2/manifest.json", 3), "and must differ across attempts");
        assert_ne!(fraction, jitter_fraction("wx/v2/20260810T1430Z/f0/s0-0.obcg", 2), "and across keys");
    }

    /// `SignedHeaders` must list exactly the headers the canonical block hashed, in the same sorted
    /// order, including the ones the caller added.
    #[test]
    fn extra_headers_are_signed_in_canonical_order() {
        let signed = client().sign(
            "PUT",
            "wx/v2/20260810T1430Z/f0/s0-0.obcg",
            &sha256_hex(b"body"),
            &[("content-type", "application/octet-stream"), ("cache-control", "public, max-age=31536000, immutable")],
        );
        let authorization = &signed.headers.iter().find(|(name, _)| name == "authorization").expect("signed").1;
        assert!(
            authorization.contains("SignedHeaders=cache-control;content-type;host;x-amz-content-sha256;x-amz-date"),
            "{authorization}"
        );
    }
}
