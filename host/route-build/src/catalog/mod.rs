//! The route catalog: the signed routes of a region, each with the plan that reproduces it.
//! `specs/route-catalog.md` is the contract.
mod assemble;

use crate::source::{Data, Id, Relation, Tags};
use route_engine::{
    data::{RoutingData, Selection},
    directory::Directory,
    model::Profile,
    package::Source,
    shape::{self, distance, Failure, Limits, P},
    Control, Router,
};
use serde_json::{json, Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    path::Path,
    sync::Mutex,
};

pub const FILE: &str = "route-catalog.json";
const MIN_LENGTH_M: f64 = 2_000.0;
const MAX_GAP_M: f64 = 500.0;
const ROUNDTRIP_M: f64 = 200.0;
const DESCRIPTION_CHARS: usize = 200;
/// The memory of all routers together; it limits the number of workers.
const ROUTERS_MEMORY: usize = 6 << 30;
/// The profiles a route is checked with.
const PROFILES: [&str; 5] = ["touring", "road", "gravel", "mtb", "hiking"];
/// The line keeps every point of the plan route within this distance.
const LINE_TOLERANCE_M: f64 = 50.0;
/// Router calls for the search of one route, for each profile that it checks. A search that needs
/// more does not converge, and the route leaves the catalog; the bound keeps the bake of a large
/// region within minutes.
const SEARCH_CALLS: usize = 400;
/// The search queues of one route request.
const SEARCH_BYTES: usize = 768 << 20;

/// The router control of the catalog: a fixed search space, so that a router limit does not
/// depend on the order of the routes.
fn control() -> Control<'static> {
    Control { max_heap_bytes: SEARCH_BYTES, ..Control::default() }
}

/// Why a selected relation is not in the catalog.
#[derive(Clone, Copy, Debug)]
pub enum Reason {
    ExtractEdge,
    Short,
    Gap,
    UnroutableGap,
    TooManyPoints,
    ShapingCheck,
    ShapingStall,
    RouterLimit,
    SearchBudget,
    NestedLongRoute,
    NoStages,
    MissingStage,
}

fn tag<'a>(tags: &'a Tags, key: &str) -> &'a str {
    tags.get(key).map(String::as_str).unwrap_or("")
}

/// The `route` value of a relation that the catalog selects by its tags.
fn selected(tags: &Tags) -> Option<&str> {
    let kind = tag(tags, "route");
    (matches!(tag(tags, "type"), "route" | "superroute")
        && matches!(kind, "hiking" | "foot" | "bicycle" | "mtb")
        && !(tag(tags, "name").is_empty() && tag(tags, "ref").is_empty())
        && !matches!(tag(tags, "network:type"), "node_network" | "basic_network")
        && !matches!(tag(tags, "state"), "proposed" | "planned" | "disused" | "abandoned"))
    .then_some(kind)
}

/// The Balanced profiles of the activities that list a kind. The first gives the plan route.
fn profiles(kind: &str) -> &'static [&'static str] {
    match kind {
        "bicycle" => &["touring", "road", "gravel"],
        "mtb" => &["mtb"],
        _ => &["hiking"],
    }
}

fn graded(kind: &str) -> bool {
    kind != "bicycle"
}

fn rank(network: &str) -> u8 {
    match network {
        "iwn" | "icn" => 4,
        "nwn" | "ncn" => 3,
        "rwn" | "rcn" => 2,
        "lwn" | "lcn" => 1,
        _ => 0,
    }
}

fn description(text: &str) -> &str {
    let Some((end, _)) = text.char_indices().nth(DESCRIPTION_CHARS) else { return text };
    // A space right after the limit also ends a word.
    let head = &text[..end + text[end..].chars().next().map_or(0, char::len_utf8)];
    match head.rfind(char::is_whitespace) {
        Some(space) if space > 0 => head[..space].trim_end(),
        _ => &text[..end],
    }
}

/// The OSM tag fields of a record.
fn header(relation: &Relation, kind: &str, france: bool) -> Map<String, Value> {
    let tags = &relation.tags;
    let mut record = Map::new();
    record.insert("id".into(), json!(relation.id));
    record.insert("kind".into(), json!(kind));
    record.insert("rank".into(), json!(rank(tag(tags, "network"))));
    let website = ["website", "contact:website", "url"].into_iter().map(|key| tag(tags, key)).find(|w| !w.is_empty());
    let symbol = if france { "" } else { tag(tags, "osmc:symbol") };
    for (field, value) in [
        ("name", tag(tags, "name")),
        ("ref", tag(tags, "ref")),
        ("operator", tag(tags, "operator")),
        ("description", description(tag(tags, "description"))),
        ("website", website.unwrap_or("")),
        ("symbol", symbol),
    ] {
        if !value.is_empty() {
            record.insert(field.into(), json!(value));
        }
    }
    record
}

/// The zoom 9 cells that the bounding box of each segment touches, edges included.
fn cells(line: &[P], into: &mut BTreeSet<(u32, u32)>) {
    let tile = |p: P| {
        let (lon, lat) = (p[0] as f64 * 1e-6, (p[1] as f64 * 1e-6).to_radians());
        [(lon + 180.0) / 360.0 * 512.0, (1.0 - lat.tan().asinh() / std::f64::consts::PI) / 2.0 * 512.0]
    };
    let range = |a: f64, b: f64| (a.min(b).ceil() as i64 - 1).max(0)..=(a.max(b).floor() as i64).min(511);
    for w in line.windows(2).chain(std::iter::once(&line[..1]).filter(|_| line.len() == 1)) {
        let (a, b) = (tile(w[0]), tile(w[w.len() - 1]));
        for x in range(a[0], b[0]) {
            for y in range(a[1], b[1]) {
                into.insert((x as u32, y as u32));
            }
        }
    }
}

fn cell_ids(cells: &BTreeSet<(u32, u32)>) -> Vec<String> {
    let mut ids: Vec<_> = cells.iter().map(|(x, y)| format!("9-{x}-{y}")).collect();
    ids.sort();
    ids
}

fn encode(line: &[P]) -> Vec<i64> {
    let mut previous = [0i64; 2];
    line.iter()
        .flat_map(|p| {
            let delta = [p[0] as i64 - previous[0], p[1] as i64 - previous[1]];
            previous = [p[0] as i64, p[1] as i64];
            delta
        })
        .collect()
}

/// A kept route, as its long route needs it.
struct Kept {
    record: Map<String, Value>,
    start: P,
    finish: P,
    points: usize,
    cells: BTreeSet<(u32, u32)>,
    grades: Option<[u64; 6]>,
}

/// One route relation with its main-line members.
struct Job<'a> {
    relation: &'a Relation,
    kind: String,
    members: Vec<assemble::Member>,
}

/// The facts that the report needs, besides the records.
#[derive(Default)]
pub struct Report {
    pub tag_selected: usize,
    pub dropped: BTreeMap<String, Vec<i64>>,
    pub kept: BTreeMap<String, usize>,
    pub loops: usize,
    pub long_routes: usize,
    pub stages: usize,
    pub via: Vec<usize>,
    pub long_points: Vec<(i64, usize)>,
    /// The search of each route: relation ID, seconds and router calls.
    pub searches: Vec<(i64, f64, usize)>,
    pub workers: usize,
    /// Wall time of the parallel search.
    pub search_seconds: f64,
    pub seconds: f64,
}

/// The median, the 90th percentile and the maximum.
fn spread(mut values: Vec<f64>) -> Value {
    values.sort_by(f64::total_cmp);
    let at = |q: f64| values.get(((values.len() as f64 * q) as usize).min(values.len().saturating_sub(1))).copied();
    json!({"median": at(0.5), "p90": at(0.9), "max": values.last()})
}

impl Report {
    pub fn to_json(&self) -> Value {
        let mut slowest = self.searches.clone();
        slowest.sort_by(|a, b| b.1.total_cmp(&a.1));
        let busy: f64 = self.searches.iter().map(|s| s.1).sum();
        json!({
            "tag_selected": self.tag_selected, "dropped": self.dropped, "kept": self.kept, "loops": self.loops,
            "long_routes": self.long_routes, "stages": self.stages,
            "via": spread(self.via.iter().map(|&v| v as f64).collect()),
            "search": {
                "seconds": spread(self.searches.iter().map(|s| s.1).collect()),
                "calls": spread(self.searches.iter().map(|s| s.2 as f64).collect()),
                "slowest": slowest.iter().take(10).map(|s| json!([s.0, s.1, s.2])).collect::<Vec<_>>(),
                "workers": self.workers,
                "utilisation": busy / (self.workers as f64 * self.search_seconds).max(f64::MIN_POSITIVE),
                "wall_seconds": self.search_seconds,
            },
            "long_routes_over_64_points": self.long_points.iter().filter(|(_, n)| *n > 64).count(),
            "seconds": self.seconds,
        })
    }
}

/// Checks the catalog options before the import. The answer says whether the records leave out
/// route marks: when France is the only country.
pub fn check(countries: &[String], profiles: &[Profile]) -> Result<bool, String> {
    if let Some(profile) = PROFILES.iter().find(|&&name| !profiles.iter().any(|p| p.name == name)) {
        return Err(format!("The route catalog needs the {profile} profile"));
    }
    match countries {
        [only] => Ok(only == "FR"),
        _ if countries.iter().any(|c| c == "FR") => {
            Err("A region with France and another country needs a country lookup for route marks".into())
        }
        _ => Ok(false),
    }
}

/// Writes `route-catalog.json` into the routing package in `directory`, from the source OSM
/// objects of its import. `france` comes from `check`.
pub fn write(directory: &Path, osm: Data, france: bool) -> Result<Report, String> {
    let started = std::time::Instant::now();
    let package = Directory::open(directory).map_err(|e| e.to_string())?;
    // The routers share one graph, one junction mapping and the five prepared profiles; each
    // adds its own label blocks and a search space of `SEARCH_BYTES`.
    let routing = Selection::whole(package);
    let shared = routing.shared_bytes(PROFILES).map_err(|e| e.to_string())?;
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let workers = cpus.min(ROUTERS_MEMORY.saturating_sub(shared) / (routing.router_bytes() + SEARCH_BYTES)).max(1);
    let (records, report) = catalog(&routing, osm, france, workers)?;
    let bytes = serde_json::to_vec(&json!({"format": 1, "routes": records})).map_err(|e| e.to_string())?;
    std::fs::write(directory.join(FILE), bytes).map_err(|e| e.to_string())?;
    Ok(Report { seconds: started.elapsed().as_secs_f64(), ..report })
}

fn catalog<S: Source + Clone + Send>(
    selection: &Selection<S>,
    osm: Data,
    france: bool,
    workers: usize,
) -> Result<(Vec<Value>, Report), String> {
    let Data { nodes, mut ways, relations } = osm;
    let mut report = Report::default();
    let mut drops = BTreeMap::<i64, Reason>::new();
    let is_route = |id: &i64| relations.get(id).is_some_and(|r| matches!(tag(&r.tags, "type"), "route" | "superroute"));
    let children = |relation: &Relation| -> Vec<i64> {
        relation
            .members
            .iter()
            .filter(|(_, role)| matches!(role.as_str(), "" | "main"))
            .filter_map(|(id, _)| if let Id::Relation(id) = id { Some(*id) } else { None })
            .collect()
    };
    let mut long = Vec::new();
    let mut jobs = Vec::new();
    for relation in relations.values() {
        let Some(kind) = selected(&relation.tags) else { continue };
        report.tag_selected += 1;
        let children = children(relation);
        if children.iter().any(|id| !relations.contains_key(id)) {
            drops.insert(relation.id, Reason::ExtractEdge);
        } else if children.iter().any(is_route) {
            long.push((relation, kind.to_string()));
        } else {
            jobs.push(Job { relation, kind: kind.into(), members: Vec::new() });
        }
    }
    let needed: HashSet<i64> = jobs
        .iter()
        .flat_map(|job| &job.relation.members)
        .filter(|(_, role)| assemble::is_main(role))
        .filter_map(|(id, _)| if let Id::Way(id) = id { Some(*id) } else { None })
        .collect();
    ways.retain(|id, _| needed.contains(id));
    let needed: HashSet<i64> = ways.values().flat_map(|w| w.nodes.iter().copied()).collect();
    let points: HashMap<i64, P> =
        nodes.into_values().filter(|n| needed.contains(&n.id)).map(|n| (n.id, [n.point.lon, n.point.lat])).collect();
    jobs.retain_mut(|job| {
        for (id, role) in &job.relation.members {
            let Id::Way(id) = id else { continue };
            if !assemble::is_main(role) {
                continue;
            }
            match ways.get(id) {
                Some(way) if way.nodes.iter().all(|n| points.contains_key(n)) => job.members.push(assemble::Member {
                    nodes: way.nodes.clone(),
                    role: role.clone(),
                    roundabout: matches!(tag(&way.tags, "junction"), "roundabout" | "circular"),
                }),
                _ => {
                    drops.insert(job.relation.id, Reason::ExtractEdge);
                    return false;
                }
            }
        }
        true
    });
    drop(ways);
    // The largest routes go first, so that the long searches run side by side.
    jobs.sort_by_key(|job| job.members.iter().map(|m| m.nodes.len()).sum::<usize>());
    let queue = Mutex::new(jobs);
    let started = std::time::Instant::now();
    let results = Mutex::new(Vec::new());
    let mut forks: Vec<_> = (0..workers.max(1)).map(|_| selection.fork()).collect();
    std::thread::scope(|scope| {
        for routing in forks.drain(..) {
            let (queue, results, points) = (&queue, &results, &points);
            scope.spawn(move || {
                // The search space is capped by `control`, not by the budget, so that it
                // does not depend on the cost tables in the cache.
                let mut router = Router::new(routing, usize::MAX);
                loop {
                    // The guard must drop before the work, so the job is taken in its own statement.
                    let job = queue.lock().unwrap().pop();
                    let Some(job) = job else { break };
                    let begun = std::time::Instant::now();
                    let mut calls = 0;
                    let result = route(&mut router, &job, points, france, &mut calls);
                    let search = (job.relation.id, begun.elapsed().as_secs_f64(), calls);
                    results.lock().unwrap().push((search, result));
                }
            });
        }
    });
    let mut kept = BTreeMap::new();
    report.workers = workers.max(1);
    report.search_seconds = started.elapsed().as_secs_f64();
    for (search, result) in results.into_inner().unwrap() {
        let id = search.0;
        report.searches.push(search);
        match result {
            Ok(route) => {
                kept.insert(id, route);
            }
            Err(reason) => {
                drops.insert(id, reason);
            }
        }
    }
    // Long routes: their stages are kept routes; they get a record and their stages a parent.
    let mut parents = BTreeMap::<i64, (i64, usize)>::new();
    let mut records = Vec::new();
    for (relation, kind) in long {
        let network = tag(&relation.tags, "network");
        let stages: Vec<i64> = children(relation)
            .into_iter()
            .filter(|id| tag(&relations[id].tags, "network") == network && is_route(id))
            .collect();
        let reason = if children(relation).iter().any(|id| children(&relations[id]).iter().any(is_route)) {
            Some(Reason::NestedLongRoute)
        } else if stages.is_empty() {
            Some(Reason::NoStages)
        } else if stages.iter().any(|id| !kept.contains_key(id)) {
            Some(Reason::MissingStage)
        } else {
            None
        };
        if let Some(reason) = reason {
            drops.insert(relation.id, reason);
            continue;
        }
        let parts: Vec<&Kept> = stages.iter().map(|id| &kept[id]).collect();
        let (start, finish) = (parts[0].start, parts[parts.len() - 1].finish);
        let roundtrip = tag(&relation.tags, "roundtrip") == "yes" && distance(start, finish) <= ROUNDTRIP_M;
        let joins = parts.windows(2).filter(|w| w[0].finish == w[1].start).count();
        report.long_points.push((relation.id, parts.iter().map(|k| k.points).sum::<usize>() - joins));
        let mut record = header(relation, &kind, france);
        let sum = |field: &str| parts.iter().map(|k| k.record[field].as_u64().unwrap()).sum::<u64>();
        record.insert("loop".into(), json!(start == finish || roundtrip));
        for field in ["length_m", "ascent_m", "descent_m"] {
            record.insert(field.into(), json!(sum(field)));
        }
        if graded(&kind) {
            let mut grades = [0u64; 6];
            for part in &parts {
                for (total, value) in grades.iter_mut().zip(part.grades.unwrap_or_default()) {
                    *total += value;
                }
            }
            record.insert("grades_m".into(), json!(grades));
        }
        if let Some(hardest) = parts.iter().filter_map(|k| k.record.get("hardest").and_then(Value::as_u64)).max() {
            record.insert("hardest".into(), json!(hardest));
        }
        let cells: BTreeSet<_> = parts.iter().flat_map(|k| k.cells.iter().copied()).collect();
        record.insert("cells".into(), json!(cell_ids(&cells)));
        record.insert("stages".into(), json!(stages));
        record.insert("start_udeg".into(), json!(start));
        for (number, id) in stages.iter().enumerate() {
            parents.entry(*id).or_insert((relation.id, number + 1));
        }
        *report.kept.entry(kind.clone()).or_default() += 1;
        report.long_routes += 1;
        report.loops += usize::from(start == finish || roundtrip);
        records.push(Value::Object(record));
    }
    for (id, mut route) in kept {
        if let Some((parent, number)) = parents.get(&id) {
            route.record.insert("parent".into(), json!(parent));
            route.record.insert("stage".into(), json!(number));
            report.stages += 1;
        }
        let kind = route.record["kind"].as_str().unwrap().to_string();
        *report.kept.entry(kind).or_default() += 1;
        report.loops += usize::from(route.record["loop"] == true);
        report.via.push(route.points - 2);
        records.push(Value::Object(route.record));
    }
    records.sort_by_key(|record| record["id"].as_i64());
    for (id, reason) in drops {
        report.dropped.entry(format!("{reason:?}")).or_default().push(id);
    }
    Ok((records, report))
}

/// Builds the record of one route, or the reason it leaves the catalog.
fn route<D: RoutingData>(
    router: &mut Router<D>,
    job: &Job,
    points: &HashMap<i64, P>,
    france: bool,
    calls: &mut usize,
) -> Result<Kept, Reason> {
    let apart = |a: i64, b: i64| distance(points[&a], points[&b]);
    let mut runs = assemble::main_line(&job.members, &apart);
    // A shuffled member order with small jumps would retrace itself; the nearest-end chain does not.
    if runs.len() > 1 {
        match assemble::chain(&runs, &apart, MAX_GAP_M) {
            Some(chained) => runs = chained,
            None if runs.windows(2).all(|w| apart(w[0][w[0].len() - 1], w[1][0]) <= MAX_GAP_M) => {}
            None => return Err(Reason::Gap),
        }
    }
    let runs: Vec<Vec<P>> = runs.iter().map(|run| run.iter().map(|n| points[n]).collect()).collect();
    let length: f64 = runs.iter().map(|run| shape::length(run)).sum();
    if length < MIN_LENGTH_M {
        return Err(Reason::Short);
    }
    let profiles = profiles(&job.kind);
    let mut patch = |line: &mut Vec<P>, to: P| -> Result<(), Reason> {
        let from = line[line.len() - 1];
        if from == to {
            return Ok(());
        }
        if distance(from, to) > MAX_GAP_M {
            return Err(Reason::Gap);
        }
        let route =
            router.route(&shape::request(profiles[0], &[from, to], vec![]), &control()).map_err(
                |error| match error {
                    route_engine::Error::Limit => Reason::RouterLimit,
                    _ => Reason::UnroutableGap,
                },
            )?;
        line.extend(shape::vertices(&route).into_iter().chain([to]));
        line.dedup();
        Ok(())
    };
    let mut line = runs[0].clone();
    for run in &runs[1..] {
        patch(&mut line, run[0])?;
        line.extend(&run[1..]);
    }
    let (start, end) = (line[0], line[line.len() - 1]);
    let closed = start == end
        || tag(&job.relation.tags, "roundtrip") == "yes" && distance(start, end) <= ROUNDTRIP_M && {
            patch(&mut line, start).is_ok()
        };
    // An out-and-back spur retraces its nodes; its tip is a turnaround.
    let tips: Vec<usize> = (1..line.len().saturating_sub(1)).filter(|&k| line[k - 1] == line[k + 1]).collect();
    let mut limits = Limits { control: control(), max_calls: SEARCH_CALLS * profiles.len(), calls: 0 };
    let plan = shape::shape(router, profiles, &line, &tips, length, closed, &mut limits);
    *calls = limits.calls;
    let plan = plan.map_err(|failure| match failure {
        Failure::TooManyPoints => Reason::TooManyPoints,
        Failure::Check => Reason::ShapingCheck,
        // The catalog control never cancels.
        Failure::Limit | Failure::Cancelled => Reason::RouterLimit,
        Failure::Budget => Reason::SearchBudget,
        Failure::Stall => Reason::ShapingStall,
    })?;
    let geometry = shape::vertices(&plan.route);
    let keep: Vec<usize> = plan.route.legs.iter().map(|leg| leg.from_index).chain([geometry.len() - 1]).collect();
    let (mut simple, positions) = shape::simplify(&geometry, &keep, LINE_TOLERANCE_M);
    if closed {
        let first = simple[0];
        *simple.last_mut().unwrap() = first;
    }
    let mut cover = BTreeSet::new();
    cells(&geometry, &mut cover);
    let mut record = header(job.relation, &job.kind, france);
    let totals = &plan.route.totals;
    record.insert("loop".into(), json!(closed));
    record.insert("length_m".into(), json!(totals.distance_m));
    record.insert("ascent_m".into(), json!(totals.ascent_m));
    record.insert("descent_m".into(), json!(totals.descent_m));
    let mut grades = None;
    if graded(&job.kind) {
        let (lengths, hardest) = grade_lengths(&plan.route, &geometry, job.kind == "mtb");
        record.insert("grades_m".into(), json!(lengths));
        if let Some(hardest) = hardest {
            record.insert("hardest".into(), json!(hardest));
        }
        grades = Some(lengths);
    }
    record.insert("cells".into(), json!(cell_ids(&cover)));
    record.insert("line_udeg".into(), json!(encode(&simple)));
    let via = &positions[1..positions.len() - 1];
    record.insert("via".into(), json!(via));
    let mut turnarounds: Vec<usize> = plan.turnarounds.iter().map(|&i| positions[i]).collect();
    // A loop that leaves and returns on the same road turns back at its start; a client that
    // rotates the loop needs to know.
    let n = geometry.len();
    if closed && n > 3 && distance(geometry[1], geometry[n - 2]) <= 1.0 {
        turnarounds.insert(0, 0);
    }
    if !turnarounds.is_empty() {
        record.insert("turnarounds".into(), json!(turnarounds));
    }
    Ok(Kept {
        record,
        start: simple[0],
        finish: simple[simple.len() - 1],
        points: plan.points.len(),
        cells: cover,
        grades,
    })
}

/// The length of the plan route with each grade, and the hardest explicit grade, from the grade
/// channel of the route edges.
fn grade_lengths(route: &route_engine::Route, geometry: &[P], mtb: bool) -> ([u64; 6], Option<usize>) {
    let runs = route.edges.runs(if mtb { "mtb_scale" } else { "sac_scale" });
    let values = runs.iter().flat_map(|(value, edges)| std::iter::repeat_n(value.as_u64(), *edges));
    let mut lengths = [0.0f64; 6];
    let mut hardest = None;
    for (w, value) in geometry.windows(2).zip(values) {
        let explicit = value.filter(|&d| d <= 6).map(|d| if mtb { d.min(5) } else { d.max(1) - 1 } as usize);
        let length = distance(w[0], w[1]);
        lengths[explicit.unwrap_or(0)] += length;
        if length > 0.0 {
            hardest = hardest.max(explicit);
        }
    }
    // Scaled to the route length, so that the six rounded lengths add up to it within 3 m.
    let sum: f64 = lengths.iter().sum();
    let scale = if sum > 0.0 { route.totals.distance_m as f64 / sum } else { 0.0 };
    (lengths.map(|l| (l * scale).round() as u64), hardest)
}

#[cfg(test)]
mod tests;
