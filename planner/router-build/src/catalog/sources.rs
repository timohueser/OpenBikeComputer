//! The jobs of the second route sources: their routes and stages from `route-sources.json`, and the
//! line of each, with the stretches away from the roads of its kind cut out.
//! `specs/route-catalog.md#second-sources` is the rule.
use super::{dense, ends, profiles, runs, Failed, Line, Reason, SourceLine, MAX_GAP_M, ROUNDTRIP_M};
use crate::facts::shares::Roads;
use planner_router::{
    package::{Package, Source},
    shape::{distance, NEAR_M, P},
};
use serde_json::{Map, Value};
use std::{collections::HashMap, path::Path};

/// The file of the step `planner/route-sources`.
pub const FILE: &str = "route-sources.json";
/// The `id` of a source record is −(origin × `ORIGIN_ID` + key × 100 + stage), so it is at most
/// −`ORIGIN_ID`, below the `id` of each section.
pub const ORIGIN_ID: i64 = 1_000_000_000_000;
/// A stretch near a road between two cuts that is shorter than this belongs to the cut, so a lift
/// or a boat line that crosses a path is one cut.
const CROSSING_M: f64 = 100.0;
/// The words of a source route that a record takes.
const WORDS: [&str; 3] = ["description", "operator", "website"];

/// One job of a second source: a route without stages, or one stage of a route.
pub(super) struct SourceJob {
    /// The place of its source in the file: the rank of its origin.
    pub source: usize,
    pub origin: u64,
    pub key: u64,
    /// The stage number; 0 for a route.
    pub stage: u64,
    pub kind: String,
    pub rank: u8,
    pub ref_: String,
    pub name: String,
    /// The name of its route, which the merge compares.
    pub route: String,
    /// `description`, `operator` and `website`.
    pub words: Map<String, Value>,
    pub credit: String,
    pub members: Vec<Vec<P>>,
}

impl SourceJob {
    pub fn id(&self) -> i64 {
        -((self.origin * ORIGIN_ID as u64 + self.key * 100 + self.stage) as i64)
    }

    /// The fields of its record that do not come from its plan.
    pub fn header(&self) -> Map<String, Value> {
        let mut record = self.words.clone();
        if let Some(text) = self.words.get("description").and_then(Value::as_str) {
            record.insert("description".into(), super::description(text).into());
        }
        record.insert("id".into(), self.id().into());
        record.insert("kind".into(), self.kind.clone().into());
        record.insert("rank".into(), self.rank.into());
        record.insert("credit".into(), self.credit.clone().into());
        for (field, value) in [("name", &self.name), ("ref", &self.ref_)] {
            if !value.is_empty() {
                record.insert(field.into(), value.clone().into());
            }
        }
        record
    }
}

/// A route of a second source with stages: the long record of its stages.
pub(super) struct SourceTrail {
    /// The route as a job of stage 0 with no members, with its own words: the `id` and the fields of its long record.
    pub route: SourceJob,
    /// The number and the `name` of each stage row, with or without members, in the order of the numbers.
    pub stages: Vec<(u64, String)>,
}

impl SourceTrail {
    /// The `id` of the record of stage `number`.
    pub fn stage_id(&self, number: u64) -> i64 {
        self.route.id() - number as i64
    }
}

/// The ids of the sources of a `route-sources.json`, in its order, their jobs and their routes with stages.
#[derive(Default)]
pub struct Sources {
    pub(super) ids: Vec<String>,
    pub(super) jobs: Vec<SourceJob>,
    pub(super) trails: Vec<SourceTrail>,
}

/// The sources and jobs of the `route-sources.json` at `path`.
pub fn read(path: &Path) -> Result<Sources, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    jobs(&serde_json::from_slice(&bytes).map_err(|e| format!("{FILE}: {e}"))?)
}

fn text(value: &Value, field: &str) -> String {
    value[field].as_str().unwrap_or("").to_string()
}

/// The jobs of a `route-sources.json`: each route without stages, and each stage with members; and each route with
/// stages.
pub(super) fn jobs(file: &Value) -> Result<Sources, String> {
    if file["format"] != 1 {
        return Err(format!("{FILE} has the format {}, which this builder does not read", file["format"]));
    }
    let mut sources = Sources::default();
    for (place, source) in file["sources"].as_array().into_iter().flatten().enumerate() {
        sources.ids.push(text(source, "id"));
        let origin = source["origin"].as_u64().ok_or(format!("{FILE}: a source has no origin"))?;
        for route in source["routes"].as_array().into_iter().flatten() {
            let key = route["key"].as_u64().ok_or(format!("{FILE}: a route has no key"))?;
            let job = |part: &Value, stage: u64| {
                // A stage takes each word of its route that it lacks.
                let word =
                    |field: &str| part.get(field).or(route.get(field)).map(|value| (field.into(), value.clone()));
                let words: Map<String, Value> = WORDS.iter().filter_map(|field| word(field)).collect();
                let members = part["members"].as_array().into_iter().flatten().map(|line| {
                    let numbers: Vec<i32> =
                        line.as_array().into_iter().flatten().filter_map(Value::as_i64).map(|n| n as i32).collect();
                    numbers.as_chunks::<2>().0.to_vec()
                });
                SourceJob {
                    source: place,
                    origin,
                    key,
                    stage,
                    kind: text(route, "kind"),
                    rank: route["rank"].as_u64().unwrap_or(0) as u8,
                    ref_: text(route, "ref"),
                    name: text(part, "name"),
                    route: text(route, "name"),
                    words,
                    credit: text(source, "credit"),
                    members: members.filter(|line| line.len() >= 2).collect(),
                }
            };
            match route["stages"].as_array() {
                Some(stages) => {
                    let stages: Vec<SourceJob> =
                        stages.iter().map(|stage| job(stage, stage["stage"].as_u64().unwrap_or(0))).collect();
                    let rows = stages.iter().map(|stage| (stage.stage, stage.name.clone())).collect();
                    sources.trails.push(SourceTrail { route: job(route, 0), stages: rows });
                    sources.jobs.extend(stages.into_iter().filter(|job| !job.members.is_empty()));
                }
                None => sources.jobs.push(job(route, 0)),
            }
        }
    }
    Ok(sources)
}

/// The bit of the first plan profile of each kind in the value of `usable_ways`.
fn bit(kind: &str) -> u8 {
    match profiles(kind)[0] {
        "touring" => 1,
        "mtb/technical" => 2,
        _ => 4,
    }
}

/// The OSM ways that the first plan profile of each kind may use, with a bit of each such profile
/// (`bit`): one pass over the roads of the package, in either direction.
pub(super) fn usable_ways<S: Source>(package: &Package<S>) -> Result<HashMap<i64, u8>, String> {
    let first = ["bicycle", "mtb", "hiking"].map(|kind| (profiles(kind)[0], bit(kind)));
    let mut usable = HashMap::new();
    for road in 0..package.manifest().roads {
        let mut bits = 0;
        for (profile, bit) in first {
            bits |= if package.allowed(profile, road).map_err(|e| e.to_string())? { bit } else { 0 };
        }
        if bits != 0 {
            let way = package.with_road(road, |road| road.way).map_err(|e| e.to_string())?;
            *usable.entry(way).or_default() |= bits;
        }
    }
    Ok(usable)
}

/// The match line of a source job (the runs of its main line before the cut, also when the cut drops the job), and its
/// line with the length of each cut. The points of the members are nodes, one for each microdegree point, so members
/// that meet at a point join there. A point outside `bounds` drops the job.
pub(super) fn line(job: &SourceJob, bounds: [f64; 4], roads: &Roads, usable: &HashMap<i64, u8>) -> SourceLine {
    let [west, south, east, north] = bounds.map(|degrees| (degrees * 1e6).round() as i32);
    let (mut nodes, mut points) = (HashMap::<P, i64>::new(), HashMap::new());
    let mut members = Vec::new();
    for member in &job.members {
        if member.iter().any(|p| p[0] < west || p[0] > east || p[1] < south || p[1] > north) {
            return (Vec::new(), Err(Failed { reason: Reason::ExtractEdge, line: None }));
        }
        let ids = member.iter().map(|&p| {
            let next = nodes.len() as i64;
            let id = *nodes.entry(p).or_insert(next);
            points.insert(id, p);
            id
        });
        members.push(super::assemble::Member { nodes: ids.collect(), role: String::new(), roundabout: false });
    }
    let main = match runs(&members, &points) {
        Ok((main, _)) if !main.is_empty() => main,
        Ok(_) => return (Vec::new(), Err(Failed { reason: Reason::Short, line: None })),
        Err(failed) => return (Vec::new(), Err(failed)),
    };
    let closed = distance(main[0][0], *main[main.len() - 1].last().unwrap()) <= ROUNDTRIP_M;
    let bit = bit(&job.kind);
    let near = |p: P| roads.within(p, NEAR_M, |way| usable.get(&way).is_some_and(|bits| bits & bit != 0));
    let line = match cut(&main, closed, &near) {
        Ok((kept, cuts)) => Ok((Line { runs: kept, own: main.concat(), close_m: closed.then_some(MAX_GAP_M) }, cuts)),
        Err(reason) => Err(Failed { reason, line: ends(&main) }),
    };
    (main, line)
}

/// The runs of `main` without its cuts, and the length of each cut: a cut is a stretch of points at
/// each 10 m that are not `near` a road, with each near stretch of less than `CROSSING_M` between
/// two of them. A cut longer than `MAX_GAP_M` drops the line, and so do a cut at the start and one
/// at the end of a loop that together are longer.
pub(super) fn cut(main: &[Vec<P>], closed: bool, near: &dyn Fn(P) -> bool) -> Result<(Vec<Vec<P>>, Vec<u64>), Reason> {
    let (mut kept, mut lengths) = (Vec::new(), Vec::new());
    let mut ends_m = [0.0; 2];
    let last = main.len() - 1;
    for (r, run) in main.iter().enumerate() {
        let points = dense(run);
        let mut along = vec![0.0];
        for w in points.windows(2) {
            along.push(along[along.len() - 1] + distance(w[0].0, w[1].0));
        }
        let far: Vec<bool> = points.iter().map(|&(p, _)| !near(p)).collect();
        // Each cut as the range of its far points, with the near stretches between them that are too short.
        let mut cuts: Vec<(usize, usize)> = Vec::new();
        for k in (0..points.len()).filter(|&k| far[k]) {
            match cuts.last_mut() {
                Some(cut) if along[k] - along[cut.1] < CROSSING_M => cut.1 = k,
                _ => cuts.push((k, k)),
            }
        }
        // From the last point before a cut to the first point after it, or to the end of the run.
        let length = |(a, b): (usize, usize)| along[(b + 1).min(points.len() - 1)] - along[a.saturating_sub(1)];
        if cuts.iter().any(|&cut| length(cut) > MAX_GAP_M) {
            return Err(Reason::Gap);
        }
        if let Some(&first) = cuts.first().filter(|cut| r == 0 && cut.0 == 0) {
            ends_m[0] = length(first);
        }
        if let Some(&end) = cuts.last().filter(|cut| r == last && cut.1 == points.len() - 1) {
            ends_m[1] = length(end);
        }
        lengths.extend(cuts.iter().map(|&cut| length(cut).round() as u64));
        let mut start = 0;
        for (a, b) in cuts.into_iter().chain([(points.len(), points.len())]) {
            if a > start {
                // The vertices of the stretch, and its first and last point.
                let end = a - start - 1;
                let stretch = points[start..a].iter().enumerate();
                kept.push(
                    stretch.filter(|&(i, &(_, vertex))| vertex || i == 0 || i == end).map(|(_, &(p, _))| p).collect(),
                );
            }
            start = b + 1;
        }
    }
    if closed && ends_m[0] + ends_m[1] > MAX_GAP_M {
        return Err(Reason::Gap);
    }
    match kept.is_empty() {
        true => Err(Reason::Short),
        false => Ok((kept, lengths)),
    }
}
