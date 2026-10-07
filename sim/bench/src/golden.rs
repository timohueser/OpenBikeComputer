use std::collections::BTreeMap;

use crate::{corridor::CorridorResult, scenes::SceneResult};

//
// One record per case, `name key=value …`, keys named exactly as the `RenderStats` / corridor-table
// fields that produced them so a golden line greps straight back to the code. Both matrices share
// one namespace: corridor cases carry a `corridor/` prefix and the name selects the key set, so
// there is no record-type field to keep in sync.
//
// Timings are *not* here. Shared runners are noisy; only values that are bit-for-bit reproducible on
// any host are gated.

/// A scene record's keys, in the order [`golden_lines`] writes them. `hash` is `0x` + 16 hex
/// digits; every counter is decimal. The counters are the last of [`crate::scenes::ITERS`] warmed renders — a
/// per-frame steady state, so they do not depend on how many iterations ran.
const SCENE_KEYS: [&str; 6] =
    ["hash", "chunks_visited", "map_chunk_hits", "map_chunk_misses", "map_sd_reads", "map_bytes_read"];

/// A corridor record's keys, named after the columns [`crate::corridor::print_corridor_table`] prints.
const CORRIDOR_KEYS: [&str; 5] = ["rows", "map_reads", "map_bytes", "route_reads", "route_bytes"];

/// The corridor matrix's namespace inside the shared golden file.
const CORRIDOR_PREFIX: &str = "corridor/";

/// Which key set a case name carries.
fn keys_for(name: &str) -> &'static [&'static str] {
    if name.starts_with(CORRIDOR_PREFIX) {
        &CORRIDOR_KEYS
    } else {
        &SCENE_KEYS
    }
}

/// One scene's gated values, in [`SCENE_KEYS`] order.
pub(super) fn scene_values(r: &SceneResult) -> Vec<u64> {
    let s = &r.stats;
    vec![
        r.hash,
        s.chunks_visited as u64,
        u64::from(s.map_chunk_hits),
        u64::from(s.map_chunk_misses),
        u64::from(s.map_sd_reads),
        u64::from(s.map_bytes_read),
    ]
}

/// One corridor case's gated values, in [`CORRIDOR_KEYS`] order.
fn corridor_values(r: &CorridorResult) -> Vec<u64> {
    vec![r.results as u64, u64::from(r.map_reads), r.map_bytes, u64::from(r.route_reads), r.route_bytes]
}

/// Both matrices as `(case name, gated values)` in run order — the order the golden file is written
/// in, so it reads like the tables the bench prints.
pub(super) fn records(scenes: &[SceneResult], corridor: &[CorridorResult]) -> Vec<(String, Vec<u64>)> {
    scenes
        .iter()
        .map(|r| (r.name.clone(), scene_values(r)))
        .chain(corridor.iter().map(|r| (format!("{CORRIDOR_PREFIX}{}", r.name), corridor_values(r))))
        .collect()
}

/// Render one value the way its key is written and read.
fn format_value(key: &str, value: u64) -> String {
    if key == "hash" {
        format!("0x{value:016x}")
    } else {
        value.to_string()
    }
}

fn parse_value(at: usize, key: &str, value: &str) -> Result<u64, String> {
    if key == "hash" {
        return value
            .strip_prefix("0x")
            .filter(|digits| digits.len() == 16 && digits.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .and_then(|digits| u64::from_str_radix(digits, 16).ok())
            .ok_or_else(|| format!("line {at} has invalid hash `{value}`"));
    }
    value.parse().map_err(|error| format!("line {at} has invalid {key} `{value}`: {error}"))
}

/// Parse the golden file. Every key of the record's key set must appear exactly once and no other
/// key is accepted, so a dropped counter is an error rather than a silent zero that would gate
/// nothing.
pub(super) fn parse_golden(golden: &str) -> Result<BTreeMap<String, Vec<u64>>, String> {
    let mut expected = BTreeMap::new();
    for (index, raw) in golden.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let at = index + 1;
        let mut fields = line.split_whitespace();
        let name = fields.next().expect("a non-empty trimmed line has a first field");
        let keys = keys_for(name);
        let mut found: Vec<Option<u64>> = vec![None; keys.len()];
        for field in fields {
            let (key, value) =
                field.split_once('=').ok_or_else(|| format!("line {at} field `{field}` is not `key=value`"))?;
            let slot = keys
                .iter()
                .position(|known| *known == key)
                .ok_or_else(|| format!("line {at} has unknown key `{key}` for case `{name}`"))?;
            if found[slot].is_some() {
                return Err(format!("line {at} repeats key `{key}`"));
            }
            found[slot] = Some(parse_value(at, key, value)?);
        }
        let values = keys
            .iter()
            .zip(&found)
            .map(|(key, value)| value.ok_or_else(|| format!("line {at} case `{name}` is missing key `{key}`")))
            .collect::<Result<Vec<u64>, String>>()?;
        if expected.insert(name.to_string(), values).is_some() {
            return Err(format!("line {at} duplicates case `{name}`"));
        }
    }
    Ok(expected)
}

/// Compare a run's records to the golden file. Malformed, duplicate or incomplete golden lines, any
/// changed value, and any difference between the golden/current case-name sets print a focused
/// diagnostic and fail the check. A counter delta fails exactly like a hash delta.
pub(super) fn check_golden(scenes: &[SceneResult], corridor: &[CorridorResult], golden: &str) -> bool {
    let expected = match parse_golden(golden) {
        Ok(expected) => expected,
        Err(error) => {
            eprintln!("GOLDEN INVALID: {error}");
            return false;
        }
    };
    let mut current = BTreeMap::new();
    let mut ok = true;
    for (name, values) in records(scenes, corridor) {
        if current.insert(name.clone(), values).is_some() {
            eprintln!("CURRENT INVALID: duplicate case `{name}`");
            ok = false;
        }
    }
    for (name, want) in &expected {
        let Some(got) = current.get(name) else {
            eprintln!("STALE {name}: golden entry has no current case");
            ok = false;
            continue;
        };
        for ((key, want), got) in keys_for(name).iter().zip(want).zip(got) {
            if want != got {
                eprintln!(
                    "MISMATCH {name} {key}: golden {} != run {}",
                    format_value(key, *want),
                    format_value(key, *got)
                );
                ok = false;
            }
        }
    }
    for name in current.keys() {
        if !expected.contains_key(name) {
            eprintln!("MISSING {name}: no golden entry for this case");
            ok = false;
        }
    }
    ok
}

pub(super) fn golden_lines(scenes: &[SceneResult], corridor: &[CorridorResult]) -> String {
    let mut out = String::new();
    for (name, values) in records(scenes, corridor) {
        out.push_str(&name);
        for (key, value) in keys_for(&name).iter().zip(&values) {
            out.push_str(&format!(" {key}={}", format_value(key, *value)));
        }
        out.push('\n');
    }
    out
}
