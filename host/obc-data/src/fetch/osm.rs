//! OSM files named by the day of their data: the daily replication diffs and the dated Geofabrik
//! extracts.

use super::http::Http;
use super::upstream;
use super::{check_version, expand, files, get, record, Request};
use crate::date;
use crate::sources::Source;
use crate::store::{FileRecord, Snapshot, Store};

/// The daily diffs of version `E` from `from=B`: one diff for each day after `B` up to `E`, in
/// order. Each diff has the URL of its sequence, so the record of `E` can hold the diffs of any
/// start. A step applies them to the planet of `B` with `osmium apply-changes`; the fetch does not.
pub fn replication(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    let directory = source.fetch.url.as_deref().unwrap_or_default();
    if !directory.ends_with('/') || directory.contains('{') {
        return Err(format!("source `{}`: an `osm` URL is a replication directory that ends with `/`", source.id));
    }
    let from = match request.params.as_slice() {
        [(name, value)] if name == "from" => date::parse(value),
        _ => None,
    };
    let Some(from) = from else {
        return Err(format!(
            "source `{}` takes from=YYYY-MM-DD, the day of the base planet, and no other NAME=VALUE",
            source.id
        ));
    };
    let version = match &request.version {
        Some(version) => version.clone(),
        None => upstream::newest(store, http, source, upstream::CACHE)
            .version()
            .ok_or_else(|| format!("source `{}`: the newest daily diff is not known: give SOURCE@VERSION", source.id))?
            .to_string(),
    };
    check_version(source, &version)?;
    let day = date::parse(&version).unwrap_or_default();
    let Ok(days) = u64::try_from(day - from) else {
        return Err(format!("source `{}`: from={} is after the version {version}", source.id, date::format(from)));
    };
    if let Some(files) = stored(store, source, &version, days)? {
        return Ok(Snapshot { source: source.id.clone(), version, files });
    }
    // The sequences are known before a diff downloads, so a day without a diff fails at once.
    let (newest, newest_day) = state(http, &format!("{directory}state.txt"))?;
    if day > newest_day {
        return Err(format!("source `{}`: the newest daily diff is of {}", source.id, date::format(newest_day)));
    }
    let first = sequence(http, directory, (newest, newest_day), from)?;
    let last = sequence(http, directory, (newest, newest_day), day)?;
    if last.checked_sub(first) != Some(days) {
        return Err(format!(
            "source `{}`: the daily diffs {first} to {last} are not one per day from {} to {version}",
            source.id,
            date::format(from)
        ));
    }
    // A diff never changes, so a diff in the record of another version needs no download.
    let known: Vec<FileRecord> = store.snapshots(&source.id)?.into_iter().flat_map(|snapshot| snapshot.files).collect();
    let mut files = Vec::new();
    // Newest first: the record of `E` then always has the diff of `E`, which `stored` relies on.
    for sequence in (first + 1..=last).rev() {
        let url = format!("{directory}{}.osc.gz", path(sequence));
        let file = match known.iter().find(|file| file.url == url && store.object(&file.sha256).is_file()) {
            Some(file) => {
                record(store, &source.id, &version, std::slice::from_ref(file))?;
                file.clone()
            }
            None => get(store, http, source, Some(&version), &url, false, None)?,
        };
        files.push(file);
    }
    files.reverse();
    Ok(Snapshot { source: source.id.clone(), version, files })
}

/// The diffs of the last `days` days up to `version`, when its record has each of them. The
/// newest sequence in the record is the diff of `version`. A fetch records a diff only after it
/// found one diff per day back to its start, so the sequences before it are the days before.
fn stored(store: &Store, source: &Source, version: &str, days: u64) -> Result<Option<Vec<FileRecord>>, String> {
    if days == 0 {
        return Ok(Some(Vec::new()));
    }
    let Some(snapshot) = store.snapshot(&source.id, version)? else { return Ok(None) };
    let directory = source.fetch.url.as_deref().unwrap_or_default();
    let sequence = |file: &FileRecord| {
        let path = file.url.strip_prefix(directory)?.strip_suffix(".osc.gz")?;
        path.replace('/', "").parse::<u64>().ok()
    };
    let Some(last) = snapshot.files.iter().filter_map(sequence).max() else { return Ok(None) };
    let Some(first) = (last + 1).checked_sub(days) else { return Ok(None) };
    Ok((first..=last)
        .map(|sequence| {
            let file = snapshot.file(&format!("{directory}{}.osc.gz", path(sequence)))?;
            store.object(&file.sha256).is_file().then(|| file.clone())
        })
        .collect())
}

/// A dated Geofabrik extract. Without a version, the day of the data in the replication state of
/// the area names the newest extract.
pub fn extract(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let template = request.source.fetch.url.as_deref().unwrap_or_default();
    if !template.contains("-{yymmdd}.osm.pbf") {
        return files(store, http, request);
    }
    let version = match &request.version {
        Some(version) => version.clone(),
        None => {
            let states = template.replace("-{yymmdd}.osm.pbf", "-updates/state.txt");
            // Geofabrik makes every area each day, so the earliest of the newest days has every extract.
            let mut day = i64::MAX;
            for url in expand(&states, None, &request.params)? {
                day = day.min(state(http, &url).map_err(hint)?.1);
            }
            date::format(day)
        }
    };
    let request = Request { source: request.source, version: Some(version), params: request.params.clone() };
    files(store, http, &request).map_err(hint)
}

fn hint(error: String) -> String {
    match error.ends_with("HTTP 404") {
        true => format!(
            "{error}: Geofabrik keeps the extracts of about the last month, and of the first of each month, \
             or the area does not exist"
        ),
        false => error,
    }
}

/// The sequence of the daily diff of `day`. It follows from the newest sequence when the
/// replication has one diff per day; when its state names another day, the difference moves it
/// once more.
fn sequence(http: &Http, replication: &str, (newest, newest_day): (u64, i64), day: i64) -> Result<u64, String> {
    let mut guess = newest as i64 - (newest_day - day);
    for _ in 0..2 {
        if guess < 0 {
            break;
        }
        let (_, got) = state(http, &format!("{replication}{}.state.txt", path(guess as u64)))?;
        if got == day {
            return Ok(guess as u64);
        }
        guess += day - got;
    }
    Err(format!("{replication}: no daily diff of {}", date::format(day)))
}

/// The sequence number and the day of an Osmosis replication `state.txt`.
pub(super) fn state(http: &Http, url: &str) -> Result<(u64, i64), String> {
    let text = http.text(url, "text/plain")?;
    let value = |key: &str| text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('='));
    let sequence = value("sequenceNumber").and_then(|n| n.trim().parse().ok());
    let day = value("timestamp").and_then(|t| t.get(..10)).and_then(date::parse);
    sequence.zip(day).ok_or_else(|| format!("{url}: no sequenceNumber and timestamp"))
}

/// `5130` as `000/005/130`, the path of a replication sequence.
fn path(sequence: u64) -> String {
    let digits = format!("{sequence:09}");
    format!("{}/{}/{}", &digits[..3], &digits[3..6], &digits[6..])
}
