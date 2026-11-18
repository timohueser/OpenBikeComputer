//! Long routes: the parts of a long relation in order, their direction and gaps, and the long
//! record. `specs/route-catalog.md#long-routes` is the rule.
use super::{
    cell_ids, children, class, graded, header, is_connector, is_route, tag, Class, Kept, MAX_GAP_M, PROFILE_STEP_M,
    ROUNDTRIP_M,
};
use crate::source::{Relation, Relations, Table};
use planner_router::shape::{distance, P};
use serde_json::{json, Map, Value};
use std::collections::{BTreeSet, HashSet};

/// The levels of long relations inside each other that `leaves` opens.
const MAX_DEPTH: usize = 4;
/// The entries of `trail_profile_m`.
const TRAIL_PROFILE: usize = 201;
/// Two parts meet when one has an end this near the other's end. It leaves room for the slack of two
/// stage ends at one place, such as a plan route that starts on the road next to the line, and it is
/// far under the distance between the separate routes of one area.
const MEET_M: f64 = 60.0;

/// A part of a long relation: a route relation, or a relation that the data does not hold, with the
/// level of long relations that lists it, 1 for a member of the relation itself.
#[derive(Clone, Copy)]
pub(super) struct Leaf {
    pub id: i64,
    pub depth: usize,
}

/// The parts of a long relation in member order: its children with a stage role, each long child
/// opened in its place. A relation counts once, at its first place. The answer also says whether
/// it opens a connector leg.
pub(super) fn leaves(relation: &Relation, relations: &Table<Relations>) -> (Vec<Leaf>, bool) {
    let (mut leaves, mut connector) = (Vec::new(), false);
    open(relation, relations, 1, &mut HashSet::from([relation.id]), &mut leaves, &mut connector);
    (leaves, connector)
}

fn open(
    relation: &Relation,
    relations: &Table<Relations>,
    depth: usize,
    seen: &mut HashSet<i64>,
    leaves: &mut Vec<Leaf>,
    connector: &mut bool,
) {
    for id in children(relation) {
        if !seen.insert(id) {
            continue;
        }
        let Some(child) = relations.get(id) else {
            leaves.push(Leaf { id, depth });
            continue;
        };
        let class = class(&child, relations);
        if !is_route(relations, id) {
            continue;
        }
        // A connector leg is no stage, so it moves no stage number.
        if is_connector(&child, class, relations) {
            *connector = true;
            continue;
        }
        if class != Class::Long {
            leaves.push(Leaf { id, depth });
        } else if depth < MAX_DEPTH {
            open(&child, relations, depth + 1, seen, leaves, connector);
        }
    }
}

/// A part of a long relation as its record needs it.
pub(super) struct Part<'a> {
    pub id: i64,
    /// The OSM `name`, or empty.
    pub name: String,
    /// The official number; `None` for a section of a one-line trail.
    pub stage: Option<usize>,
    /// The builder cut the line at the end of this part.
    pub cut: bool,
    pub kept: Option<&'a Kept>,
    /// The length, first and last point of the main line of a failed route.
    pub line: Option<(f64, P, P)>,
}

/// The first and last point of a part, and whether it is a loop.
#[derive(Clone, Copy)]
struct Ends {
    start: P,
    finish: P,
    closed: bool,
}

impl Part<'_> {
    fn ends(&self) -> Option<Ends> {
        match self.kept {
            Some(kept) => Some(Ends { start: kept.start, finish: kept.finish, closed: kept.record["loop"] == true }),
            None => self.line.map(|(_, start, finish)| Ends { start, finish, closed: false }),
        }
    }

    fn length(&self) -> Option<u64> {
        match self.kept {
            Some(kept) => kept.record["length_m"].as_u64(),
            None => self.line.map(|(length, ..)| length.round() as u64),
        }
    }
}

/// Whether the parts of a long relation are separate routes, not one line: at least two parts
/// count against one line, and more than meet. A loop counts against one line: it comes back to its
/// start and takes the line no further, so it meets no part. A pair of neighbouring parts with ends,
/// neither a loop, meets or counts against one line. A part with no ends counts for nothing. One
/// count is a hole in the map data, not a set of routes.
pub(super) fn apart(parts: &[Part]) -> bool {
    let flags = order(parts);
    let ends: Vec<Option<Ends>> = parts.iter().map(Part::ends).collect();
    let (mut against, mut meets) = (ends.iter().flatten().filter(|e| e.closed).count(), 0);
    for i in 1..parts.len() {
        let (Some(before), Some(part)) = (ends[i - 1], ends[i]) else { continue };
        if before.closed || part.closed {
            continue;
        }
        let end = if flags[i - 1].0 { before.start } else { before.finish };
        if gap(part, end) <= MEET_M {
            meets += 1;
        } else {
            against += 1;
        }
    }
    against >= 2 && against > meets
}

/// The record of a long relation from its parts, and the plan points of its kept parts joined.
/// `None` when no part is kept. A relation that opens a connector leg is never complete: the leg
/// leaves a gap between its parts. It has no `symbol` when a kept part is in France.
pub(super) fn record(
    relation: &Relation,
    kind: &str,
    parts: &[Part],
    connector: bool,
) -> Option<(Map<String, Value>, usize)> {
    let kept: Vec<&Kept> = parts.iter().filter_map(|part| part.kept).collect();
    if kept.is_empty() {
        return None;
    }
    let flags = order(parts);
    // The first and last point of each part in the direction of the trail.
    let way = |i: usize| parts[i].ends().map(|e| if flags[i].0 { (e.finish, e.start) } else { (e.start, e.finish) });
    let mut record = header(relation, kind, kept.iter().any(|k| k.french));
    let sum = |field: &str| kept.iter().map(|k| k.record[field].as_u64().unwrap()).sum::<u64>();
    for field in ["length_m", "ascent_m", "descent_m"] {
        record.insert(field.into(), json!(sum(field)));
    }
    if graded(kind) {
        let mut grades = [0u64; 6];
        for part in &kept {
            for (total, value) in grades.iter_mut().zip(part.grades.unwrap_or_default()) {
                *total += value;
            }
        }
        record.insert("grades_m".into(), json!(grades));
    }
    if let Some(hardest) = kept.iter().filter_map(|k| k.record.get("hardest").and_then(Value::as_u64)).max() {
        record.insert("hardest".into(), json!(hardest));
    }
    let cells: BTreeSet<_> = kept.iter().flat_map(|k| k.cells.iter().copied()).collect();
    record.insert("cells".into(), json!(cell_ids(&cells)));
    let last = parts.len() - 1;
    let ends =
        (parts[0].kept.is_some() && parts[last].kept.is_some()).then(|| (way(0).unwrap().0, way(last).unwrap().1));
    let closed = ends.is_some_and(|(start, finish)| {
        start == finish || tag(&relation.tags, "roundtrip") == "yes" && distance(start, finish) <= ROUNDTRIP_M
    });
    record.insert("loop".into(), json!(closed));
    let mut end_m = 0;
    let entries: Vec<Value> = (0..parts.len())
        .map(|i| {
            end_m += parts[i].length().unwrap_or(0);
            entry(&parts[i], end_m, flags[i])
        })
        .collect();
    record.insert("parts".into(), json!(entries));
    let stages: BTreeSet<usize> = parts.iter().filter_map(|part| part.stage).collect();
    record.insert("stage_count".into(), json!(stages.len()));
    record.insert("trail_profile_m".into(), json!(trail_profile(parts, &flags)));
    let whole = parts.iter().all(|part| !part.cut);
    if !connector && whole && kept.len() == parts.len() && flags.iter().all(|&(backwards, jump)| !backwards && !jump) {
        record.insert("stages".into(), json!(parts.iter().map(|part| part.id).collect::<Vec<_>>()));
        record.insert("start_udeg".into(), json!(way(0).unwrap().0));
    }
    let joined =
        |i: usize| parts[i - 1].kept.is_some() && parts[i].kept.is_some() && way(i - 1).unwrap().1 == way(i).unwrap().0;
    let points = kept.iter().map(|k| k.points).sum::<usize>() - (1..parts.len()).filter(|&i| joined(i)).count();
    Some((record, points))
}

/// One object of `parts`.
fn entry(part: &Part, end_m: u64, (backwards, jump): (bool, bool)) -> Value {
    let mut entry = Map::new();
    entry.insert("id".into(), json!(part.id));
    if let Some(stage) = part.stage {
        entry.insert("stage".into(), json!(stage));
    }
    entry.insert("end_m".into(), json!(end_m));
    if !part.name.is_empty() {
        entry.insert("name".into(), json!(part.name));
    }
    if let Some(length) = part.length() {
        entry.insert("length_m".into(), json!(length));
    }
    match part.kept {
        Some(kept) => {
            entry.insert("ascent_m".into(), kept.record["ascent_m"].clone());
            entry.insert("cells".into(), kept.record["cells"].clone());
        }
        None => {
            entry.insert("missing".into(), json!(true));
            if let Some((_, start, end)) = part.line {
                entry.insert("start_udeg".into(), json!(start));
                entry.insert("end_udeg".into(), json!(end));
            }
        }
    }
    for (flag, set) in [("cut", part.cut), ("backwards", backwards), ("jump", jump)] {
        if set {
            entry.insert(flag.into(), json!(true));
        }
    }
    Value::Object(entry)
}

/// Whether each part runs backwards, and whether it has a jump. A part with no ends has neither.
/// The known end before a part is the end, in the trail direction, of the part right before it.
fn order(parts: &[Part]) -> Vec<(bool, bool)> {
    let ends: Vec<Option<Ends>> = parts.iter().map(Part::ends).collect();
    let (mut known, mut kept_before) = (None, false);
    let mut flags = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        let flag = match (ends[i], known) {
            (None, _) => (false, false),
            (Some(e), Some(end)) if gap(e, end) <= MAX_GAP_M => {
                (!e.closed && distance(e.finish, end) < distance(e.start, end), false)
            }
            // The first part of a run.
            (Some(e), _) => (leads_back(e, ends.get(i + 1).copied().flatten()), kept_before),
        };
        known = ends[i].map(|e| if flag.0 { e.start } else { e.finish });
        kept_before |= part.kept.is_some();
        flags.push(flag);
    }
    flags
}

/// The distance from a point to the nearer end of a part.
fn gap(part: Ends, to: P) -> f64 {
    distance(part.start, to).min(distance(part.finish, to))
}

/// Whether a part joins the next part with its start, within `MAX_GAP_M`, and not with its finish.
fn leads_back(part: Ends, next: Option<Ends>) -> bool {
    next.is_some_and(|next| {
        let (start, finish) = (gap(next, part.start), gap(next, part.finish));
        !part.closed && start < finish && start <= MAX_GAP_M
    })
}

/// `TRAIL_PROFILE` heights at equal shares of the whole trail, from the `profile_m` of each kept
/// part in the trail direction.
fn trail_profile(parts: &[Part], flags: &[(bool, bool)]) -> Vec<Value> {
    let total: u64 = parts.iter().filter_map(Part::length).sum();
    let height = |k: usize| {
        // A share is exact at the last entry, so the end of the trail falls on its last part.
        let mut at = total as f64 * k as f64 / (TRAIL_PROFILE - 1) as f64;
        for (part, &(backwards, _)) in parts.iter().zip(flags) {
            let length = part.length().unwrap_or(0) as f64;
            if length > 0.0 && at <= length {
                let profile = part.kept.and_then(|k| k.record.get("profile_m")).and_then(Value::as_array);
                return profile.map_or(Value::Null, |profile| {
                    let along = if backwards { length - at } else { at };
                    // The nearest entry: one at each `PROFILE_STEP_M`, and the last at the end.
                    let place = |j: usize| if j + 1 == profile.len() { length } else { j as f64 * PROFILE_STEP_M };
                    let j = ((along / PROFILE_STEP_M) as usize).min(profile.len() - 2);
                    profile[if along - place(j) <= place(j + 1) - along { j } else { j + 1 }].clone()
                });
            }
            at -= length;
        }
        Value::Null
    };
    (0..TRAIL_PROFILE).map(height).collect()
}
