//! The route catalog: the signed routes of a region, each with the plan that reproduces it.
//! `specs/route-catalog.md` is the contract.
mod assemble;
mod connector;
mod long;
pub mod trails;

use crate::source::{Data, Id, Relation, Relations, Table, Tags};
use planner_router::{
    data::{RoutingData, Selection},
    directory::Directory,
    geometry,
    model::{Point, Profile, NO_ELEVATION},
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
/// The owner chose 1 km, so that short named walks are in. The connector rule, not the length,
/// keeps out the signed pieces between two signposts.
const MIN_LENGTH_M: f64 = 1_000.0;
const MAX_GAP_M: f64 = 500.0;
/// The owner's limit for the side branches of a route, all together, as a share of the rest.
const SIDE_TRIP_SHARE: f64 = 0.10;
/// The most main member ways of a route that the import does not hold, such as a park or a way with
/// no road tag; the line joins across them.
const MAX_SKIPPED_WAYS: usize = 2;
const ROUNDTRIP_M: f64 = 200.0;
const DESCRIPTION_CHARS: usize = 200;
/// The memory of all routers together; it limits the number of workers.
const ROUTERS_MEMORY: usize = 6 << 30;
/// The profiles a route is checked with.
const PROFILES: [&str; 5] = ["touring", "road", "gravel", "mtb/technical", "hiking/alpine"];
/// The line keeps every point of the plan route within this distance.
const LINE_TOLERANCE_M: f64 = 10.0;
/// The distance between two entries of `profile_m`.
const PROFILE_STEP_M: f64 = 100.0;
/// Router calls for the search of one route, for each profile that it checks. A search that needs
/// more does not converge, and the route leaves the catalog; the bound keeps the bake of a large
/// region within minutes.
const SEARCH_CALLS: usize = 400;
/// The search queues of one route request.
const SEARCH_BYTES: usize = 768 << 20;
/// The longest section of a cut line. The owner chose this length for the sections of long trails:
/// it is below the longest single line that the builder plans whole.
const SECTION_M: f64 = 60_000.0;
/// The `id` of a section holds its number in two decimal digits.
const MAX_SECTIONS: usize = 99;

/// The router control of the catalog: a fixed search space, so that a router limit does not
/// depend on the order of the routes.
fn control() -> Control<'static> {
    Control { max_heap_bytes: SEARCH_BYTES, ..Control::default() }
}

/// Why a selected relation is not in the catalog.
#[derive(Clone, Copy, Debug)]
pub enum Reason {
    Connector,
    ExtractEdge,
    Short,
    Gap,
    UnroutableGap,
    TooManyPoints,
    ShapingCheck,
    ShapingStall,
    Fork,
    RouterLimit,
    SearchBudget,
    NoStages,
    /// A long relation whose parts are separate routes, not one line.
    Apart,
}

/// A route that `route` did not keep, with the length, first and last point of its main line when
/// it built one.
#[derive(Clone, Copy)]
struct Failed {
    reason: Reason,
    line: Option<(f64, P, P)>,
}

/// What `route` builds: one route, or the sections of a long line whose plan failed.
enum Built {
    Route(Kept),
    Sections(Vec<Result<Kept, Failed>>),
}

/// The `id` of section `number` of a relation. It is negative, so it is no OSM relation ID.
fn section_id(relation: i64, number: usize) -> i64 {
    -(relation * 100 + number as i64)
}

fn tag<'a>(tags: &'a Tags, key: &str) -> &'a str {
    tags.get(key).unwrap_or("")
}

fn named(tags: &Tags) -> bool {
    !(tag(tags, "name").is_empty() && tag(tags, "ref").is_empty())
}

/// The `route` value of a relation that the catalog can build, named or not.
fn route_kind<'a>(tags: &'a Tags) -> Option<&'a str> {
    let kind = tag(tags, "route");
    (matches!(tag(tags, "type"), "route" | "superroute")
        && matches!(kind, "hiking" | "foot" | "bicycle" | "mtb")
        && !matches!(tag(tags, "network:type"), "node_network" | "basic_network")
        && !matches!(tag(tags, "state"), "proposed" | "planned" | "disused" | "abandoned"))
    .then_some(kind)
}

/// The `route` value of a relation that the catalog selects by its tags.
fn selected<'a>(tags: &'a Tags) -> Option<&'a str> {
    route_kind(tags).filter(|_| named(tags))
}

fn is_route(relations: &Table<Relations>, id: i64) -> bool {
    relations.get(id).is_some_and(|r| matches!(tag(&r.tags, "type"), "route" | "superroute"))
}

fn stage_role(role: &str) -> bool {
    matches!(role, "" | "main" | "part")
}

/// The child relations with a stage role, in member order.
fn children(relation: &Relation) -> Vec<i64> {
    relation
        .members()
        .filter(|(_, role)| stage_role(role))
        .filter_map(|(id, _)| if let Id::Relation(id) = id { Some(id) } else { None })
        .collect()
}

fn has_child_route(relation: &Relation, relations: &Table<Relations>) -> bool {
    relation.members().any(|(id, _)| matches!(id, Id::Relation(id) if is_route(relations, id)))
}

/// Whether a child relation is a piece: a route in the data with no child routes that is not a
/// route of its own.
fn piece(relations: &Table<Relations>, id: i64) -> bool {
    is_route(relations, id) && {
        let child = relations.get(id).expect("a route is in the data");
        !has_child_route(&child, relations) && (!named(&child.tags) || connector::connector(&child.tags, false))
    }
}

/// Whether a child relation is a stage that the catalog builds although it has no `name` and no
/// `ref`: a route that is no connector leg and no long route.
fn unnamed_stage(relations: &Table<Relations>, id: i64) -> bool {
    relations.get(id).is_some_and(|child| {
        let class = class(&child, relations);
        route_kind(&child.tags).is_some()
            && !named(&child.tags)
            && class != Class::Long
            && !is_connector(&child, class, relations)
    })
}

/// Whether a relation of `class` is a connector leg. A joined trail is one route, so its pieces are
/// no child routes.
fn is_connector(relation: &Relation, class: Class, relations: &Table<Relations>) -> bool {
    connector::connector(&relation.tags, class != Class::Joined && has_child_route(relation, relations))
}

/// How the catalog builds a selected relation.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Class {
    /// One route from its own member ways.
    Route,
    /// One route from its own member ways and those of its pieces.
    Joined,
    /// A route of stages. An absent child can be a route, so a relation with one is long.
    Long,
}

fn class(relation: &Relation, relations: &Table<Relations>) -> Class {
    let routes: Vec<i64> =
        children(relation).into_iter().filter(|&id| relations.get(id).is_none() || is_route(relations, id)).collect();
    if routes.is_empty() {
        Class::Route
    } else if routes.iter().all(|&id| piece(relations, id)) {
        Class::Joined
    } else {
        Class::Long
    }
}

/// The main member ways and their roles in member order. In a joined trail, each piece gives its
/// own in its place.
fn member_ways(relation: &Relation, relations: &Table<Relations>, joined: bool) -> Vec<(i64, String)> {
    relation
        .members()
        .flat_map(|(member, role)| match member {
            Id::Way(id) if assemble::is_main(role) => vec![(id, role.to_string())],
            Id::Relation(id) if joined && stage_role(role) && piece(relations, id) => {
                member_ways(&relations.get(id).expect("a piece is in the data"), relations, false)
            }
            _ => Vec::new(),
        })
        .collect()
}

/// The profiles that reproduce a kind. The first gives the plan route. A graded kind uses its
/// widest skill choice; a client selects it when `hardest` is above the ordinary limit.
fn profiles(kind: &str) -> &'static [&'static str] {
    match kind {
        "bicycle" => &["touring", "road", "gravel"],
        "mtb" => &["mtb/technical"],
        _ => &["hiking/alpine"],
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

/// The OSM tag fields of a record. A route in France has no `symbol`: the marks of the French
/// networks belong to their federation.
fn header(relation: &Relation, kind: &str, french: bool) -> Map<String, Value> {
    let tags = &relation.tags;
    let mut record = Map::new();
    record.insert("id".into(), json!(relation.id));
    record.insert("kind".into(), json!(kind));
    record.insert("rank".into(), json!(rank(tag(tags, "network"))));
    let website = ["website", "contact:website", "url"].into_iter().map(|key| tag(tags, key)).find(|w| !w.is_empty());
    let symbol = if french { "" } else { tag(tags, "osmc:symbol") };
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

/// Gives the record of an unnamed stage the `name` and `ref` of the long relation that names it.
fn named_after(record: &mut Map<String, Value>, trail: &Relation) {
    for field in ["name", "ref"].into_iter().filter(|field| !tag(&trail.tags, field).is_empty()) {
        record.insert(field.into(), json!(tag(&trail.tags, field)));
    }
}

/// The zoom 9 cells that the bounding box of each segment touches, edges included.
fn cells(line: &[P], into: &mut BTreeSet<(u32, u32)>) {
    for w in line.windows(2).chain(std::iter::once(&line[..1]).filter(|_| line.len() == 1)) {
        into.extend(crate::grid::touched(w[0], w[w.len() - 1], crate::grid::ZOOM));
    }
}

fn cell_ids(cells: &BTreeSet<(u32, u32)>) -> Vec<String> {
    let mut ids: Vec<_> = cells.iter().map(|(x, y)| format!("9-{x}-{y}")).collect();
    ids.sort();
    ids
}

pub(crate) fn encode(line: &[P]) -> Vec<i64> {
    let mut previous = [0i64; 2];
    line.iter()
        .flat_map(|p| {
            let delta = [p[0] as i64 - previous[0], p[1] as i64 - previous[1]];
            previous = [p[0] as i64, p[1] as i64];
            delta
        })
        .collect()
}

/// The vertices of a `line_udeg`.
pub(crate) fn decode(line: &Value) -> Vec<P> {
    let deltas: Vec<i64> = line.as_array().into_iter().flatten().filter_map(Value::as_i64).collect();
    let mut previous = [0i64; 2];
    deltas
        .as_chunks::<2>()
        .0
        .iter()
        .map(|delta| {
            previous = [previous[0] + delta[0], previous[1] + delta[1]];
            [previous[0] as i32, previous[1] as i32]
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
    /// A vertex of the plan route is in France.
    french: bool,
}

/// One route relation with its main-line members.
struct Job<'a> {
    relation: Relation<'a>,
    /// The long relation whose `name` and `ref` the record of an unnamed stage takes.
    named_by: Option<Relation<'a>>,
    kind: String,
    member_ways: Vec<(i64, String)>,
    members: Vec<assemble::Member>,
    skipped: usize,
}

impl<'a> Job<'a> {
    fn new(relation: Relation<'a>, kind: &str, relations: &Table<Relations>, named_by: Option<Relation<'a>>) -> Self {
        let member_ways = member_ways(&relation, relations, class(&relation, relations) == Class::Joined);
        Job { relation, named_by, kind: kind.into(), member_ways, members: Vec::new(), skipped: 0 }
    }
}

/// The facts that the report needs, besides the records.
#[derive(Default)]
pub struct Report {
    pub tag_selected: usize,
    pub dropped: BTreeMap<String, Vec<i64>>,
    pub kept: BTreeMap<String, usize>,
    pub loops: usize,
    pub long_routes: usize,
    /// Long records that are not complete, with no kept part backwards or after a jump.
    pub partial_long_routes: usize,
    /// Long records with a kept part backwards or after a jump.
    pub broken_long_routes: usize,
    /// Long records with a part that the builder cut, and no kept part backwards or after a jump.
    pub cut_long_routes: usize,
    pub missing_parts: usize,
    pub backwards_parts: usize,
    pub jumps: usize,
    pub stages: usize,
    pub via: Vec<usize>,
    pub long_points: Vec<(i64, usize)>,
    /// The trails that are trails by their moving time only.
    pub trails_by_hours: usize,
    /// The routes that skip main member ways that the import does not hold, with their count. A
    /// route can still leave the catalog later.
    pub skipped_ways: BTreeMap<i64, usize>,
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
    /// Counts a long record and the joined plan points of its kept parts.
    fn add_long(&mut self, record: &Map<String, Value>, points: usize) {
        let parts = record["parts"].as_array().expect("a long record has parts");
        let count = |flag: &str| parts.iter().filter(|part| part.get(flag).is_some()).count();
        let (missing, backwards, jumps) = (count("missing"), count("backwards"), count("jump"));
        self.missing_parts += missing;
        self.backwards_parts += backwards;
        self.jumps += jumps;
        let kept = parts.iter().filter(|part| part.get("missing").is_none());
        if kept.clone().any(|part| part.get("backwards").is_some() || part.get("jump").is_some()) {
            self.broken_long_routes += 1;
        } else if parts.iter().any(|part| part.get("cut").is_some()) {
            self.cut_long_routes += 1;
        } else if !record.contains_key("stages") {
            self.partial_long_routes += 1;
        }
        *self.kept.entry(record["kind"].as_str().unwrap().to_string()).or_default() += 1;
        self.long_routes += 1;
        self.loops += usize::from(record["loop"] == true);
        self.long_points.push((record["id"].as_i64().unwrap(), points));
    }

    pub fn to_json(&self) -> Value {
        let mut slowest = self.searches.clone();
        slowest.sort_by(|a, b| b.1.total_cmp(&a.1));
        let busy: f64 = self.searches.iter().map(|s| s.1).sum();
        json!({
            "tag_selected": self.tag_selected, "dropped": self.dropped, "kept": self.kept, "loops": self.loops,
            "long_routes": self.long_routes, "partial_long_routes": self.partial_long_routes,
            "broken_long_routes": self.broken_long_routes, "cut_long_routes": self.cut_long_routes,
            "missing_parts": self.missing_parts,
            "backwards_parts": self.backwards_parts, "jumps": self.jumps,
            "stages": self.stages, "skipped_ways": self.skipped_ways,
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
            "trails_by_hours": self.trails_by_hours,
            "seconds": self.seconds,
        })
    }
}

/// Checks the catalog options before the import.
pub fn check(profiles: &[Profile]) -> Result<(), String> {
    match PROFILES.iter().find(|&&name| !profiles.iter().any(|p| p.name == name)) {
        Some(profile) => Err(format!("The route catalog needs the {profile} profile")),
        None => Ok(()),
    }
}

/// Writes `route-catalog.json` into the routing package in `directory`, from the source OSM
/// objects of its import. Returns each trail, as `trails::trails` gives it, and the report.
pub fn write(directory: &Path, osm: Data) -> Result<(Vec<(Value, Value)>, Report), String> {
    let started = std::time::Instant::now();
    let package = Directory::open(directory).map_err(|e| e.to_string())?;
    // The routers share one graph, one junction mapping and the five prepared profiles; each
    // adds its own label blocks and a search space of `SEARCH_BYTES`.
    let routing = Selection::whole(package);
    let shared = routing.shared_bytes(PROFILES).map_err(|e| e.to_string())?;
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    let workers = cpus.min(ROUTERS_MEMORY.saturating_sub(shared) / (routing.router_bytes() + SEARCH_BYTES)).max(1);
    let (records, mut report) = catalog(&routing, osm, workers, SECTION_M)?;
    let trails = trails::trails(&records);
    report.trails_by_hours =
        trails.iter().filter(|(_, record)| record["stage_count"].as_u64().is_none_or(|count| count < 2)).count();
    let bytes =
        serde_json::to_vec(&crate::sort_keys(json!({"format": 1, "routes": records}))).map_err(|e| e.to_string())?;
    std::fs::write(directory.join(FILE), bytes).map_err(|e| e.to_string())?;
    Ok((trails, Report { seconds: started.elapsed().as_secs_f64(), ..report }))
}

fn catalog<S: Source + Clone + Send>(
    selection: &Selection<S>,
    osm: Data,
    workers: usize,
    section_m: f64,
) -> Result<(Vec<Value>, Report), String> {
    let Data { ways, relations, .. } = osm;
    let mut report = Report::default();
    let mut drops = BTreeMap::<i64, Reason>::new();
    let mut long = Vec::new();
    let mut jobs = Vec::new();
    // Each unnamed stage that is no connector leg, with the long relations that list it in ascending
    // ID order, as the relations come.
    let mut unnamed = BTreeMap::<i64, Vec<i64>>::new();
    for relation in relations.iter() {
        let Some(kind) = selected(&relation.tags).map(str::to_string) else { continue };
        report.tag_selected += 1;
        let class = class(&relation, &relations);
        if is_connector(&relation, class, &relations) {
            drops.insert(relation.id, Reason::Connector);
        } else if class != Class::Long {
            jobs.push(Job::new(relation, &kind, &relations, None));
        } else {
            for id in children(&relation).into_iter().filter(|&id| unnamed_stage(&relations, id)) {
                unnamed.entry(id).or_default().push(relation.id);
            }
            long.push((relation, kind));
        }
    }
    for (&id, listers) in &unnamed {
        let child = relations.get(id).expect("a stage is in the data");
        let kind = route_kind(&child.tags).expect("a stage has a kind").to_string();
        jobs.push(Job::new(child, &kind, &relations, relations.get(listers[0])));
    }
    let needed: HashSet<i64> = jobs.iter().flat_map(|job| &job.member_ways).map(|(id, _)| *id).collect();
    let points: HashMap<i64, P> = needed
        .into_iter()
        .filter_map(|id| ways.get(id))
        .flat_map(|way| way.nodes.iter().copied().zip(way.points.iter().copied()).collect::<Vec<_>>())
        .filter(|(_, point)| point.present())
        .map(|(id, point)| (id, [point.lon, point.lat]))
        .collect();
    jobs.retain_mut(|job| {
        // The import does not hold a way outside the region either, so only a way between two held
        // ways is skipped.
        let mut missing = 0;
        let held = job.member_ways.iter().all(|(id, role)| match ways.get(*id) {
            Some(way) if way.nodes.iter().all(|n| points.contains_key(n)) => {
                let between = !job.members.is_empty() || missing == 0;
                (job.skipped, missing) = (job.skipped + missing, 0);
                job.members.push(assemble::Member {
                    nodes: way.nodes.to_vec(),
                    role: role.clone(),
                    roundabout: matches!(tag(&way.tags, "junction"), "roundabout" | "circular"),
                });
                between
            }
            None => {
                missing += 1;
                true
            }
            Some(_) => false,
        });
        if !held || missing > 0 || job.skipped > MAX_SKIPPED_WAYS {
            drops.insert(job.relation.id, Reason::ExtractEdge);
            return false;
        }
        if job.skipped > 0 {
            report.skipped_ways.insert(job.relation.id, job.skipped);
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
                    let result = route(&mut router, &job, points, section_m, &mut calls);
                    let search = (job.relation.id, begun.elapsed().as_secs_f64(), calls);
                    results.lock().unwrap().push((search, result));
                }
            });
        }
    });
    let mut kept = BTreeMap::new();
    let mut lines = HashMap::new();
    report.workers = workers.max(1);
    report.search_seconds = started.elapsed().as_secs_f64();
    // The number of sections of each cut relation.
    let mut cut = BTreeMap::new();
    for (search, result) in results.into_inner().unwrap() {
        let id = search.0;
        report.searches.push(search);
        let routes = match result {
            Ok(Built::Route(route)) => vec![(id, Ok(route))],
            Ok(Built::Sections(sections)) => {
                cut.insert(id, sections.len());
                sections.into_iter().enumerate().map(|(i, section)| (section_id(id, i + 1), section)).collect()
            }
            Err(failed) => vec![(id, Err(failed))],
        };
        for (id, route) in routes {
            match route {
                Ok(route) => {
                    kept.insert(id, route);
                }
                Err(Failed { reason, line }) => {
                    drops.insert(id, reason);
                    lines.extend(line.map(|line| (id, line)));
                }
            }
        }
    }
    // The parts of a relation in a long record: the relation itself, or its sections.
    let parts_of = |id: i64, stage: Option<usize>| -> Vec<long::Part> {
        let name = relations.get(id).map_or(String::new(), |r| tag(&r.tags, "name").to_string());
        let ids: Vec<i64> = cut.get(&id).map_or(vec![id], |&n| (1..=n).map(|k| section_id(id, k)).collect());
        let last = ids.len() - 1;
        let part = |(i, id)| long::Part {
            id,
            name: name.clone(),
            stage,
            cut: i < last,
            kept: kept.get(&id),
            line: lines.get(&id).copied(),
        };
        ids.into_iter().enumerate().map(part).collect()
    };
    let mut cut_records = BTreeMap::new();
    for &id in cut.keys() {
        let relation = relations.get(id).expect("a cut route is in the data");
        let kind = route_kind(&relation.tags).expect("a cut route has a kind");
        let (mut record, points) =
            long::record(&relation, kind, &parts_of(id, None), false).expect("a cut route keeps a section");
        if let Some(trail) = unnamed.get(&id).and_then(|listers| relations.get(listers[0])) {
            named_after(&mut record, &trail);
        }
        report.add_long(&record, points);
        cut_records.insert(id, record);
    }
    // Each kept part gets the innermost long record that lists it as its trail, and a stage of a
    // complete long record the innermost complete one as its parent; of two at the same level, the
    // lower relation ID.
    let mut trails = BTreeMap::<i64, (usize, i64, usize)>::new();
    let mut parents = BTreeMap::<i64, (usize, i64, usize)>::new();
    let innermost = |map: &mut BTreeMap<i64, (usize, i64, usize)>, id: i64, trail| {
        map.entry(id).and_modify(|t| *t = (*t).min(trail)).or_insert(trail);
    };
    let mut records = Vec::new();
    let mut apart = HashSet::new();
    for (relation, kind) in long {
        let (leaves, connector) = long::leaves(&relation, &relations);
        let parts: Vec<long::Part> =
            leaves.iter().enumerate().flat_map(|(i, leaf)| parts_of(leaf.id, Some(i + 1))).collect();
        if long::apart(&parts) {
            drops.insert(relation.id, Reason::Apart);
            apart.insert(relation.id);
            continue;
        }
        let Some((record, points)) = long::record(&relation, &kind, &parts, connector) else {
            drops.insert(relation.id, Reason::NoStages);
            continue;
        };
        let complete = record.contains_key("stages");
        let built = |id| kept.contains_key(id) || cut_records.contains_key(id);
        for (number, leaf) in leaves.iter().enumerate().filter(|(_, leaf)| built(&leaf.id)) {
            let trail = (leaf.depth, relation.id, number + 1);
            innermost(&mut trails, leaf.id, trail);
            if complete {
                innermost(&mut parents, leaf.id, trail);
            }
        }
        report.add_long(&record, points);
        records.push(Value::Object(record));
    }
    let in_trail = |record: &mut Map<String, Value>, id| {
        if let Some((_, trail, number)) = trails.get(&id) {
            record.insert("trail".into(), json!(trail));
            record.insert("trail_stage".into(), json!(number));
        }
    };
    // An unnamed stage takes the name of the lowest long relation that lists it and is no set of
    // separate routes, or else of its trail. With neither, it leaves, as any unnamed route.
    let named = |record: &mut Map<String, Value>, id: i64| -> bool {
        // A section holds the ID of its relation (`section_id`).
        let relation = if id < 0 { -id / 100 } else { id };
        let Some(listers) = unnamed.get(&relation) else { return true };
        let namer = listers.iter().copied().find(|id| !apart.contains(id)).or(trails.get(&relation).map(|t| t.1));
        let Some(namer) = namer else { return false };
        if namer != listers[0] {
            record.remove("name");
            record.remove("ref");
            named_after(record, &relations.get(namer).expect("a long relation is in the data"));
        }
        true
    };
    for (id, mut record) in cut_records {
        if !named(&mut record, id) {
            continue;
        }
        in_trail(&mut record, id);
        records.push(Value::Object(record));
    }
    for (id, mut route) in kept {
        if !named(&mut route.record, id) {
            continue;
        }
        in_trail(&mut route.record, id);
        if let Some((_, parent, number)) = parents.get(&id) {
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
    section_m: f64,
    calls: &mut usize,
) -> Result<Built, Failed> {
    let apart = |a: i64, b: i64| distance(points[&a], points[&b]);
    let located =
        |runs: &[Vec<i64>]| -> Vec<Vec<P>> { runs.iter().map(|run| run.iter().map(|n| points[n]).collect()).collect() };
    let runs = assemble::main_line(&job.members, &apart);
    // A shuffled member order with small jumps would retrace itself; the nearest-end chain does not.
    let chained = if runs.len() > 1 { assemble::chain(&runs, &apart, MAX_GAP_M) } else { None };
    let in_order = chained.is_some() || runs.windows(2).all(|w| apart(w[0][w[0].len() - 1], w[1][0]) <= MAX_GAP_M);
    let mut runs = chained.unwrap_or(runs);
    let line = ends(&located(&runs));
    let failed = |reason| Failed { reason, line };
    if !in_order {
        let ends = [runs[0][0], runs[runs.len() - 1][runs[runs.len() - 1].len() - 1]];
        let (parts, branches) = assemble::prune_forks(&runs, &apart, SIDE_TRIP_SHARE).ok_or(failed(Reason::Fork))?;
        runs = assemble::chain(&parts, &apart, MAX_GAP_M).ok_or(failed(Reason::Gap))?;
        assemble::side_trips(&mut runs, &branches, ends, &apart);
    }
    shaped(router, job, &located(&runs), section_m, calls).map_err(failed)
}

/// The length, first and last point of a line in runs.
fn ends(runs: &[Vec<P>]) -> Option<(f64, P, P)> {
    let (first, last) = (*runs.first()?.first()?, *runs.last()?.last()?);
    Some((runs.iter().map(|run| shape::length(run)).sum(), first, last))
}

/// Builds the record of one route from the runs of its main line in order, or the sections of a
/// line longer than `section_m` whose plan fails, when one of them passes.
fn shaped<D: RoutingData>(
    router: &mut Router<D>,
    job: &Job,
    runs: &[Vec<P>],
    section_m: f64,
    calls: &mut usize,
) -> Result<Built, Reason> {
    let own = runs.concat();
    let own = shape::Near::new(&own);
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
                    planner_router::Error::Limit | planner_router::Error::Memory => Reason::RouterLimit,
                    _ => Reason::UnroutableGap,
                },
            )?;
        let patch: Vec<P> = [from].into_iter().chain(shape::vertices(&route)).chain([to]).collect();
        // The shaping check compares with the line that holds the patch, so it cannot see a detour
        // in it. A patch may leave the route's own line, its runs joined straight across their
        // gaps, for at most the longest hole that the builder joins.
        if shape::far(&patch, &own) > MAX_GAP_M {
            return Err(Reason::Gap);
        }
        line.extend(&patch[1..]);
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
    let reason = match planned(router, job, &line, length, closed, calls) {
        Err(reason) if length > section_m => reason,
        whole => return whole.map(Built::Route),
    };
    let sections = sections(&line, section_m);
    if sections.len() > MAX_SECTIONS {
        return Err(reason);
    }
    let built = sections.iter().enumerate().map(|(i, line)| {
        let length = shape::length(line);
        let failed = |reason| Failed { reason, line: ends(std::slice::from_ref(line)) };
        if length < MIN_LENGTH_M {
            return Err(failed(Reason::Short));
        }
        let mut kept = planned(router, job, line, length, false, calls).map_err(failed)?;
        kept.record.insert("id".into(), json!(section_id(job.relation.id, i + 1)));
        kept.record.insert("section".into(), json!(i + 1));
        kept.record.insert("trail".into(), json!(job.relation.id));
        Ok(kept)
    });
    let built: Vec<_> = built.collect();
    // With no section kept, the route leaves as the whole line did.
    if built.iter().all(Result::is_err) {
        return Err(reason);
    }
    Ok(Built::Sections(built))
}

/// A line cut into ⌈length / `section_m`⌉ sections of equal length, each cut at the line point
/// nearest to it. Each section ends at the point where the next one starts.
fn sections(line: &[P], section_m: f64) -> Vec<Vec<P>> {
    let mut along = vec![0.0];
    for w in line.windows(2) {
        along.push(along[along.len() - 1] + distance(w[0], w[1]));
    }
    let total = along[along.len() - 1];
    let n = (total / section_m).ceil() as usize;
    let nearest = |at: f64| {
        let j = along.partition_point(|&a| a < at).min(line.len() - 1);
        if j > 0 && at - along[j - 1] < along[j] - at {
            j - 1
        } else {
            j
        }
    };
    let cuts: Vec<usize> = (0..=n).map(|k| nearest(total * k as f64 / n as f64)).collect();
    cuts.windows(2).map(|w| line[w[0]..=w[1]].to_vec()).collect()
}

/// Plans a line and builds its record.
fn planned<D: RoutingData>(
    router: &mut Router<D>,
    job: &Job,
    line: &[P],
    length: f64,
    closed: bool,
    calls: &mut usize,
) -> Result<Kept, Reason> {
    let profiles = profiles(&job.kind);
    // An out-and-back spur retraces its nodes; its tip is a turnaround.
    let tips: Vec<usize> = (1..line.len().saturating_sub(1)).filter(|&k| line[k - 1] == line[k + 1]).collect();
    let mut limits = Limits { control: control(), max_calls: SEARCH_CALLS * profiles.len(), calls: 0 };
    let plan = shape::shape(router, profiles, line, &tips, length, closed, &mut limits);
    *calls += limits.calls;
    let plan = plan.map_err(|failure| match failure {
        Failure::TooManyPoints => Reason::TooManyPoints,
        Failure::Check => Reason::ShapingCheck,
        // The catalog control never cancels.
        Failure::Limit | Failure::Memory | Failure::Cancelled => Reason::RouterLimit,
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
    // Every vertex of the plan route: a simple line can cut across a small piece of France.
    let french = crate::country::in_france(&geometry);
    let mut record = header(&job.relation, &job.kind, french);
    if let Some(trail) = &job.named_by {
        named_after(&mut record, trail);
    }
    let totals = plan.route.totals();
    record.insert("loop".into(), json!(closed));
    record.insert("length_m".into(), json!(totals.distance_m));
    record.insert("ascent_m".into(), json!(totals.ascent_m));
    record.insert("descent_m".into(), json!(totals.descent_m));
    record.insert("moving_s".into(), json!(totals.seconds.round() as u64));
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
    if let Some(profile) = profile(&plan.route.points, totals.distance_m as f64) {
        record.insert("profile_m".into(), json!(profile));
    }
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
        french,
    })
}

/// The height at each `PROFILE_STEP_M` along a route, and at its end, with the walked distance
/// scaled to `length_m`. `None` when no point has an elevation.
fn profile(points: &[Point], length_m: f64) -> Option<Vec<Option<i32>>> {
    if points.len() < 2 || points.iter().all(|p| p.elevation == NO_ELEVATION) {
        return None;
    }
    let along = geometry::cumulative(points);
    let walked = along[along.len() - 1];
    let steps = (length_m / PROFILE_STEP_M).floor() as usize;
    let at = |distance: f64| {
        let u = if length_m > 0.0 { (distance / length_m * walked).min(walked) } else { 0.0 };
        // An entry on a point takes its height; between points, the straight line between them.
        let height = geometry::at(points, &along, u).elevation;
        (height != NO_ELEVATION).then(|| height.round() as i32)
    };
    Some((0..=steps).map(|i| i as f64 * PROFILE_STEP_M).chain([length_m]).map(at).collect())
}

/// The length of the plan route with each grade, and the hardest explicit grade, from the grades
/// of the route pieces.
fn grade_lengths(route: &planner_router::Route, geometry: &[P], mtb: bool) -> ([u64; 6], Option<usize>) {
    let mut lengths = [0.0f64; 6];
    let mut hardest = None;
    let mut start = 0;
    for piece in route.pieces() {
        let grade = if mtb { piece.mtb_scale } else { piece.sac_scale };
        let explicit = grade.filter(|&d| d <= 6).map(|d| if mtb { d.min(5) } else { d.max(1) - 1 } as usize);
        for w in geometry[start..=piece.end].windows(2) {
            let length = distance(w[0], w[1]);
            lengths[explicit.unwrap_or(0)] += length;
            if length > 0.0 {
                hardest = hardest.max(explicit);
            }
        }
        start = piece.end;
    }
    // Scaled to the route length, so that the six rounded lengths add up to it within 3 m.
    let sum: f64 = lengths.iter().sum();
    let scale = if sum > 0.0 { route.totals().distance_m as f64 / sum } else { 0.0 };
    (lengths.map(|l| (l * scale).round() as u64), hardest)
}

#[cfg(test)]
mod tests;
