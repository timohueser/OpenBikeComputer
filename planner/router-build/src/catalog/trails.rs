//! The long trails of the catalog: which records are trails, and the far line and the list entry
//! of each. `specs/route-catalog.md#trail-files` is the rule.
use super::{decode, encode};
use planner_router::shape::{distance, P};
use serde_json::{json, Map, Value};
use std::collections::HashMap;

/// A record is a trail when it takes more than this moving time: the owner's "longer than a day".
pub const TRAIL_HOURS: f64 = 12.0;
/// The tolerance of the far line: about one pixel at the nearest far view, a view of about 100 km
/// on a map of about 1,000 pixels. A gap between two parts below it does not show.
pub const FAR_TOLERANCE_M: f64 = 100.0;
/// The largest far file: below the largest catalog cell file of `central-switzerland`, so a far
/// view of 12 files costs at most what a near view of 12 dense cell files costs.
pub const TRAIL_FILE_BYTES: usize = 400_000;

/// The fields of a far entry that are as in the record.
const ENTRY_FIELDS: [&str; 10] =
    ["id", "kind", "name", "ref", "symbol", "rank", "loop", "length_m", "ascent_m", "hardest"];

/// Each trail of `records`, in their order: its far entry with its whole far line in `far_udeg`,
/// and its whole record. `hours` holds the moving time of each route record.
pub fn trails(records: &[Value], hours: &HashMap<i64, f64>) -> Vec<(Value, Value)> {
    let by_id: HashMap<i64, &Value> = records.iter().map(|record| (id(record), record)).collect();
    records
        .iter()
        .filter(|record| is_trail(record, hours))
        .map(|record| (entry(record, &far_line(record, &by_id)), record.clone()))
        .collect()
}

fn id(value: &Value) -> i64 {
    value["id"].as_i64().expect("a record has an id")
}

/// The parts of a long record that are in the catalog.
fn kept(record: &Value) -> Option<impl Iterator<Item = &Value>> {
    let parts = record.get("parts")?.as_array()?;
    Some(parts.iter().filter(|part| part.get("missing").is_none()))
}

/// The moving time of a record in hours: a route record by its plan route, a long record by its
/// parts in the catalog.
fn moving_hours(record: &Value, hours: &HashMap<i64, f64>) -> f64 {
    let time = |id| hours.get(&id).copied().unwrap_or(0.0);
    match kept(record) {
        Some(parts) => parts.map(|part| time(id(part))).sum(),
        None => time(id(record)),
    }
}

/// A record with no `trail` that has two or more official stages, or takes more than
/// `TRAIL_HOURS`.
fn is_trail(record: &Value, hours: &HashMap<i64, f64>) -> bool {
    record.get("trail").is_none()
        && (record["stage_count"].as_u64() >= Some(2) || moving_hours(record, hours) > TRAIL_HOURS)
}

/// The far line: the `line_udeg` of each part in the catalog in the trail direction, or the
/// route's own line. It breaks at a missing part and where a part starts more than
/// `FAR_TOLERANCE_M` from the end of the part before it.
fn far_line(record: &Value, records: &HashMap<i64, &Value>) -> Vec<Vec<P>> {
    let Some(parts) = record.get("parts").and_then(Value::as_array) else {
        return vec![decode(&record["line_udeg"])];
    };
    let mut pieces: Vec<Vec<P>> = Vec::new();
    let mut after_kept = false;
    for part in parts {
        if part.get("missing").is_some() {
            after_kept = false;
            continue;
        }
        let mut line = decode(&records.get(&id(part)).expect("a kept part has a record")["line_udeg"]);
        if part.get("backwards").is_some() {
            line.reverse();
        }
        match pieces.last_mut() {
            Some(last) if after_kept && distance(last[last.len() - 1], line[0]) <= FAR_TOLERANCE_M => {
                let shared = usize::from(last[last.len() - 1] == line[0]);
                last.extend(&line[shared..]);
            }
            _ => pieces.push(line),
        }
        after_kept = true;
    }
    pieces
}

/// The far entry of a trail with the pieces of its far line. `stage_count` is there when the
/// trail has official numbers.
fn entry(record: &Value, pieces: &[Vec<P>]) -> Value {
    let mut entry = Map::new();
    for field in ENTRY_FIELDS {
        if let Some(value) = record.get(field) {
            entry.insert(field.into(), value.clone());
        }
    }
    if let Some(count) = record["stage_count"].as_u64().filter(|&count| count > 0) {
        entry.insert("stage_count".into(), json!(count));
    }
    entry.insert("far_udeg".into(), json!(pieces.iter().map(|piece| encode(piece)).collect::<Vec<_>>()));
    Value::Object(entry)
}
