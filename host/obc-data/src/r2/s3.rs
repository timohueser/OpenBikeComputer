//! An R2 target. It is read through the HTTPS origin that serves the keys of its bucket, and written
//! with the credentials of `OBC_R2_*`: a small document through the S3 API, signed with AWS
//! Signature Version 4 and with a conditional write, and the files of a release with rclone
//! ([`crate::r2::Bucket`]). A target can be a folder of its bucket, such as a test.

use std::time::Duration;

use sha2::{Digest, Sha256};

use super::{Account, Bucket, Credentials};
use crate::target::{Condition, Tag};

/// An R2 target: the origin that serves the keys of its bucket, and the folder of the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Remote {
    /// `https://` and a host, without a path.
    pub public: String,
    /// The folder of the bucket that holds the target, without `/` at either end; empty for the
    /// root of the bucket.
    pub prefix: String,
}

/// The size, tag and time of an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub size: u64,
    pub tag: Tag,
    /// `Last-Modified`, in seconds since 1970: the time of the server.
    pub modified: u64,
}

impl Remote {
    /// The key of the bucket that holds `key` of the target.
    pub fn key(&self, key: &str) -> String {
        match self.prefix.as_str() {
            "" => key.into(),
            prefix => format!("{prefix}/{key}"),
        }
    }

    /// The public URL of `key`: each part of the key percent-encoded, so `#` is no fragment.
    pub fn url(&self, key: &str) -> String {
        format!("{}/{}", self.public, encode(&self.key(key), false))
    }

    /// The writer of the target, with the credentials of `OBC_R2_*`.
    pub fn writer(&self) -> Result<Writer, String> {
        let account = Account::from_env(Credentials::Main)?;
        let agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .user_agent(concat!("OpenBikeComputer obc-data/", env!("CARGO_PKG_VERSION")))
            .timeout_global(Some(Duration::from_secs(60)))
            .build()
            .into();
        Ok(Writer { remote: self.clone(), account, agent })
    }
}

/// Writes an R2 target.
pub struct Writer {
    remote: Remote,
    account: Account,
    agent: ureq::Agent,
}

impl Writer {
    /// The bytes at `key` and their tag, or `None` when the bucket has no such key.
    pub fn get(&self, key: &str) -> Result<Option<(Vec<u8>, Tag)>, String> {
        let mut response = self.send("GET", key, Vec::new(), &[])?;
        match response.status().as_u16() {
            404 => Ok(None),
            200 => {
                let tag = tag(&response).ok_or_else(|| format!("R2 GET {key}: no ETag"))?;
                let bytes = response.body_mut().read_to_vec().map_err(|e| format!("R2 GET {key}: {e}"))?;
                Ok(Some((bytes, tag)))
            }
            _ => Err(failed("GET", key, response)),
        }
    }

    /// The object at `key`, or `None`, and the time of the server.
    pub fn head(&self, key: &str) -> Result<(Option<Head>, u64), String> {
        let response = self.send("HEAD", key, Vec::new(), &[])?;
        let date = header(&response, "date").and_then(|date| crate::date::http_seconds(&date));
        let date = date.ok_or_else(|| format!("R2 HEAD {key}: no Date"))?;
        match response.status().as_u16() {
            404 => Ok((None, date)),
            200 => {
                let size = header(&response, "content-length").and_then(|size| size.parse().ok());
                let modified = header(&response, "last-modified").and_then(|time| crate::date::http_seconds(&time));
                let tag = tag(&response);
                let (Some(size), Some(modified), Some(tag)) = (size, modified, tag) else {
                    return Err(format!("R2 HEAD {key}: no Content-Length, Last-Modified or ETag"));
                };
                Ok((Some(Head { size, tag, modified }), date))
            }
            _ => Err(failed("HEAD", key, response)),
        }
    }

    /// Write `bytes` at `key` when `condition` holds, and give their tag. `None` when it does not
    /// hold: nothing changed.
    pub fn put(&self, key: &str, bytes: &[u8], condition: &Condition) -> Result<Option<Tag>, String> {
        let mut headers = vec![("cache-control".to_string(), cache_control(key).to_string())];
        headers.extend(precondition(condition));
        let response = self.send("PUT", key, headers, bytes)?;
        match response.status().as_u16() {
            // A key that holds bytes for `Absent` can also answer 409 while another write runs.
            412 | 409 => Ok(None),
            200 => tag(&response).map(Some).ok_or_else(|| format!("R2 PUT {key}: no ETag")),
            _ => Err(failed("PUT", key, response)),
        }
    }

    /// Delete `key`. A key that the bucket lacks is no error.
    pub fn delete(&self, key: &str) -> Result<(), String> {
        let response = self.send("DELETE", key, Vec::new(), &[])?;
        match response.status().as_u16() {
            200 | 204 | 404 => Ok(()),
            _ => Err(failed("DELETE", key, response)),
        }
    }

    /// The rclone client of the folder of the target, for its files.
    pub fn bucket(&self) -> Result<Bucket, String> {
        Ok(Bucket::from_env(Credentials::Main)?.within(&self.remote.prefix))
    }

    fn send(
        &self,
        method: &'static str,
        key: &str,
        headers: Vec<(String, String)>,
        body: &[u8],
    ) -> Result<ureq::http::Response<ureq::Body>, String> {
        super::check_key(key)?;
        let signed = request(&self.account, method, &self.remote.key(key), headers, body, crate::date::now());
        let mut builder = ureq::http::Request::builder().method(method).uri(&signed.url);
        for (name, value) in &signed.headers {
            builder = builder.header(name, value);
        }
        let request = builder.body(body.to_vec()).map_err(|e| format!("R2 {method} {key}: {e}"))?;
        self.agent.run(request).map_err(|e| format!("R2 {method} {key}: {e}"))
    }
}

/// A request to the S3 API, signed: its URL, and the headers besides `Host`.
#[derive(Debug, PartialEq, Eq)]
struct Signed {
    url: String,
    headers: Vec<(String, String)>,
}

/// The request `method` of the bucket key `key`, with `headers` (lowercase names) and `payload`,
/// signed at the time `now`.
fn request(
    account: &Account,
    method: &str,
    key: &str,
    mut headers: Vec<(String, String)>,
    payload: &[u8],
    now: u64,
) -> Signed {
    let host = account.endpoint.trim_start_matches("https://").trim_end_matches('/');
    let path = format!("/{}/{}", encode(&account.bucket, false), encode(key, false));
    let time = crate::date::timestamp(now).replace(['-', ':'], "");
    let payload = hex(&Sha256::digest(payload));
    headers.extend([
        ("host".to_string(), host.to_string()),
        ("x-amz-content-sha256".to_string(), payload.clone()),
        ("x-amz-date".to_string(), time.clone()),
    ]);
    headers.sort();
    let signing = Signing {
        access_key_id: &account.access_key_id,
        secret_access_key: &account.secret_access_key,
        region: "auto",
        service: "s3",
    };
    let authorization = sign(&signing, method, &path, "", &headers, &payload, &time);
    headers.retain(|(name, _)| name != "host");
    headers.push(("authorization".to_string(), authorization));
    Signed { url: format!("https://{host}{path}"), headers }
}

/// What signs a request.
struct Signing<'a> {
    access_key_id: &'a str,
    secret_access_key: &'a str,
    region: &'a str,
    service: &'a str,
}

/// The `Authorization` of a request, by AWS Signature Version 4. `path` and `query` are encoded;
/// `headers` are the signed headers, with lowercase names, sorted by name; `time` is
/// `YYYYMMDDTHHMMSSZ`.
fn sign(
    signing: &Signing,
    method: &str,
    path: &str,
    query: &str,
    headers: &[(String, String)],
    payload: &str,
    time: &str,
) -> String {
    let names = headers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(";");
    let lines: String = headers.iter().map(|(name, value)| format!("{name}:{}\n", value.trim())).collect();
    let canonical = format!("{method}\n{path}\n{query}\n{lines}\n{names}\n{payload}");
    let day = &time[..8];
    let scope = format!("{day}/{}/{}/aws4_request", signing.region, signing.service);
    let text = format!("AWS4-HMAC-SHA256\n{time}\n{scope}\n{}", hex(&Sha256::digest(canonical)));
    let mut key = hmac(format!("AWS4{}", signing.secret_access_key).as_bytes(), day.as_bytes());
    for part in [signing.region, signing.service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, text.as_bytes()));
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={names}, Signature={signature}",
        signing.access_key_id
    )
}

/// HMAC-SHA256, as RFC 2104 defines it.
fn hmac(key: &[u8], message: &[u8]) -> [u8; 32] {
    let mut block = [0u8; 64];
    match key.len() {
        0..=64 => block[..key.len()].copy_from_slice(key),
        _ => block[..32].copy_from_slice(&Sha256::digest(key)),
    }
    let padded = |byte: u8| block.map(|b| b ^ byte);
    let inner = Sha256::new().chain_update(padded(0x36)).chain_update(message).finalize();
    Sha256::new().chain_update(padded(0x5c)).chain_update(inner).finalize().into()
}

/// `text` with each byte but `A-Z a-z 0-9 - _ . ~` percent-encoded, as S3 and a URL need it: also
/// `/` when `slash`.
fn encode(text: &str, slash: bool) -> String {
    let mut encoded = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => encoded.push(byte as char),
            b'/' if !slash => encoded.push('/'),
            _ => encoded += &format!("%{byte:02X}"),
        }
    }
    encoded
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The header of a conditional write.
fn precondition(condition: &Condition) -> Option<(String, String)> {
    match condition {
        Condition::Any => None,
        Condition::Absent => Some(("if-none-match".into(), "*".into())),
        Condition::Match(Tag(etag)) => Some(("if-match".into(), etag.clone())),
    }
}

/// The caching of a key that never changes.
pub const IMMUTABLE: &str = "public, max-age=31536000, immutable";

/// The caching of a key: `release.json`, `planner/next.json` and `lock.json` change at any time, a
/// pointer each update, and every other key never.
fn cache_control(key: &str) -> &'static str {
    match key {
        crate::release::SERVING | crate::release::NEXT | crate::lock::LOCK => "no-store",
        key if crate::release::POINTERS.contains(&key) => "public, max-age=60, must-revalidate",
        _ => IMMUTABLE,
    }
}

fn header<B>(response: &ureq::http::Response<B>, name: &str) -> Option<String> {
    response.headers().get(name)?.to_str().ok().map(str::to_string)
}

fn tag<B>(response: &ureq::http::Response<B>) -> Option<Tag> {
    header(response, "etag").map(Tag)
}

/// The error of an answer: its status and the code of S3, never a credential.
fn failed(method: &str, key: &str, mut response: ureq::http::Response<ureq::Body>) -> String {
    let status = response.status().as_u16();
    let body = response.body_mut().read_to_string().unwrap_or_default();
    let code = body.split("<Code>").nth(1).and_then(|rest| rest.split("</Code>").next()).unwrap_or_default();
    format!("R2 {method} {key}: HTTP {status} {code}").trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex_bytes(text: &str) -> Vec<u8> {
        (0..text.len()).step_by(2).map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap()).collect()
    }

    #[test]
    fn hmac_sha256_gives_the_values_of_rfc_4231() {
        let cases: [(Vec<u8>, &[u8], &str); 3] = [
            (vec![0x0b; 20], b"Hi There", "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"),
            (
                b"Jefe".to_vec(),
                b"what do ya want for nothing?",
                "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843",
            ),
            (
                vec![0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First",
                "60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54",
            ),
        ];
        for (key, message, expected) in cases {
            assert_eq!(hmac(&key, message).to_vec(), hex_bytes(expected));
        }
    }

    /// Cases of the AWS Signature Version 4 test suite.
    #[test]
    fn a_signature_is_the_one_of_the_aws_test_suite() {
        let signing = Signing {
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
            service: "service",
        };
        let empty = hex(&Sha256::digest(b""));
        let headers = |extra: &[(&str, &str)]| -> Vec<(String, String)> {
            let mut headers: Vec<(String, String)> =
                extra.iter().map(|(name, value)| (name.to_string(), value.to_string())).collect();
            headers.push(("host".into(), "example.amazonaws.com".into()));
            headers.push(("x-amz-date".into(), "20150830T123600Z".into()));
            headers.sort();
            headers
        };
        let signature = |method, path, query, headers: &[(String, String)], payload: &str| {
            let authorization = sign(&signing, method, path, query, headers, payload, "20150830T123600Z");
            authorization.rsplit_once("Signature=").unwrap().1.to_string()
        };
        let cases = [
            (
                "get-vanilla",
                signature("GET", "/", "", &headers(&[]), &empty),
                "5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31",
            ),
            (
                "get-vanilla-query-order-key-case",
                signature("GET", "/", "Param1=value1&Param2=value2", &headers(&[]), &empty),
                "b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500",
            ),
            (
                "get-utf8",
                signature("GET", &format!("/{}", encode("ሴ", false)), "", &headers(&[]), &empty),
                "8318018e0b0f223aa2bbf98705b62bb787dc9c0e678f255a891fd03141be5d85",
            ),
            (
                "post-x-www-form-urlencoded",
                signature(
                    "POST",
                    "/",
                    "",
                    &headers(&[("content-type", "application/x-www-form-urlencoded")]),
                    &hex(&Sha256::digest(b"Param1=value1")),
                ),
                "ff11897932ad3f4e8b18135d722051e5ac45fc38421b1da7b9d196a0fe09473a",
            ),
        ];
        for (case, got, expected) in cases {
            assert_eq!(got, expected, "{case}");
        }
    }

    /// The example "GET Object" of the S3 documentation of Signature Version 4.
    #[test]
    fn a_signature_of_s3_is_the_one_of_its_documentation() {
        let signing = Signing {
            access_key_id: "AKIAIOSFODNN7EXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
            region: "us-east-1",
            service: "s3",
        };
        let empty = hex(&Sha256::digest(b""));
        let headers: Vec<(String, String)> = [
            ("host", "examplebucket.s3.amazonaws.com"),
            ("range", "bytes=0-9"),
            ("x-amz-content-sha256", &empty),
            ("x-amz-date", "20130524T000000Z"),
        ]
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .into();
        let authorization = sign(&signing, "GET", "/test.txt", "", &headers, &empty, "20130524T000000Z");
        assert_eq!(
            authorization,
            "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20130524/us-east-1/s3/aws4_request, \
             SignedHeaders=host;range;x-amz-content-sha256;x-amz-date, \
             Signature=f0e8bdb87c964420e857bd35b5d6ed310bd44f0170aba48dd91039c6036bdb41"
        );
    }

    #[test]
    fn a_request_names_the_key_of_the_folder_encoded_and_signs_its_condition() {
        let account = Account {
            endpoint: "https://account.r2.cloudflarestorage.com".into(),
            bucket: "maps".into(),
            access_key_id: "AKID".into(),
            secret_access_key: "secret".into(),
        };
        let remote = Remote { public: "https://maps.example".into(), prefix: "test/run-1".into() };
        let key = remote.key("sources/head/1/a#b c.txt");
        assert_eq!(key, "test/run-1/sources/head/1/a#b c.txt");
        assert_eq!(
            remote.url("sources/head/1/a#b c.txt"),
            "https://maps.example/test/run-1/sources/head/1/a%23b%20c.txt"
        );

        let condition = precondition(&Condition::Match(Tag("\"abc\"".into()))).into_iter().collect();
        let signed = request(&account, "PUT", &key, condition, b"{}", 1_440_938_160);
        assert_eq!(signed.url, "https://account.r2.cloudflarestorage.com/maps/test/run-1/sources/head/1/a%23b%20c.txt");
        let names: Vec<&str> = signed.headers.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["if-match", "x-amz-content-sha256", "x-amz-date", "authorization"]);
        assert_eq!(signed.headers[2].1, "20150830T123600Z");
        assert!(signed.headers[3].1.contains("/20150830/auto/s3/aws4_request, SignedHeaders=host;if-match;"));

        assert_eq!(precondition(&Condition::Absent), Some(("if-none-match".into(), "*".into())));
        assert_eq!(precondition(&Condition::Any), None);
        assert_eq!(cache_control("lock.json"), "no-store");
        assert_eq!(cache_control("planner/next.json"), "no-store");
        assert_eq!(cache_control("objects/ab"), IMMUTABLE);
    }
}
