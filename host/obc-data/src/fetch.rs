//! Fetchers: the only code that uses the network. A fetch puts the files of one source version in
//! the store and records them in the snapshot of that version.

pub mod capture;
pub mod http;
pub mod osm;
pub mod upstream;

use self::http::{Expect, Http};
use crate::date;
use crate::sources::{FetchKind, Source, VersionScheme};
use crate::store::{FileRecord, Snapshot, Store};

pub struct Request<'a> {
    pub source: &'a Source,
    /// `None` takes the newest version upstream has. A URL of the `http` fetcher with `{version}`
    /// or `{yymmdd}` cannot give it; the `geofabrik` and `osm` fetchers find the newest day.
    pub version: Option<String>,
    /// A value for each `{name}` of the URL but `{version}` and `{yymmdd}`, or the `NAME=VALUE` of a
    /// program fetcher. A name may repeat: one file per value.
    pub params: Vec<(String, String)>,
}

/// Fetch the files of `request` into the store, or find them there. The snapshot that comes back
/// holds the requested files, in the order of the request.
pub fn fetch(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    match source.fetch.kind {
        FetchKind::Http | FetchKind::Glo30 | FetchKind::Github => files(store, http, request),
        FetchKind::Geofabrik => osm::extract(store, http, request),
        FetchKind::Osm => osm::replication(store, http, request),
        FetchKind::Dtm => capture::dtm(store, request),
        FetchKind::Capture => Err(format!("source `{}`: its kind of fetch has no fetcher yet", source.id)),
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

/// One file per URL. A URL without `{version}` or `{yymmdd}` gives only upstream's newest file: a
/// date version `V` is that file when it was last modified on or before `V`. A release or commit
/// version of such a URL is only a name, so it needs a record that pins the bytes.
fn files(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    let template = source.fetch.url.as_deref().unwrap_or_default();
    let named = names_version(template);
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
    let modified_by = version.as_deref().filter(|_| source.version == VersionScheme::Date && !named);
    let files = urls
        .iter()
        .map(|url| get(store, http, source, version.as_deref(), url, only_a_name, modified_by))
        .collect::<Result<Vec<_>, _>>()?;
    let version = version.unwrap_or_else(|| files[0].sha256.clone());
    Ok(Snapshot { source: source.id.clone(), version, files })
}

/// The file at `url` of `version`: from the store when the record of the version has it, or else
/// downloaded and added to that record. `pinned_only` refuses a file that no record pins.
fn get(
    store: &Store,
    http: &Http,
    source: &Source,
    version: Option<&str>,
    url: &str,
    pinned_only: bool,
    modified_by: Option<&str>,
) -> Result<FileRecord, String> {
    let _download = Http::lock(store, url)?;
    let known = match version {
        Some(version) => store.snapshot(&source.id, version)?.and_then(|snapshot| snapshot.file(url).cloned()),
        None => None,
    };
    if let Some(file) = known.as_ref().filter(|file| store.object(&file.sha256).is_file()) {
        return Ok(file.clone());
    }
    if pinned_only && known.is_none() {
        return Err(format!(
            "source `{}`: {url} does not name the version, and no snapshot record pins its bytes",
            source.id
        ));
    }
    let expect = Expect {
        sha256: known
            .as_ref()
            .map(|file| file.sha256.as_str())
            .or(version.filter(|_| source.version == VersionScheme::Digest)),
        modified_by,
    };
    eprintln!("obc data: fetching {url}");
    let got = http.download(store, url, &expect)?;
    let file = FileRecord {
        name: url.rsplit('/').next().unwrap_or_default().to_string(),
        url: url.to_string(),
        size: got.size,
        sha256: got.sha256,
        retrieved: date::timestamp(date::now()),
    };
    record(store, &source.id, version.unwrap_or(&file.sha256), std::slice::from_ref(&file))?;
    Ok(file)
}

/// Add `files` to the snapshot record of the version. A version names one set of bytes, so a
/// record that has a URL with other bytes is an error.
fn record(store: &Store, source: &str, version: &str, files: &[FileRecord]) -> Result<(), String> {
    let _lock = store.lock(&format!("snapshot-{source}@{version}"))?;
    let mut snapshot = store.snapshot(source, version)?.unwrap_or_else(|| Snapshot {
        source: source.into(),
        version: version.into(),
        files: Vec::new(),
    });
    let before = snapshot.files.len();
    for file in files {
        match snapshot.file(&file.url) {
            Some(old) if old.sha256 != file.sha256 => {
                return Err(format!(
                    "{source}@{version}: {} now has the SHA-256 {}, but the record has {}",
                    file.url, file.sha256, old.sha256
                ))
            }
            Some(_) => {}
            None => snapshot.files.push(file.clone()),
        }
    }
    if snapshot.files.len() == before {
        return Ok(());
    }
    store.put_snapshot(&snapshot)
}

/// Whether a URL template names the version: `{version}`, or `{yymmdd}` for a date.
fn names_version(template: &str) -> bool {
    template.contains("{version}") || template.contains("{yymmdd}")
}

/// The URLs of `template` with every `{name}` filled: `{version}` from the version, `{yymmdd}`
/// from a date version, every other name from `params`.
fn expand(template: &str, version: Option<&str>, params: &[(String, String)]) -> Result<Vec<String>, String> {
    let mut names: Vec<&str> = Vec::new();
    for piece in template.split('{').skip(1) {
        let name = piece.split_once('}').map_or(piece, |(name, _)| name);
        if !names.contains(&name) {
            names.push(name);
        }
    }
    let fixed = |name: &str| name == "version" || name == "yymmdd";
    if let Some((name, _)) = params.iter().find(|(name, _)| fixed(name) || !names.contains(&name.as_str())) {
        return Err(format!("`{name}=` names no placeholder of {template}"));
    }
    let mut urls = vec![template.to_string()];
    for name in names {
        let values: Vec<String> = match name {
            "version" => version.map(str::to_string).into_iter().collect(),
            "yymmdd" => {
                version.filter(|v| date::parse(v).is_some()).map(|v| v[2..].replace('-', "")).into_iter().collect()
            }
            _ => params.iter().filter(|(n, _)| n == name).map(|(_, value)| value.clone()).collect(),
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
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                // The request path, as HTTP/2 names it.
                let path = line.split_whitespace().nth(1).unwrap_or_default().to_string();
                let mut headers = vec![(":path".to_string(), path)];
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

    /// A source of `kind`, with its URL at the test server.
    fn located(kind: FetchKind, url: &str) -> Source {
        let mut source = source(url, "date");
        source.fetch.kind = kind;
        source
    }

    fn not_found() -> Reply {
        Reply { status: 404, headers: vec![], body: vec![], length: 0 }
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
        assert_eq!(expand("https://h/p-{yymmdd}.pbf", Some("2026-09-28"), &[]).unwrap(), ["https://h/p-260928.pbf"]);
    }

    /// A replication directory at the test server, with the daily diff of each `(sequence, day)`.
    /// Its `state.txt` is the last of them.
    fn replication(diffs: &'static [(u32, &'static str)]) -> (Source, Log) {
        let state = |&(sequence, day): &(u32, &str)| {
            whole(format!("sequenceNumber={sequence}\ntimestamp={day}T00\\:00\\:00Z\n").as_bytes())
        };
        let (url, log) = serve(move |_, headers| {
            let path = header(headers, ":path").unwrap_or_default();
            if path == "/replication/day/state.txt" {
                return state(diffs.last().unwrap());
            }
            for diff in diffs {
                let stem = format!("/replication/day/000/005/{}", diff.0 - 5000);
                if path == format!("{stem}.state.txt") {
                    return state(diff);
                }
                if path == format!("{stem}.osc.gz") {
                    return whole(path.as_bytes());
                }
            }
            not_found()
        });
        (located(FetchKind::Osm, &url.replace("data/file.bin", "replication/day/")), log)
    }

    fn from(day: &str) -> Vec<(String, String)> {
        vec![("from".into(), day.into())]
    }

    #[test]
    fn replication_is_the_daily_diffs_after_the_base_up_to_the_version() {
        let (diffs, log) = replication(&[
            (5128, "2026-09-27"),
            (5129, "2026-09-28"),
            (5130, "2026-09-29"),
            (5131, "2026-09-30"),
            (5132, "2026-10-01"),
        ]);
        let scratch = Scratch::new("replication");
        let store = Store::at(&scratch.0);
        let wednesday = Request { source: &diffs, version: Some("2026-09-30".into()), params: from("2026-09-28") };
        let snapshot = fetch(&store, &quick(), &wednesday).unwrap();
        let names: Vec<_> = snapshot.files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, ["130.osc.gz", "131.osc.gz"]);
        let asked = log.lock().unwrap().len();
        assert_eq!(fetch(&store, &quick(), &wednesday).unwrap(), snapshot);
        let later = Request { params: from("2026-09-29"), ..wednesday };
        assert_eq!(fetch(&store, &quick(), &later).unwrap().files, snapshot.files[1..]);
        assert_eq!(log.lock().unwrap().len(), asked, "a record that has the diffs needs no request");
        // A refresh keeps the base and downloads the diff of the new day only.
        let refresh = Request { source: &diffs, version: None, params: from("2026-09-28") };
        let refreshed = fetch(&store, &quick(), &refresh).unwrap();
        assert_eq!(refreshed.version, "2026-10-01");
        assert_eq!(refreshed.files[..2], snapshot.files);
        let log = log.lock().unwrap();
        let downloads: Vec<_> =
            log[asked..].iter().filter_map(|h| header(h, ":path")).filter(|p| p.ends_with(".osc.gz")).collect();
        assert_eq!(downloads, ["/replication/day/000/005/132.osc.gz"]);
    }

    #[test]
    fn replication_needs_a_base_and_one_diff_per_day() {
        // 2026-10-01 has no diff.
        let (diffs, log) = replication(&[
            (5128, "2026-09-27"),
            (5129, "2026-09-28"),
            (5130, "2026-09-29"),
            (5131, "2026-09-30"),
            (5132, "2026-10-02"),
        ]);
        let scratch = Scratch::new("gaps");
        let store = Store::at(&scratch.0);
        let fails = |version: &str, params: Vec<(String, String)>| {
            let request = Request { source: &diffs, version: Some(version.into()), params };
            fetch(&store, &quick(), &request).unwrap_err()
        };
        assert!(fails("2026-09-30", vec![]).contains("takes from=YYYY-MM-DD"));
        assert!(fails("2026-09-30", from("2026-10-01")).contains("after the version"));
        assert!(log.lock().unwrap().is_empty());
        let err = fails("2026-10-02", from("2026-09-28"));
        assert!(err.contains("5129 to 5132 are not one per day"), "{err}");
        let err = fails("2026-10-01", from("2026-09-28"));
        assert!(err.contains("no daily diff of 2026-10-01"), "{err}");
        let err = fails("2026-10-03", from("2026-09-28"));
        assert!(err.contains("the newest daily diff is of 2026-10-02"), "{err}");
        let log = log.lock().unwrap();
        assert!(!log.iter().filter_map(|h| header(h, ":path")).any(|p| p.ends_with(".osc.gz")), "no diff downloads");
    }

    #[test]
    fn a_geofabrik_extract_is_the_file_of_the_day_of_its_data() {
        let (url, log) = serve(|_, headers| match header(headers, ":path").unwrap_or_default() {
            "/europe/monaco-updates/state.txt" => whole(b"sequenceNumber=4928\ntimestamp=2026-10-03T20\\:20\\:50Z\n"),
            "/europe/monaco-261003.osm.pbf" => whole(b"monaco"),
            _ => not_found(),
        });
        let scratch = Scratch::new("geofabrik");
        let store = Store::at(&scratch.0);
        let extracts = located(FetchKind::Geofabrik, &url.replace("data/file.bin", "{area}-{yymmdd}.osm.pbf"));
        let area = vec![("area".to_string(), "europe/monaco".to_string())];
        let newest = Request { source: &extracts, version: None, params: area.clone() };
        let snapshot = fetch(&store, &quick(), &newest).unwrap();
        assert_eq!(
            (snapshot.version.as_str(), snapshot.files[0].name.as_str()),
            ("2026-10-03", "monaco-261003.osm.pbf")
        );
        // Last-Modified is two days after the data, which a dated file does not ask.
        assert_eq!(log.lock().unwrap().len(), 2);
        let gone = Request { source: &extracts, version: Some("2026-08-15".into()), params: area };
        assert!(fetch(&store, &quick(), &gone).unwrap_err().contains("first of each month"));
    }

    #[cfg(unix)]
    #[test]
    fn a_capture_is_stored_and_another_day_comes_only_from_the_store() {
        let scratch = Scratch::new("capture");
        let store = Store::at(&scratch.0);
        let dtm = located(FetchKind::Dtm, "https://example.org/wcs");
        let today = Request { source: &dtm, version: None, params: vec![] };
        let script = |script: &'static str| {
            move |work: &std::path::Path, out: &std::path::Path| {
                let mut command = std::process::Command::new("sh");
                command.args(["-c", script, "sh"]).arg(work).arg(out);
                command
            }
        };
        let err = capture::capture(&store, &today, "bbox=1,2,3,4", script("echo partial > \"$2/a.tif\"; exit 3"))
            .unwrap_err();
        assert!(err.contains("failed"), "{err}");
        let version = date::format(date::today());
        assert_eq!(store.snapshot("land", &version).unwrap(), None, "a failed run records nothing");
        let writes = "mkdir \"$2/sub\" && echo raster > \"$2/sub/a.tif\" && echo crs > \"$2/sub/a.prj\" && echo zip > \"$1/a.zip\"";
        let snapshot = capture::capture(&store, &today, "bbox=1,2,3,4", script(writes)).unwrap();
        let urls: Vec<_> = snapshot.files.iter().map(|file| file.url.as_str()).collect();
        assert_eq!(
            urls,
            ["https://example.org/wcs#bbox=1,2,3,4/sub/a.prj", "https://example.org/wcs#bbox=1,2,3,4/sub/a.tif"]
        );
        assert_eq!(snapshot.files[1].sha256, sha256_hex(b"raster\n"));
        assert!(store.object(&snapshot.files[1].sha256).is_file());
        assert!(!store.root().join("partial").read_dir().unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("capture-")));
        assert_eq!(capture::capture(&store, &today, "bbox=1,2,3,4", script("exit 1")).unwrap(), snapshot);
        let pinned = Request { version: Some(version), ..today };
        assert_eq!(capture::capture(&store, &pinned, "bbox=1,2,3,4", script("exit 1")).unwrap(), snapshot);
        let yesterday = Request { version: Some(date::format(date::today() - 1)), ..pinned };
        let err = capture::capture(&store, &yesterday, "bbox=1,2,3,4", script("exit 1")).unwrap_err();
        assert!(err.contains("today's data"), "{err}");
        let keyed = Source {
            credential: Some(crate::sources::Credential { env: vec!["OBC_TEST_NO_SUCH_KEY".into()], file: None }),
            ..dtm.clone()
        };
        let blocked = Request { source: &keyed, version: None, params: vec![] };
        let err = capture::capture(&store, &blocked, "bbox=5,6,7,8", script("exit 1")).unwrap_err();
        assert!(err.contains("credential missing: OBC_TEST_NO_SUCH_KEY"), "{err}");
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
