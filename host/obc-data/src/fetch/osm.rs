//! OSM files named by the day of their data: the weekly planet with the daily replication diffs
//! after it, and the dated Geofabrik extracts.

use super::http::Http;
use super::upstream;
use super::{check_version, expand, files, get, record, Request};
use crate::date;
use crate::store::{Snapshot, Store};

/// The planet of version `V`: the weekly planet of the Monday on or before `V`, then one daily
/// diff for each later day up to `V`, in order. A step applies the diffs with
/// `osmium apply-changes`; the fetch does not.
pub fn planet(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let source = request.source;
    let template = source.fetch.url.as_deref().unwrap_or_default();
    let server = template.split_once("/pbf/").map(|(server, _)| server);
    let Some(server) = server.filter(|_| template.contains("{yymmdd}")) else {
        return Err(format!("source `{}`: an `osm` URL is <server>/pbf/…{{yymmdd}}…", source.id));
    };
    if !request.params.is_empty() {
        return Err(format!("source `{}` takes no NAME=VALUE", source.id));
    }
    let version = match &request.version {
        Some(version) => version.clone(),
        None => upstream::newest(store, http, source, upstream::CACHE)
            .version()
            .ok_or_else(|| format!("source `{}`: the newest planet is not known: give SOURCE@VERSION", source.id))?
            .to_string(),
    };
    check_version(source, &version)?;
    let day = date::parse(&version).unwrap_or_default();
    // 1970-01-01 was a Thursday.
    let monday = day - (day + 3).rem_euclid(7);
    let weekly = date::format(monday);
    let planet_url = expand(template, Some(&weekly), &[])?.remove(0);
    let days = (day - monday) as u64;
    let stored = store.snapshot(&source.id, &version)?.filter(|snapshot| {
        snapshot.file(&planet_url).is_some()
            && snapshot.files.len() as u64 == days + 1
            && snapshot.files.iter().all(|file| store.object(&file.sha256).is_file())
    });
    if let Some(snapshot) = stored {
        return Ok(snapshot);
    }
    // The diffs are known before the planet downloads, so a day without them fails at once.
    let mut diffs = Vec::new();
    let replication = format!("{server}/replication/day/");
    if days > 0 {
        let (newest, newest_day) = state(http, &format!("{replication}state.txt"))?;
        if day > newest_day {
            return Err(format!("source `{}`: the newest daily diff is of {}", source.id, date::format(newest_day)));
        }
        let first = sequence(http, &replication, (newest, newest_day), monday)?;
        let last = sequence(http, &replication, (newest, newest_day), day)?;
        if last.checked_sub(first) != Some(days) {
            return Err(format!(
                "source `{}`: the daily diffs {first} to {last} are not one per day from {weekly} to {version}",
                source.id
            ));
        }
        diffs = (first + 1..=last).map(|sequence| format!("{replication}{}.osc.gz", path(sequence))).collect();
    }
    let planet = get(store, http, source, Some(&weekly), &planet_url, false, None).map_err(|error| {
        match error.ends_with("HTTP 404") {
            true => format!("{error}: a planet appears some days after its Monday, and a week can be missing"),
            false => error,
        }
    })?;
    if days > 0 {
        record(store, &source.id, &version, std::slice::from_ref(&planet))?;
    }
    let mut files = vec![planet];
    for url in &diffs {
        files.push(get(store, http, source, Some(&version), url, false, None)?);
    }
    Ok(Snapshot { source: source.id.clone(), version, files })
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
fn state(http: &Http, url: &str) -> Result<(u64, i64), String> {
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
