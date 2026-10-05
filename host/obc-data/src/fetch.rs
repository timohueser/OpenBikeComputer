//! Fetchers: the only code that uses the network. A fetch puts the files of one source version in
//! the store and records them in the snapshot of that version.

pub mod http;
pub mod upstream;

use self::http::{Expect, Http};
use crate::date;
use crate::sources::{FetchKind, Source, VersionScheme};
use crate::store::{FileRecord, Snapshot, Store};

pub struct Request<'a> {
    pub source: &'a Source,
    /// `None` takes the newest file upstream has, which only a URL without `{version}` can give.
    pub version: Option<String>,
    /// A value for each `{name}` of the URL but `{version}`. A name may repeat: one file per value.
    pub params: Vec<(String, String)>,
}

/// Fetch the files of `request` into the store, or find them there. The snapshot that comes back
/// holds the requested files, in the order of the request.
pub fn fetch(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    match source.fetch.kind {
        FetchKind::Http | FetchKind::Geofabrik | FetchKind::Glo30 | FetchKind::Github => files(store, http, request),
        FetchKind::Osm | FetchKind::Dtm | FetchKind::Capture => {
            Err(format!("source `{}`: its kind of fetch has no fetcher yet", source.id))
        }
        FetchKind::ByHand => Err(format!(
            "source `{}`: a person downloads it from {}",
            source.id,
            source.fetch.url.as_deref().unwrap_or_default()
        )),
        FetchKind::Installed => Err(format!("source `{}` is installed, not fetched", source.id)),
    }
}

/// Whether `version` has the form of the source's version scheme. Every version also names a
/// path in the store: segments of letters, digits, `.`, `_`, `+` and `-`, joined by `/`, none
/// starting with `.`.
fn check_version(source: &Source, version: &str) -> Result<(), String> {
    let path = version.split('/').all(|segment| {
        !segment.is_empty()
            && !segment.starts_with('.')
            && segment.chars().all(|c| c.is_ascii_alphanumeric() || "._+-".contains(c))
    });
    let (fits, form) = match source.version {
        VersionScheme::Date => (date::parse(version).is_some(), "a YYYY-MM-DD date"),
        VersionScheme::Digest => {
            (version.len() == 64 && version.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')), "a SHA-256 in hex")
        }
        VersionScheme::Release | VersionScheme::Commit => (path, "letters, digits, `.`, `_`, `+`, `-` and `/`"),
    };
    if fits && path {
        return Ok(());
    }
    Err(format!("source `{}`: the version `{version}` is not {form}", source.id))
}

/// One file per URL. A URL without `{version}` gives only upstream's newest file: a date version
/// `V` is that file when it was last modified on or before `V`. A release or commit version of
/// such a URL is only a name, so it needs a record that pins the bytes.
fn files(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    let template = source.fetch.url.as_deref().unwrap_or_default();
    let named = template.contains("{version}");
    let only_a_name = !named && matches!(source.version, VersionScheme::Release | VersionScheme::Commit);
    if let Some(version) = &request.version {
        check_version(source, version)?;
    } else if named || only_a_name {
        return Err(format!("source `{}` needs a version: give SOURCE@VERSION", source.id));
    }
    let urls = expand(template, request.version.as_deref(), &request.params)?;
    if source.version == VersionScheme::Digest && urls.len() > 1 {
        return Err(format!("source `{}`: a digest version names one file", source.id));
    }
    // The day of the newest file names a date version, so a file already in the store needs no download.
    let version = match &request.version {
        None if source.version == VersionScheme::Date => {
            // One retry, as a HEAD is cheap and a failure stops the whole fetch.
            let days: Result<Vec<_>, _> =
                urls.iter().map(|url| http.modified(url).or_else(|_| http.modified(url))).collect();
            Some(
                days?
                    .into_iter()
                    .map(|day| day.unwrap_or_else(|| date::format(date::today())))
                    .max()
                    .unwrap_or_default(),
            )
        }
        version => version.clone(),
    };
    let mut files = Vec::new();
    for url in &urls {
        let _download = Http::lock(store, url)?;
        let known = match &version {
            Some(version) => store.snapshot(&source.id, version)?.and_then(|snapshot| snapshot.file(url).cloned()),
            None => None,
        };
        if let Some(file) = known.as_ref().filter(|file| store.object(&file.sha256).is_file()) {
            files.push(file.clone());
            continue;
        }
        if only_a_name && known.is_none() {
            return Err(format!(
                "source `{}`: {url} does not name the version, and no snapshot record pins its bytes",
                source.id
            ));
        }
        let version = version.as_deref();
        let expect = Expect {
            sha256: known
                .as_ref()
                .map(|file| file.sha256.as_str())
                .or(version.filter(|_| source.version == VersionScheme::Digest)),
            modified_by: version.filter(|_| source.version == VersionScheme::Date && !named),
        };
        eprintln!("obc data: fetching {url}");
        let got = http.download(store, url, &expect)?;
        let name = url.rsplit('/').next().unwrap_or_default().to_string();
        let file = FileRecord {
            name,
            url: url.clone(),
            size: got.size,
            sha256: got.sha256,
            retrieved: date::timestamp(date::now()),
        };
        record(store, &source.id, version.unwrap_or(&file.sha256), &file)?;
        files.push(file);
    }
    let version = version.unwrap_or_else(|| files[0].sha256.clone());
    Ok(Snapshot { source: source.id.clone(), version, files })
}

/// Add `file` to the snapshot record of the version. A version names one set of bytes, so a
/// record that has the URL with other bytes is an error.
fn record(store: &Store, source: &str, version: &str, file: &FileRecord) -> Result<(), String> {
    let _lock = store.lock(&format!("snapshot-{source}@{version}"))?;
    let mut snapshot = store.snapshot(source, version)?.unwrap_or_else(|| Snapshot {
        source: source.into(),
        version: version.into(),
        files: Vec::new(),
    });
    match snapshot.file(&file.url) {
        Some(old) if old.sha256 != file.sha256 => Err(format!(
            "{source}@{version}: {} now has the SHA-256 {}, but the record has {}",
            file.url, file.sha256, old.sha256
        )),
        Some(_) => Ok(()),
        None => {
            snapshot.files.push(file.clone());
            store.put_snapshot(&snapshot)
        }
    }
}

/// The URLs of `template` with every `{name}` filled: `{version}` from the version, every other
/// name from `params`.
fn expand(template: &str, version: Option<&str>, params: &[(String, String)]) -> Result<Vec<String>, String> {
    let mut names: Vec<&str> = Vec::new();
    for piece in template.split('{').skip(1) {
        let name = piece.split_once('}').map_or(piece, |(name, _)| name);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    if let Some((name, _)) = params.iter().find(|(name, _)| name == "version" || !names.contains(&name.as_str())) {
        return Err(format!("`{name}=` names no placeholder of {template}"));
    }
    let mut urls = vec![template.to_string()];
    for name in names {
        let values: Vec<&str> = match name {
            "version" => version.into_iter().collect(),
            _ => params.iter().filter(|(n, _)| n == name).map(|(_, value)| value.as_str()).collect(),
        };
        if values.is_empty() {
            return Err(format!("{template} needs a value for `{{{name}}}`: give {name}=VALUE"));
        }
        let placeholder = format!("{{{name}}}");
        urls = urls.iter().flat_map(|url| values.iter().map(|value| url.replace(&placeholder, value))).collect();
    }
    Ok(urls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::upstream::Upstream;
    use crate::sources::parse_sources;
    use crate::store::tests::Scratch;
    use crate::store::{hash_file, sha256_hex};
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// The headers of each request the test server saw, in order.
    type Log = Arc<Mutex<Vec<Vec<(String, String)>>>>;

    /// What the test server answers: the status, the headers, the body it sends, and a
    /// `Content-Length` that may promise more than it sends.
    struct Reply {
        status: u16,
        headers: Vec<(&'static str, String)>,
        body: Vec<u8>,
        length: usize,
    }

    /// An HTTP/1.1 server on 127.0.0.1 that logs the headers of each request and answers with
    /// `reply`. It runs until the test process ends.
    fn serve(reply: impl Fn(usize, &[(String, String)]) -> Reply + Send + 'static) -> (String, Log) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/data/file.bin", listener.local_addr().unwrap());
        let log = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&log);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut headers = Vec::new();
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                loop {
                    line.clear();
                    reader.read_line(&mut line).unwrap();
                    match line.trim_end().split_once(':') {
                        Some((name, value)) => headers.push((name.to_ascii_lowercase(), value.trim().to_string())),
                        None => break,
                    }
                }
                let index = {
                    let mut log = seen.lock().unwrap();
                    log.push(headers.clone());
                    log.len() - 1
                };
                let reply = reply(index, &headers);
                let mut head =
                    format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\n", reply.status, reply.length);
                for (name, value) in &reply.headers {
                    head += &format!("{name}: {value}\r\n");
                }
                let _ = stream.write_all(format!("{head}\r\n").as_bytes());
                let _ = stream.write_all(&reply.body);
            }
        });
        (url, log)
    }

    fn whole(body: &[u8]) -> Reply {
        let headers = vec![("ETag", "\"v1\"".to_string()), ("Last-Modified", "Mon, 05 Oct 2026 03:43:59 GMT".into())];
        Reply { status: 200, headers, body: body.to_vec(), length: body.len() }
    }

    fn source(url: &str, version: &str) -> Source {
        let text = format!(
            "[[source]]\nid = \"land\"\nkind = \"data\"\nfetch = {{ kind = \"http\", url = \"https://example.org/x\" }}\n\
             version = \"{version}\"\nrefresh = \"manual\"\nredistribute = true\n"
        );
        let mut source = parse_sources(&text).unwrap().remove(0);
        source.fetch.url = Some(url.to_string());
        source
    }

    fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
        headers.iter().find(|(n, _)| n == name).map(|(_, value)| value.as_str())
    }

    fn quick() -> Http {
        Http::with_backoff(Duration::ZERO)
    }

    #[test]
    fn an_interrupted_fetch_resumes_from_the_part_file() {
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let served = body.clone();
        let (url, log) = serve(move |index, _| match index {
            // The connection closes after 40 000 of the promised bytes.
            0 => Reply { body: served[..40_000].to_vec(), ..whole(&served) },
            _ => {
                let mut reply = whole(&served[40_000..]);
                reply.status = 206;
                reply.headers.push(("Content-Range", "bytes 40000-99999/100000".into()));
                reply
            }
        });
        let scratch = Scratch::new("resume");
        let store = Store::at(&scratch.0);
        let land = source(&url, "date");
        let request = Request { source: &land, version: Some("2026-10-05".into()), params: vec![] };
        let snapshot = fetch(&store, &quick(), &request).unwrap();
        let log = log.lock().unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!((header(&log[1], "range"), header(&log[1], "if-range")), (Some("bytes=40000-"), Some("\"v1\"")));
        let file = &snapshot.files[0];
        assert_eq!((file.sha256.as_str(), file.size), (sha256_hex(&body).as_str(), 100_000));
        assert_eq!(hash_file(&store.object(&file.sha256)).unwrap().0, file.sha256);
        assert_eq!(store.snapshot("land", "2026-10-05").unwrap().as_ref(), Some(&snapshot));
    }

    #[test]
    fn a_whole_answer_to_a_resume_replaces_the_part_file() {
        let old: Vec<u8> = vec![1; 100_000];
        let new: Vec<u8> = vec![2; 60_000];
        let served = new.clone();
        let (url, _log) = serve(move |index, _| match index {
            0 => Reply { body: old[..40_000].to_vec(), ..whole(&old) },
            // Upstream changed between the tries, so If-Range gets the whole new file.
            _ => Reply { headers: vec![("ETag", "\"v2\"".into())], ..whole(&served) },
        });
        let scratch = Scratch::new("restart");
        let store = Store::at(&scratch.0);
        let land = source(&url, "date");
        let request = Request { source: &land, version: Some("2026-10-05".into()), params: vec![] };
        let file = fetch(&store, &quick(), &request).unwrap().files.remove(0);
        assert_eq!((file.sha256, file.size), (sha256_hex(&new), 60_000));
    }

    #[test]
    fn a_wrong_digest_fails_and_leaves_no_object() {
        let (url, _log) = serve(|_, _| whole(b"tampered"));
        let scratch = Scratch::new("digest");
        let store = Store::at(&scratch.0);
        let pgf = source(&url, "digest");
        let want = sha256_hex(b"expected");
        let request = Request { source: &pgf, version: Some(want), params: vec![] };
        let err = fetch(&store, &quick(), &request).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        let left: Vec<_> = walk(&scratch.0).into_iter().filter(|p| !p.starts_with(scratch.0.join("locks"))).collect();
        assert!(left.is_empty(), "no object, part or snapshot stays: {left:?}");
    }

    #[test]
    fn a_fetched_version_comes_from_the_store() {
        let (url, log) = serve(|_, _| whole(b"tile bytes"));
        let scratch = Scratch::new("hit");
        let store = Store::at(&scratch.0);
        let named = source(&url.replace("file.bin", "{version}.bin"), "release");
        let request = Request { source: &named, version: Some("v1".into()), params: vec![] };
        let first = fetch(&store, &quick(), &request).unwrap();
        assert_eq!(fetch(&store, &quick(), &request).unwrap(), first);
        assert_eq!(log.lock().unwrap().len(), 1, "the second fetch asks upstream nothing");
        // Without a version, one HEAD names the date version, and the store has it.
        let land = source(&url, "date");
        let newest = Request { source: &land, version: None, params: vec![] };
        let dated = fetch(&store, &quick(), &newest).unwrap();
        assert_eq!(dated.version, "2026-10-05");
        assert_eq!(fetch(&store, &quick(), &newest).unwrap(), dated);
        assert_eq!(log.lock().unwrap().len(), 4, "HEAD and GET, then HEAD only");
    }

    #[test]
    fn a_release_version_of_a_url_that_names_none_is_refused() {
        let (url, log) = serve(|_, _| whole(b"today's bytes"));
        let scratch = Scratch::new("name");
        let store = Store::at(&scratch.0);
        let natural_earth = source(&url, "release");
        let request = Request { source: &natural_earth, version: Some("5.1.2".into()), params: vec![] };
        let err = fetch(&store, &quick(), &request).unwrap_err();
        assert!(err.contains("no snapshot record pins its bytes"), "{err}");
        assert!(log.lock().unwrap().is_empty());
    }

    #[test]
    fn an_upstream_check_is_kept_for_an_hour() {
        let (url, log) = serve(|_, _| whole(b""));
        let scratch = Scratch::new("upstream");
        let store = Store::at(&scratch.0);
        let land = source(&url, "date");
        let newest = Upstream::Newest("2026-10-05".into());
        assert_eq!(upstream::newest(&store, &quick(), &land, upstream::CACHE), newest);
        assert_eq!(upstream::newest(&store, &quick(), &land, upstream::CACHE), newest);
        assert_eq!(log.lock().unwrap().len(), 1);
        assert_eq!(upstream::newest(&store, &quick(), &land, 0), newest);
        assert_eq!(log.lock().unwrap().len(), 2, "an older answer than asked for is checked again");
    }

    #[test]
    fn a_newest_only_url_refuses_an_older_date_version() {
        let (url, log) = serve(|_, _| whole(b"polygons"));
        let scratch = Scratch::new("older");
        let store = Store::at(&scratch.0);
        let land = source(&url, "date");
        let request = Request { source: &land, version: Some("2026-08-05".into()), params: vec![] };
        let err = fetch(&store, &quick(), &request).unwrap_err();
        assert!(err.contains("changed on 2026-10-05"), "{err}");
        assert_eq!(log.lock().unwrap().len(), 1, "it does not retry");
    }

    #[test]
    fn placeholders_take_the_version_and_one_url_per_value() {
        let params =
            |pairs: &[(&str, &str)]| pairs.iter().map(|(n, v)| (n.to_string(), v.to_string())).collect::<Vec<_>>();
        let urls =
            expand("https://h/{tile}/{tile}-{version}.tif", Some("v1"), &params(&[("tile", "a"), ("tile", "b")]));
        assert_eq!(urls.unwrap(), ["https://h/a/a-v1.tif", "https://h/b/b-v1.tif"]);
        assert!(expand("https://h/{area}.poly", None, &[]).unwrap_err().contains("area=VALUE"));
        assert!(expand("https://h/x", None, &params(&[("tile", "a")])).unwrap_err().contains("names no placeholder"));
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files.extend(walk(&path));
            } else {
                files.push(path);
            }
        }
        files
    }
}
