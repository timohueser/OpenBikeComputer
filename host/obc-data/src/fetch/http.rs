//! One file over HTTP into the store: retries, a `.part` file that the next attempt resumes with a
//! `Range` request, and a digest check before the file becomes an object.

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::archive::Archive;
use crate::date;
use crate::sources::{Keep, Source};
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
    /// The MD5 of the bytes, in lowercase hex, as upstream publishes it.
    pub md5: Option<&'a str>,
}

/// What a `HEAD` request tells about a file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Head {
    pub bytes: Option<u64>,
    /// A strong `ETag`.
    pub etag: Option<String>,
    /// The `Last-Modified` day.
    pub modified: Option<String>,
    /// The `Last-Modified` time, in seconds since 1970-01-01.
    pub modified_at: Option<u64>,
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
/// The longest time a page of a bucket listing may take: a page of 1,000 keys of a busy service.
/// Chosen, not measured.
const LISTING: Duration = Duration::from_secs(120);
/// The longest wait that a server `Retry-After` gets. A server that asks for more does not answer
/// within the retries of one run, so the download fails sooner and the next run resumes its part.
const RETRY_AFTER: Duration = Duration::from_secs(60);
/// How long a retry wait sleeps between checks of [`STOPPED`].
const SLICE: Duration = Duration::from_millis(100);

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

/// Whether an error of this module says that upstream did not serve: no connection, a timeout, or
/// HTTP 404, 408, 429 or 5xx.
pub fn outage(error: &str) -> bool {
    let status = error.rsplit_once("HTTP ").and_then(|(_, status)| status.parse::<u16>().ok());
    unreachable(error) || status.is_some_and(|status| matches!(status, 404 | 408 | 429 | 500..=599))
}

/// Set when the run of this process stops: a download ends within one read or one [`SLICE`] of a
/// retry wait, and keeps its part.
static STOPPED: AtomicBool = AtomicBool::new(false);

pub fn stop() {
    STOPPED.store(true, Ordering::Relaxed);
}

fn stopped(url: &str) -> Result<(), String> {
    match STOPPED.load(Ordering::Relaxed) {
        true => Err(format!("GET {url}: the run stopped; the next fetch resumes the download")),
        false => Ok(()),
    }
}

/// Sleep for `delay`, or until the run stops.
fn pause(url: &str, delay: Duration) -> Result<(), String> {
    let end = Instant::now() + delay;
    loop {
        stopped(url)?;
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(());
        }
        std::thread::sleep(left.min(SLICE));
    }
}

/// `text` with the query of each URL in it left out: a query can sign the URL, such as a SAS token,
/// and a log line must not show it.
pub fn without_queries(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) =
        rest.find("://").and_then(|at| rest[..at].rfind(char::is_whitespace).map_or(Some(0), |space| Some(space + 1)))
    {
        let end = rest[at..].find(char::is_whitespace).map_or(rest.len(), |end| at + end);
        let token = &rest[at..end];
        out.push_str(&rest[..at]);
        match token.find('?') {
            // A URL before a `: reason` keeps its `:`.
            Some(query) => out.push_str(&format!("{}{}", &token[..query], if token.ends_with(':') { ":" } else { "" })),
            None => out.push_str(token),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
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
    /// Each HTTPS origin, with a trailing `/`, and the token that authorizes its requests.
    bearer: Vec<(String, String)>,
    backoff: Duration,
    archive: Option<Archive>,
    /// The origin, with a trailing `/`, whose requests are signed for S3, and its keys.
    s3: Option<(String, S3Keys)>,
}

/// The keys and the region of an S3 API that signs with AWS Signature Version 4.
pub struct S3Keys {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub region: String,
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
        Self { agent: config.into(), bearer: Vec::new(), backoff: Duration::ZERO, archive: None, s3: None }
    }

    /// A client with the archive of `OBC_DATA_ARCHIVE`.
    pub fn new() -> Self {
        Self { archive: Archive::from_env(), ..Self::with_backoff(Duration::from_secs(2)) }
    }

    /// The first retry waits `backoff`; each later one waits twice as long as the one before. No
    /// archive.
    pub fn with_backoff(backoff: Duration) -> Self {
        let config = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(concat!("OpenBikeComputer obc-data/", env!("CARGO_PKG_VERSION")))
            .timeout_connect(Some(Duration::from_secs(30)))
            .timeout_recv_response(Some(Duration::from_secs(60)))
            .timeout_recv_body(Some(BODY))
            .build();
        Self { agent: config.into(), bearer: Vec::new(), backoff, archive: None, s3: None }
    }

    #[cfg(test)]
    pub(crate) fn with_archive(self, root: &Path) -> Self {
        self.with_roots(vec![root.to_path_buf()])
    }

    #[cfg(test)]
    pub(crate) fn with_roots(mut self, roots: Vec<PathBuf>) -> Self {
        self.archive = Some(Archive::new(roots));
        self
    }

    /// The archive, for a source with `keep = "archive"`: the archive holds that dataset, so its
    /// copy serves before upstream.
    pub fn archive(&self, source: &Source) -> Option<&Archive> {
        self.archive.as_ref().filter(|_| source.keep == Keep::Archive)
    }

    /// The roots of the archive, whatever the source: the homes of the prepared sources are there.
    pub fn roots(&self) -> &[PathBuf] {
        self.archive.as_ref().map_or(&[], Archive::roots)
    }

    /// Authorize only requests to this HTTPS origin. Redirects strip authorization by default.
    pub fn with_bearer(mut self, origin: &str, token: String) -> Self {
        self.bearer.push((format!("{}/", origin.trim_end_matches('/')), token));
        self
    }

    /// Sign the `GET` and `HEAD` requests to this HTTPS origin for S3. A URL must have its path and
    /// query encoded as S3 needs them, and its query sorted by name.
    pub fn with_s3(mut self, origin: &str, keys: S3Keys) -> Self {
        self.s3 = Some((format!("{}/", origin.trim_end_matches('/')), keys));
        self
    }

    fn authorize(
        &self,
        method: &str,
        url: &str,
        mut request: ureq::RequestBuilder<ureq::typestate::WithoutBody>,
    ) -> ureq::RequestBuilder<ureq::typestate::WithoutBody> {
        if let Some((_, token)) = self.bearer.iter().find(|(origin, _)| url.starts_with(origin.as_str())) {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        if let Some((_, keys)) = self.s3.as_ref().filter(|(origin, _)| url.starts_with(origin.as_str())) {
            for (name, value) in crate::r2::s3::sign_url(keys, method, url, date::now()) {
                request = request.header(name, value);
            }
        }
        request
    }

    /// What a `HEAD` request of `url` tells, after redirects. A 404 is an error that [`not_found`]
    /// knows.
    pub fn head(&self, url: &str) -> Result<Head, String> {
        // The size of the bytes as upstream stores them, as a download asks for them.
        let request = self.authorize("HEAD", url, self.agent.head(url)).header("accept-encoding", "identity");
        let response =
            request.config().timeout_global(Some(SMALL)).build().call().map_err(|e| no_answer("HEAD", url, e))?;
        if !response.status().is_success() {
            return Err(format!("HEAD {url}: HTTP {}", response.status().as_u16()));
        }
        Ok(Head {
            bytes: header(&response, "content-length").and_then(|value| value.parse().ok()),
            etag: header(&response, "etag").filter(|value| !value.starts_with("W/")),
            modified: header(&response, "last-modified").and_then(|value| date::from_http(&value)),
            modified_at: header(&response, "last-modified").and_then(|value| date::http_seconds(&value)),
        })
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
        self.answer_within(url, accept, SMALL)
    }

    /// A page of a bucket listing, which can take longer than a small document.
    /// It is tried again like a download after no answer and after HTTP 408, 429 and 5xx.
    pub fn listing(&self, url: &str) -> Result<String, String> {
        self.retried("GET", url, || self.answer_within(url, "application/xml", LISTING))
    }

    /// A page of a JSON listing or an API answer, tried again as [`Http::listing`] is.
    pub fn listing_json(&self, url: &str) -> Result<String, String> {
        self.retried("GET", url, || self.answer_within(url, "application/json", LISTING))
    }

    /// A page of a JSON listing that a `POST` of `body` asks for, tried again as [`Http::listing`]
    /// is.
    pub fn post_listing(&self, url: &str, body: &[u8]) -> Result<String, String> {
        self.retried("POST", url, || {
            let request =
                self.agent.post(url).header("content-type", "application/json").header("accept", "application/json");
            let mut response = request
                .config()
                .timeout_global(Some(LISTING))
                .build()
                .send(body)
                .map_err(|e| no_answer("POST", url, e))?;
            let status = response.status().as_u16();
            match response.body_mut().read_to_vec() {
                Ok(body) => Ok((status, body)),
                // The status of an answer that is no success says enough; a body cut short is
                // tried again.
                Err(_) if !(200..=299).contains(&status) => Ok((status, Vec::new())),
                Err(e) => Err(format!("POST {url}: {e}")),
            }
        })
    }

    fn retried(
        &self,
        method: &str,
        url: &str,
        attempt: impl Fn() -> Result<(u16, Vec<u8>), String>,
    ) -> Result<String, String> {
        let mut failures = 0;
        loop {
            let why = match attempt() {
                Ok((200..=299, body)) => return String::from_utf8(body).map_err(|e| format!("{method} {url}: {e}")),
                Ok((status @ (408 | 429 | 500..=599), _)) => format!("{method} {url}: HTTP {status}"),
                Ok((status, _)) => return Err(format!("{method} {url}: HTTP {status}")),
                Err(why) => why,
            };
            failures += 1;
            if failures == ATTEMPTS {
                return Err(why);
            }
            eprintln!("obc data: {} — retrying", without_queries(&why));
            pause(url, self.backoff * 2u32.pow(failures - 1))?;
        }
    }

    fn answer_within(&self, url: &str, accept: &str, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
        let request = self
            .authorize("GET", url, self.agent.get(url))
            .header("accept", accept)
            .config()
            .timeout_global(Some(timeout))
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
            eprintln!("obc data: {} — retrying", without_queries(&why));
            pause(url, delay.min(RETRY_AFTER).max(self.backoff * 2u32.pow(failures - 1)))?;
        }
        let _ = fs::remove_file(validator);
        let (sha256, size) = match expect.md5 {
            Some(want) => {
                let (sha256, md5, size) = store::digests(part)?;
                if md5 != want {
                    let _ = fs::remove_file(part);
                    return Err(format!("{url}: the MD5 is {md5}, not {want}"));
                }
                (sha256, size)
            }
            None => store::hash_file(part)?,
        };
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
        stopped(url).map_err(Failure::Final)?;
        let have = fs::metadata(part).map_or(0, |metadata| metadata.len());
        let saved = fs::read_to_string(validator).ok();
        // Without a validator or a digest, nothing would tell a changed file from the rest of the old one.
        let resume = have > 0
            && (saved.is_some() || expect.sha256.is_some())
            && expect.etag.is_none_or(|want| saved.as_deref() == Some(want));
        // The bytes as upstream stores them: a decoded body has no ranges and another digest.
        let mut request = self.authorize("GET", url, self.agent.get(url)).header("accept-encoding", "identity");
        if let Some(etag) = expect.etag {
            request = request.header("if-match", etag);
        }
        if resume {
            request = request.header("range", format!("bytes={have}-"));
            if let Some(saved) = &saved {
                request = request.header("if-range", saved);
            }
        }
        let mut response = request.call().map_err(|e| Failure::Retry(no_answer("GET", url, e)))?;
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
        let (mut received, mut reported) = (0u64, std::time::Instant::now());
        let retry = |grew: bool, why: String| if append && grew { Failure::Grew(why) } else { Failure::Retry(why) };
        loop {
            stopped(url).map_err(Failure::Final)?;
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
                        eprintln!(
                            "obc data: {}: {done} / {} downloaded",
                            without_queries(url),
                            crate::engine::runs::bytes(whole)
                        )
                    }
                    None => eprintln!("obc data: {}: {done} downloaded", without_queries(url)),
                }
                reported = std::time::Instant::now();
            }
            file.write_all(&buffer[..read]).map_err(|e| Failure::Final(format!("{}: {e}", part.display())))?;
            received += read as u64;
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
