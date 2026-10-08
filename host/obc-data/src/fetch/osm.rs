//! OSM files named by the day of their data: the daily replication diffs and the dated Geofabrik
//! extracts.

use super::http::Http;
use super::upstream;
use super::{check_version, expand, files, get, record, Request};
use crate::date;
use crate::store::{FileRecord, Snapshot, Store};

/// The daily diffs of version `E` from `from=B`: the diff and the `state.txt` of each sequence
/// after the sequence of `B` up to the sequence of `E`, and the `state.txt` of `B`. The states
/// name the day of each sequence, so a record of any `E` serves any start. The fetch does not
/// apply the diffs.
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
            "source `{}` takes from=YYYY-MM-DD, a version of `{}`, and no other NAME=VALUE",
            source.id,
            source.fetch.from.as_deref().unwrap_or_default()
        ));
    };
    let version = match &request.version {
        Some(version) => version.clone(),
        None => upstream::observe(store, http, source, &request.params, upstream::CACHE, date::now())
            .result
            .version()
            .ok_or_else(|| format!("source `{}`: the newest daily diff is not known: give SOURCE@VERSION", source.id))?
            .to_string(),
    };
    check_version(source, &version)?;
    let day = date::parse(&version).unwrap_or_default();
    if from > day {
        return Err(format!("source `{}`: from={} is after the version {version}", source.id, date::format(from)));
    }
    // A diff and its state never change, so a file in the record of any version serves this one.
    let mut known: Vec<FileRecord> = store
        .snapshots(&source.id)?
        .into_iter()
        .flat_map(|snapshot| snapshot.files)
        .filter(|file| store.object(&file.sha256).is_file())
        .collect();
    known.sort_by(|a, b| a.url.cmp(&b.url));
    known.dedup_by(|a, b| a.url == b.url);
    let file = |url: &str| -> Result<FileRecord, String> {
        match known.iter().find(|file| file.url == url) {
            Some(file) => {
                record(store, &source.id, &version, std::slice::from_ref(file))?;
                Ok(file.clone())
            }
            None => get(store, http, source, Some(&version), url, false, None),
        }
    };
    let url = |sequence: u64, suffix: &str| format!("{directory}{}{suffix}", path(sequence));
    let sequences = match stored(store, directory, &known, from, day) {
        Some(sequences) => sequences,
        None => {
            // The sequences are known before a diff downloads, so a day without a diff fails at once.
            let (newest, newest_day) = state(http, &format!("{directory}state.txt"))?;
            if day > newest_day {
                return Err(format!(
                    "source `{}`: the newest daily diff is of {}",
                    source.id,
                    date::format(newest_day)
                ));
            }
            // A probe records nothing, so a failed fetch leaves no record and a wrong guess none
            // in the record of `E`.
            let day_of = |sequence| {
                let url = url(sequence, ".state.txt");
                let state = match known.iter().find(|file| file.url == url) {
                    Some(file) => parse_state(
                        &std::fs::read_to_string(store.object(&file.sha256)).map_err(|e| e.to_string())?,
                        &url,
                    ),
                    None => state(http, &url),
                };
                Ok(state?.1)
            };
            let first = sequence(directory, (newest, newest_day), from, day_of)?;
            first..=sequence(directory, (newest, newest_day), day, day_of)?
        }
    };
    let mut files = vec![file(&url(*sequences.start(), ".state.txt"))?];
    for sequence in sequences.start() + 1..=*sequences.end() {
        files.push(file(&url(sequence, ".osc.gz"))?);
        files.push(file(&url(sequence, ".state.txt"))?);
    }
    Ok(Snapshot { source: source.id.clone(), version, files })
}

/// The sequences of `from` and `day` from the states in `known`, when each day has one sequence
/// there and `known` has the diff and the state of every sequence between them.
fn stored(
    store: &Store,
    directory: &str,
    known: &[FileRecord],
    from: i64,
    day: i64,
) -> Option<std::ops::RangeInclusive<u64>> {
    let days: Vec<(u64, i64)> = known
        .iter()
        .filter(|file| file.url.starts_with(directory) && file.url.ends_with(".state.txt"))
        .filter_map(|file| parse_state(&std::fs::read_to_string(store.object(&file.sha256)).ok()?, &file.url).ok())
        .collect();
    let sequence = |of: i64| match days.iter().filter(|(_, got)| *got == of).collect::<Vec<_>>()[..] {
        [(sequence, _)] => Some(*sequence),
        _ => None,
    };
    let (first, last) = (sequence(from)?, sequence(day)?);
    let has = |url: String| known.iter().any(|file| file.url == url);
    (first + 1..=last)
        .all(|s| has(format!("{directory}{}.osc.gz", path(s))) && has(format!("{directory}{}.state.txt", path(s))))
        .then_some(first..=last)
}

/// A dated Geofabrik extract. Without a version, the day of the data in the replication state of
/// the area names the newest extract.
pub fn extract(store: &Store, http: &Http, request: &Request) -> Result<Snapshot, String> {
    let template = request.source.fetch.url.as_deref().unwrap_or_default();
    if !template.contains("-{yymmdd}.osm.pbf") {
        return files(store, http, request);
    }
    let day = match &request.version {
        Some(_) => None,
        None => {
            let states = template.replace("-{yymmdd}.osm.pbf", "-updates/state.txt");
            // Geofabrik makes every area each day, so the earliest of the newest days has every extract.
            let mut day = i64::MAX;
            for url in expand(&states, None, &request.params)? {
                day = day.min(state(http, &url).map_err(hint)?.1);
            }
            Some(day)
        }
    };
    let at =
        |version: String| Request { source: request.source, version: Some(version), params: request.params.clone() };
    match day {
        None => files(store, http, &at(request.version.clone().unwrap_or_default())).map_err(hint),
        // The dated file of the newest day can come after its `state.txt`; the day before is there.
        Some(day) => match files(store, http, &at(date::format(day))) {
            Err(error) if super::http::not_found(&error) => {
                files(store, http, &at(date::format(day - 1))).map_err(hint)
            }
            result => result.map_err(hint),
        },
    }
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
/// once more. A day with two diffs has no sequence.
fn sequence(
    replication: &str,
    (newest, newest_day): (u64, i64),
    day: i64,
    day_of: impl Fn(u64) -> Result<i64, String>,
) -> Result<u64, String> {
    let mut guess = newest as i64 - (newest_day - day);
    for _ in 0..2 {
        if guess < 0 {
            break;
        }
        let got = day_of(guess as u64)?;
        if got == day {
            let guess = guess as u64;
            // The day grows with the sequence, so a second diff of the day is a neighbour.
            for neighbour in
                [guess.checked_sub(1), Some(guess + 1).filter(|&next| next <= newest)].into_iter().flatten()
            {
                if day_of(neighbour)? == day {
                    return Err(format!("{replication}: two daily diffs of {}", date::format(day)));
                }
            }
            return Ok(guess);
        }
        guess += day - got;
    }
    Err(format!("{replication}: no daily diff of {}", date::format(day)))
}

/// The sequence number and the day of an Osmosis replication `state.txt`.
pub(super) fn state(http: &Http, url: &str) -> Result<(u64, i64), String> {
    parse_state(&http.text(url, "text/plain")?, url)
}

fn parse_state(text: &str, url: &str) -> Result<(u64, i64), String> {
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
