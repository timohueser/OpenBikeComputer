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

use std::time::Duration;

use sha2::{Digest, Sha256};

/// SHA-256 of the empty body, which every GET/HEAD/DELETE signs.
const EMPTY_PAYLOAD_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

const ALGORITHM: &str = "AWS4-HMAC-SHA256";
const SERVICE: &str = "s3";

/// Objects are shards of a 648 M-cell mosaic; the largest observed is far under this, and the cap
/// is here so a torn or hostile response cannot be read into memory unbounded.
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;

/// A single request's ceiling. Generous for a ~75 KB object, tight enough that a black-holed
/// connection cannot hold the cycle's lock until systemd's `TimeoutStartSec`.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
}

/// How a failed attempt should be treated, the same split [`crate::fetch`] applies upstream:
/// transport faults and 5xx/429 are the network being the network, a 4xx is a contract failure and
/// retrying it just spends the cycle's clock arriving at the same answer.
enum Attempt<T> {
    Done(T),
    Retry(String),
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
        })
    }

    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
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
                Err(error) => Attempt::Retry(format!("{error}")),
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
                            None => Attempt::Retry("200 with no Content-Length".to_string()),
                        }
                    }
                    404 => Attempt::Done(None),
                    status => self.classify(status, response),
                },
                Err(error) => Attempt::Retry(format!("{error}")),
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
                                Attempt::Retry(format!(
                                    "read back {} bytes but the response declared {} — a torn or truncated body",
                                    bytes.len(),
                                    declared.unwrap_or_default()
                                ))
                            }
                            Ok(bytes) => Attempt::Done(Some(bytes)),
                            // Retrying it is why the retry ladder survived the move off rclone,
                            // which retried low-level errors internally and no longer does it for us.
                            Err(error) => Attempt::Retry(format!("body: {error}")),
                        }
                    }
                    404 => Attempt::Done(None),
                    status => self.classify(status, response),
                },
                Err(error) => Attempt::Retry(format!("{error}")),
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
                Err(error) => Attempt::Retry(format!("{error}")),
            }
        })
    }

    /// A status this call did not expect, with as much of the body as is useful for a journal line.
    fn classify<T>(&self, status: u16, response: ureq::http::Response<ureq::Body>) -> Attempt<T> {
        let body = response
            .into_body()
            .with_config()
            .limit(4096)
            .read_to_vec()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_string())
            .unwrap_or_default();
        let detail = self
            .redact(&format!("status {status}{}", if body.is_empty() { String::new() } else { format!(": {body}") }));
        if status == 429 || status >= 500 {
            Attempt::Retry(detail)
        } else {
            Attempt::Fatal(detail)
        }
    }

    fn with_retries<T>(&self, key: &str, mut once: impl FnMut(u32) -> Attempt<T>) -> Result<T, String> {
        let mut delay = Duration::from_millis(400);
        let mut last = String::new();
        for attempt in 1..=self.attempts {
            match once(attempt) {
                Attempt::Done(value) => return Ok(value),
                Attempt::Fatal(message) => return Err(format!("{key}: {}", self.redact(&message))),
                Attempt::Retry(message) => {
                    last = message;
                    if attempt < self.attempts {
                        std::thread::sleep(delay);
                        delay = (delay * 3).min(Duration::from_secs(10));
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

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
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
fn host_of(endpoint: &str) -> Result<String, String> {
    let (scheme, rest) = endpoint
        .split_once("://")
        .ok_or_else(|| format!("{endpoint} is not an absolute URL (expected https://host)"))?;
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(format!("{endpoint} has no host"));
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
/// this client speaks and nothing else — no listing, no multipart, no auth check.
#[cfg(test)]
pub(crate) mod double {
    use std::collections::BTreeMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    pub struct Double {
        pub endpoint: String,
        objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
        running: Arc<AtomicBool>,
        /// Declare the real length and then send one byte less — the mid-stream tear this bucket
        /// has a recorded history of.
        truncate: Arc<AtomicBool>,
    }

    impl Double {
        pub fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
            let endpoint = format!("http://{}", listener.local_addr().expect("local addr"));
            let objects: Arc<Mutex<BTreeMap<String, Vec<u8>>>> = Arc::new(Mutex::new(BTreeMap::new()));
            let running = Arc::new(AtomicBool::new(true));
            let truncate = Arc::new(AtomicBool::new(false));
            let (thread_objects, thread_running) = (Arc::clone(&objects), Arc::clone(&running));
            let thread_truncate = Arc::clone(&truncate);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    if !thread_running.load(Ordering::Relaxed) {
                        return;
                    }
                    let Ok(stream) = stream else { return };
                    let objects = Arc::clone(&thread_objects);
                    let truncate = Arc::clone(&thread_truncate);
                    // One thread per connection: the concurrency test needs several at once, which
                    // is the property being tested.
                    std::thread::spawn(move || serve(stream, &objects, truncate.load(Ordering::Relaxed)));
                }
            });
            Self { endpoint, objects, running, truncate }
        }

        pub fn set_truncate_bodies(&self, truncate: bool) {
            self.truncate.store(truncate, Ordering::Relaxed);
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
    }

    impl Drop for Double {
        fn drop(&mut self) {
            self.running.store(false, Ordering::Relaxed);
            // Unblock the accept loop so the thread can observe the flag and exit.
            let _ = TcpStream::connect(self.endpoint.trim_start_matches("http://"));
        }
    }

    fn serve(stream: TcpStream, objects: &Mutex<BTreeMap<String, Vec<u8>>>, truncate: bool) {
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut request = String::new();
        if reader.read_line(&mut request).is_err() || request.is_empty() {
            return;
        }
        let mut parts = request.split_whitespace();
        let (method, path) =
            (parts.next().unwrap_or_default().to_string(), parts.next().unwrap_or_default().to_string());
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().unwrap_or(0);
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
        let mut out = stream;
        let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len());
        let _ = out.write_all(head.as_bytes());
        // A HEAD response carries the length header and no body, which is what makes `S3::head`
        // able to answer without transferring the object.
        if method != "HEAD" {
            let sent = if truncate && !payload.is_empty() { &payload[..payload.len() - 1] } else { &payload[..] };
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

    /// The AWS-documented SigV4 derivation vector. This pins the whole HMAC ladder — get any rung
    /// wrong and every request is a 403 that reads like a bad credential.
    #[test]
    fn the_signing_key_ladder_matches_the_published_vector() {
        let key = signing_key("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", "20150830", "us-east-1");
        // AWS's example derives for `iam`; ours is fixed to `s3`, so re-derive the last two rungs.
        let date_key = hmac_sha256(b"AWS4wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY", b"20150830");
        let region_key = hmac_sha256(&date_key, b"us-east-1");
        let iam_key = hmac_sha256(&hmac_sha256(&region_key, b"iam"), b"aws4_request");
        assert_eq!(hex(&iam_key), "c4afb1cc5771d871763a393e44b703571b55cc28424d1a5e86da6ed3c154a4b9");
        // And the s3 variant this client actually signs with is the same ladder, one label along.
        assert_eq!(key, hmac_sha256(&hmac_sha256(&region_key, b"s3"), b"aws4_request"));
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

    /// The publish phase runs several requests at once and must land every one of them exactly
    /// once — no dropped object, no double-count, no key crossed with another's body.
    #[test]
    fn a_concurrent_batch_lands_every_object_exactly_once() {
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
