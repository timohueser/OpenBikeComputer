//! The road solver by area: a job solves the ways within a halo of a tile of AREA_ZOOM, and each
//! vertex takes its height from the job of its area.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rstar::primitives::GeomWithData;
use rstar::RTree;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

use super::chains::{self, area_facts, assemble, AreaFacts};
use super::fit::Graph;
use super::heights::Heights;
use super::network::Network;
use super::network::{distinct, hypot, sub, unique};
use super::rows::{root, union};
use super::{align, solve, Classes, Feature, Ground, GroundMetrics, Metrics, Near, Solved, Surroundings, Way};
use crate::geometry::{bounds, Boxes};
use crate::{metres, Terrain};

/// An area is a tile of AREA_ZOOM, about 27 km at 47° N. Its job holds the ways within HALO_M of it.
pub const AREA_ZOOM: u32 = 10;
pub const HALO_M: f64 = 500.0;
/// A job holds at most about this many bytes per metre of the ways of its area: the network of its
/// nodes, the rows and the factorization of its structures and its alignment, measured on box B.
const JOB_BYTES_PER_M: f64 = 400.0;
/// The ways that a deck or a bore crosses join a job from the points every FREE_STEP_M along it.
const FREE_STEP_M: f64 = 50.0;
/// A job takes the waterways within NEAR_M of its ways: the beds beside its sidewalks.
const NEAR_M: f64 = 20.0;
/// A bridge or a tunnel has almost no length in the graph of the areas: a job holds it whole.
const BARE_M: f64 = 1e-6;

pub(crate) type Area = (i64, i64);

/// The OSM vertices of the ways of the features as one graph: `point` and `area` per vertex;
/// `vertex` and `feature` per occurrence; the OSM segments as edges of their metres, a bridge or a
/// tunnel of almost none.
struct Region {
    feature: Vec<usize>,
    vertex: Vec<usize>,
    point: Vec<[f64; 2]>,
    graph: Graph,
    /// Per area that holds a vertex, the metres of the segments that start in it: what its job weighs.
    areas: HashMap<Area, f64>,
    /// Points every FREE_STEP_M along each bridge and tunnel, and a vertex of its segment.
    free_point: Vec<[f64; 2]>,
    free_vertex: Vec<usize>,
    /// The vertices by cell of a side `cell`, which is HALO_M or more anywhere in the region.
    cells: HashMap<(i64, i64), Vec<u32>>,
    cell: f64,
}

pub(crate) fn area_of(point: [f64; 2]) -> Area {
    let n = (1u64 << AREA_ZOOM) as f64;
    ((point[0] * n).floor() as i64, (point[1] * n).floor() as i64)
}

impl Region {
    fn new(features: &[&Feature]) -> Region {
        let (mut feature, mut xy, mut free, mut way) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for (f, item) in features.iter().enumerate() {
            for points in &item.parts {
                let points = distinct(points);
                if points.len() >= 2 {
                    feature.extend(std::iter::repeat_n(f, points.len()));
                    way.extend(std::iter::repeat_n(free.len(), points.len()));
                    xy.extend(points);
                    free.push(item.free);
                }
            }
        }
        let (vertex, count) = unique(&xy);
        let mut point = vec![[0.0; 2]; count];
        for (o, &v) in vertex.iter().enumerate() {
            point[v] = xy[o];
        }
        let mut edges = Vec::new();
        let mut areas: HashMap<Area, f64> = point.iter().map(|&p| (area_of(p), 0.0)).collect();
        for o in (0..xy.len().saturating_sub(1)).filter(|&o| way[o] == way[o + 1]) {
            let d = sub(xy[o + 1], xy[o]);
            let span = hypot(d) * metres(xy[o][1] + d[1] / 2.0);
            *areas.get_mut(&area_of(point[vertex[o]])).unwrap() += span;
            let length = if free[way[o]] { BARE_M } else { span.max(0.01) };
            let (lo, hi) = (vertex[o].min(vertex[o + 1]), vertex[o].max(vertex[o + 1]));
            edges.push((lo, hi, length));
        }
        // Of two ways along one segment, the shorter edge holds.
        edges.sort_by(|p, q| p.partial_cmp(q).unwrap());
        edges.dedup_by(|later, first| (later.0, later.1) == (first.0, first.1));
        let (mut free_point, mut free_vertex) = (Vec::new(), Vec::new());
        for &(lo, hi, _) in edges.iter().filter(|e| e.2 == BARE_M) {
            let (a, b) = (point[lo], point[hi]);
            let steps = (hypot(sub(b, a)) * metres(a[1]) / FREE_STEP_M).ceil().max(1.0) as usize;
            for j in 0..=steps {
                let t = j as f64 / steps as f64;
                free_point.push([a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]);
                free_vertex.push(lo);
            }
        }
        let cell = HALO_M / point.iter().map(|p| metres(p[1])).fold(f64::INFINITY, f64::min);
        let mut cells: HashMap<(i64, i64), Vec<u32>> = HashMap::new();
        for (v, p) in point.iter().enumerate() {
            cells.entry(((p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64)).or_default().push(v as u32);
        }
        let graph = Graph::symmetric(count, &edges);
        Region { feature, vertex, point, graph, areas, free_point, free_vertex, cells, cell }
    }

    /// The features of the ways within HALO_M of an area: from the nearest point of the area in a
    /// straight line, then along the ways.
    fn near(&self, (x, y): Area) -> Vec<usize> {
        let n = (1u64 << AREA_ZOOM) as f64;
        let (x0, x1, y0, y1) = (x as f64 / n, (x + 1) as f64 / n, y as f64 / n, (y + 1) as f64 / n);
        let mut entry: Vec<f64> = self
            .point
            .iter()
            .map(|&[px, py]| {
                let dx = (x0 - px).max(px - x1).max(0.0);
                let dy = (y0 - py).max(py - y1).max(0.0);
                dx.hypot(dy) * metres(py)
            })
            .collect();
        let scale = metres((y as f64 + 0.5) / n);
        let mut ties = 0;
        loop {
            let starts = (0..entry.len()).filter(|&v| entry[v] <= HALO_M).map(|v| (v, entry[v].max(1e-9)));
            let distance = self.graph.distances(starts, HALO_M);
            // A deck or a bore ties the ways that it crosses without a vertex, however far along it:
            // they enter at the distance of the bridge or the tunnel and their gap to it.
            let reached: Vec<usize> =
                (0..self.free_point.len()).filter(|&i| distance[self.free_vertex[i]].is_finite()).collect();
            if reached.len() == ties {
                let near: BTreeSet<usize> = (0..self.vertex.len())
                    .filter(|&o| distance[self.vertex[o]].is_finite())
                    .map(|o| self.feature[o])
                    .collect();
                return near.into_iter().collect();
            }
            ties = reached.len();
            let scaled = |p: [f64; 2]| [p[0] * scale, p[1] * scale];
            let free: RTree<GeomWithData<[f64; 2], usize>> =
                RTree::bulk_load(reached.iter().map(|&i| GeomWithData::new(scaled(self.free_point[i]), i)).collect());
            // Only a vertex within HALO_M of a free point can get nearer.
            let reach = HALO_M / scale;
            let near: BTreeSet<(i64, i64)> = reached
                .iter()
                .flat_map(|&i| {
                    let [px, py] = self.free_point[i];
                    let cell = |x: f64| (x / self.cell).floor() as i64;
                    (cell(px - reach)..=cell(px + reach))
                        .flat_map(move |cx| (cell(py - reach)..=cell(py + reach)).map(move |cy| (cx, cy)))
                })
                .collect();
            for v in near.iter().filter_map(|c| self.cells.get(c)).flatten().map(|&v| v as usize) {
                let p = scaled(self.point[v]);
                let nearest = free.nearest_neighbor(&p).unwrap();
                let gap = hypot(sub(*nearest.geom(), p));
                if gap < HALO_M {
                    entry[v] = entry[v].min(gap + distance[self.free_vertex[nearest.data]]);
                }
            }
        }
    }
}

/// The features of `beyond` that the solve of `features` needs: each bridge and tunnel of a network of
/// them that holds a bridge or a tunnel of `features`, and each way on the ground with a vertex at an
/// end of such a network. Then the features of a network with an end that meets no way where
/// `inside(xy)` is false: the input cuts that network, so it has no ends to run between.
pub fn context(
    features: &[Feature],
    beyond: &[Feature],
    inside: &dyn Fn([f64; 2]) -> bool,
) -> (Vec<usize>, BTreeSet<usize>) {
    let every: Vec<&Feature> = features.iter().chain(beyond).collect();
    let count = features.len();
    let mut owner = Vec::new();
    let mut xy = Vec::new();
    let mut tips = Vec::new();
    for (f, feature) in every.iter().enumerate() {
        for points in &feature.parts {
            let points = distinct(points);
            if points.len() >= 2 {
                tips.extend([xy.len(), xy.len() + points.len() - 1]);
                owner.extend(std::iter::repeat_n(f, points.len()));
                xy.extend(points);
            }
        }
    }
    if xy.is_empty() {
        return (Vec::new(), BTreeSet::new());
    }
    let (vertex, keys) = unique(&xy);
    let free = |f: usize| every[f].free;
    let mut parent: Vec<usize> = (0..every.len() + keys).collect();
    for o in (0..xy.len()).filter(|&o| free(owner[o])) {
        union(&mut parent, owner[o], every.len() + vertex[o]);
    }
    let network: Vec<usize> = (0..every.len()).map(|f| root(&mut parent, f)).collect();
    let held: BTreeSet<usize> = (0..count).filter(|&f| free(f)).map(|f| network[f]).collect();
    let joined: Vec<bool> = (0..every.len()).map(|f| free(f) && held.contains(&network[f])).collect();
    let mut occurrences = vec![0usize; keys];
    for &v in &vertex {
        occurrences[v] += 1;
    }
    let tips: Vec<usize> = tips.into_iter().filter(|&o| joined[owner[o]]).collect();
    let open: BTreeSet<usize> =
        tips.iter().filter(|&&o| occurrences[vertex[o]] < 2 && !inside(xy[o])).map(|&o| network[owner[o]]).collect();
    let whole: Vec<bool> = (0..every.len()).map(|f| joined[f] && !open.contains(&network[f])).collect();
    let ends: BTreeSet<usize> = tips.iter().filter(|&&o| whole[owner[o]]).map(|&o| vertex[o]).collect();
    let mut landed = vec![false; every.len()];
    for o in (0..xy.len()).filter(|&o| !free(owner[o]) && ends.contains(&vertex[o])) {
        landed[owner[o]] = true;
    }
    let cut = (0..count).filter(|&f| joined[f] && !whole[f]).collect();
    ((count..every.len()).filter(|&f| whole[f] || landed[f]).map(|f| f - count).collect(), cut)
}

/// The heights of the features by the three phases of a bake. Phase 0 turns the culverts into ways on
/// the ground. Phase A finds per area the facts of the points that it owns, for the areas of `features`
/// and of `context`, since the owner of a fact's anchor emits it; phase B joins them into chains and
/// solves each chain once, in the order of its reads; phase C takes the heights per area of `features`
/// and aligns the ways, and each OSM vertex of the area takes its alignment from there, with the vertices
/// that follow it to the next one.
pub fn solve_by_area(
    features: &[&Feature],
    context: &[&Feature],
    terrain: &dyn Terrain,
    around: &Surroundings,
    classes: &Classes,
    (memory, threads): (usize, usize),
) -> Solved {
    let mut clock = std::time::Instant::now();
    let rivers = &around.rivers;
    let culvert_boxes = Boxes::new(&around.culverts.iter().map(|r| bounds(&r.points)).collect::<Vec<_>>());
    let count = features.len();
    let boxes = Boxes::new(&rivers.iter().map(|r| bounds(&r.points)).collect::<Vec<_>>());
    let near = |subset: &[&Feature]| {
        let points: Vec<[f64; 2]> = subset.iter().flat_map(|f| &f.parts).flat_map(|points| distinct(points)).collect();
        let [x0, y0, x1, y1] = bounds(&points);
        let pad = NEAR_M / metres(y1);
        let reach = [x0 - pad, y0 - pad, x1 + pad, y1 + pad];
        let rivers = boxes.overlapping(reach).into_iter().map(|i| &around.rivers[i]).collect();
        let culverts = culvert_boxes.overlapping(reach).into_iter().map(|i| &around.culverts[i]).collect();
        Near { rivers, culverts, buildings: Some(&around.buildings) }
    };
    let every: Vec<&Feature> = features.iter().chain(context).copied().collect();
    let (grounded, culverts) = grounded(&every, terrain);
    let every = with_culverts(&every, &grounded, &culverts);
    let (features, context) = every.split_at(count);
    let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
        let Some(net) = Network::new(subset, classes) else { return AreaFacts::default() };
        let near = near(subset);
        area_facts(&net, classes, terrain, (&near.rivers, &near.culverts), chosen, &|xy| area_of(xy) == area)
    };
    let facts = AreaFacts::join(each_area(features, context, true, (memory, threads), &job));
    let mut phases = [(clock.elapsed().as_secs_f64(), peak()); 3];
    eprintln!("roads: phase A {:.1} s", phases[0].0);
    clock = std::time::Instant::now();
    let chains = assemble(&every, &facts.facts);
    let heights = Heights::solve(&every, classes, terrain, &chains, &facts, threads);
    phases[1] = (clock.elapsed().as_secs_f64(), peak());
    eprintln!("roads: phase B {:.1} s ({:.1} s solving)", phases[1].0, heights.metrics.seconds);
    clock = std::time::Instant::now();
    let job = |subset: &[&Feature], chosen: &[usize], area: Area| {
        let owned = |xy: [f64; 2]| area_of(xy) == area;
        let ground = Ground { heights: &heights, feature_of: chosen };
        solve_area(subset, chosen, &solve(subset, terrain, &near(subset), classes, Some(&owned), &ground), &owned)
    };
    let mut solved = Solved {
        ways: features.iter().map(|f| vec![None; f.parts.len()]).collect(),
        metrics: Metrics::default(),
        ground: GroundMetrics { conflicts: chains.conflicts, ..heights.metrics.clone() },
        culverts: (0..count).filter(|&f| culverts[f]).collect(),
        ..Default::default()
    };
    let mut pieces: BTreeMap<(usize, usize), Vec<Run>> = BTreeMap::new();
    for job in each_area(features, context, false, (memory, threads), &job) {
        for (way, runs) in job.found {
            pieces.entry(way).or_default().extend(runs);
        }
        solved.joins.extend(job.joins.into_iter().filter(|join| join.iter().all(|end| end.0 < count)));
        solved.underpasses.extend(job.underpasses.into_iter().filter(|&f| f < count));
        solved.metrics.add(&job.metrics);
    }
    solved.metrics.culverts = solved.culverts.len();
    for ((f, p), mut runs) in pieces.into_iter().filter(|((f, _), _)| *f < count) {
        runs.sort_by_key(|run| run.0);
        let mut way = Way::default();
        for (_, run) in &runs {
            (0..run.points.len()).for_each(|i| way.add(run, i));
        }
        solved.ways[f][p] = Some(way);
    }
    align::measure(&mut solved.ways, &solved.joins);
    phases[2] = (clock.elapsed().as_secs_f64(), peak());
    eprintln!("roads: phase C {:.1} s", phases[2].0);
    solved.phases = phases;
    solved
}

/// Phase 0: the culverts of the features, and each as a way on the ground.
fn grounded(features: &[&Feature], terrain: &dyn Terrain) -> (Vec<Feature>, Vec<bool>) {
    let culverts = chains::culverts(features, terrain);
    let grounded = (0..features.len())
        .filter(|&f| culverts[f])
        .map(|f| Feature { free: false, culvert: true, ..features[f].clone() })
        .collect();
    (grounded, culverts)
}

/// The features with each culvert replaced by its way on the ground.
fn with_culverts<'a>(features: &[&'a Feature], grounded: &'a [Feature], culverts: &[bool]) -> Vec<&'a Feature> {
    let mut replaced = grounded.iter();
    features.iter().zip(culverts).map(|(&f, &c)| if c { replaced.next().unwrap() } else { f }).collect()
}

/// All phases with one job that owns every point.
#[cfg(test)]
pub(crate) fn solve_whole(features: &[&Feature], terrain: &dyn Terrain, near: &Near, classes: &Classes) -> Solved {
    let (grounded, culverts) = grounded(features, terrain);
    let every = with_culverts(features, &grounded, &culverts);
    let numbers: Vec<usize> = (0..every.len()).collect();
    let Some(net) = Network::new(&every, classes) else {
        return Solved { ways: features.iter().map(|f| vec![None; f.parts.len()]).collect(), ..Default::default() };
    };
    let facts =
        AreaFacts::join(vec![area_facts(&net, classes, terrain, (&near.rivers, &near.culverts), &numbers, &|_| true)]);
    let chains = assemble(&every, &facts.facts);
    let heights = Heights::solve(&every, classes, terrain, &chains, &facts, 1);
    let mut solved = solve(&every, terrain, near, classes, None, &Ground { heights: &heights, feature_of: &numbers });
    solved.culverts = (0..features.len()).filter(|&f| culverts[f]).collect();
    solved.metrics.culverts = solved.culverts.len();
    solved.ground = GroundMetrics { conflicts: chains.conflicts, ..heights.metrics.clone() };
    solved
}

/// What `each_area` runs per area.
pub(crate) type AreaJob<'a, T> = dyn Fn(&[&Feature], &[usize], Area) -> T + Sync + 'a;

/// `job` per area of `features`, and with `of_context` also per area of `context` alone: it gets the
/// features within HALO_M of the area, their numbers in `features` ++ `context`, and the area. The jobs run
/// on `threads` threads, the largest first, as many at once as `memory` holds; the results are in the
/// order of the areas.
pub(crate) fn each_area<T: Send>(
    features: &[&Feature],
    context: &[&Feature],
    of_context: bool,
    (memory, threads): (usize, usize),
    job: &AreaJob<'_, T>,
) -> Vec<T> {
    let count = features.len();
    let every: Vec<&Feature> = features.iter().chain(context).copied().collect();
    let region = Region::new(&every);
    let areas = |of: &dyn Fn(usize) -> bool| -> BTreeSet<Area> {
        (0..region.vertex.len())
            .filter(|&o| of(region.feature[o]))
            .map(|o| area_of(region.point[region.vertex[o]]))
            .collect()
    };
    let mut wanted = areas(&|f| f < count);
    let own = wanted.len();
    if of_context {
        wanted.extend(areas(&|f| f >= count));
    }
    let mut jobs: Vec<Area> = wanted.into_iter().filter(|area| region.areas.contains_key(area)).collect();
    jobs.sort_by(|p, q| region.areas[q].partial_cmp(&region.areas[p]).unwrap().then(p.cmp(q)));
    let cores = threads.max(1);
    let memory = memory.saturating_sub(resident());
    let budget = Budget { state: Mutex::new((memory, 0)), freed: Condvar::new() };
    let more = jobs.len().saturating_sub(own);
    eprintln!(
        "roads: {} areas, {more} of the context alone, on {cores} threads in {:.1} GB",
        jobs.len(),
        memory as f64 / 1e9
    );
    let (next, done) = (AtomicUsize::new(0), Mutex::new(Vec::new()));
    std::thread::scope(|scope| {
        for _ in 0..cores.min(jobs.len()) {
            scope.spawn(|| loop {
                let number = next.fetch_add(1, Ordering::Relaxed);
                let Some(&area) = jobs.get(number) else { break };
                let held = budget.take((JOB_BYTES_PER_M * region.areas[&area]) as usize);
                let chosen = region.near(area);
                let subset: Vec<&Feature> = chosen.iter().map(|&f| every[f]).collect();
                let result = job(&subset, &chosen, area);
                release_freed();
                drop(held);
                done.lock().unwrap().push((number, result));
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_by_key(|(number, _)| *number);
    done.into_iter().map(|(_, result)| result).collect()
}

/// The vertices of a way from one OSM vertex of an area on: (the OSM vertex, the vertices).
type Run = (usize, Way);

struct Job {
    /// Per way (feature, part) that the job solves, the runs of its vertices in the area.
    found: Vec<((usize, usize), Vec<Run>)>,
    joins: Vec<[(usize, usize, bool); 2]>,
    underpasses: BTreeSet<usize>,
    metrics: Metrics,
}

/// The runs of the vertices of a job's solve in its area, and its joins, tunnels and metrics, by the
/// numbers `chosen` of its features.
fn solve_area(subset: &[&Feature], chosen: &[usize], solved: &Solved, owned: &dyn Fn([f64; 2]) -> bool) -> Job {
    let mut found = Vec::new();
    for ((&f, feature), parts) in chosen.iter().zip(subset).zip(&solved.ways) {
        for (p, part) in parts.iter().enumerate() {
            let Some(way) = part else { continue };
            let mine: Vec<bool> = distinct(&feature.parts[p]).iter().map(|&xy| owned(xy)).collect();
            let (mut runs, mut last): (Vec<Run>, Option<usize>) = (Vec::new(), None);
            for i in (0..way.points.len()).filter(|&i| mine[way.osm[i]]) {
                if last.is_none_or(|l| l + 1 != i) {
                    runs.push((way.osm[i], Way::default()));
                }
                runs.last_mut().unwrap().1.add(way, i);
                last = Some(i);
            }
            found.push(((f, p), runs));
        }
    }
    Job {
        found,
        joins: solved.joins.iter().map(|join| join.map(|(f, p, end)| (chosen[f], p, end))).collect(),
        underpasses: solved.underpasses.iter().map(|&i| chosen[i]).collect(),
        metrics: solved.metrics.clone(),
    }
}

/// The memory that the running jobs may still take, and how many run.
struct Budget {
    state: Mutex<(usize, usize)>,
    freed: Condvar,
}

/// Bytes of a [`Budget`] that a job holds until it ends, also when it panics.
struct Held<'a> {
    budget: &'a Budget,
    bytes: usize,
}

impl Budget {
    /// Wait until `bytes` fit, or no job runs; then hold them, or all that is left. With nothing left,
    /// one job runs at a time.
    fn take(&self, bytes: usize) -> Held<'_> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        while state.1 > 0 && state.0 < bytes {
            state = self.freed.wait(state).unwrap_or_else(PoisonError::into_inner);
        }
        let bytes = bytes.min(state.0);
        state.0 -= bytes;
        state.1 += 1;
        Held { budget: self, bytes }
    }
}

impl Drop for Held<'_> {
    fn drop(&mut self) {
        let mut state = self.budget.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.0 += self.bytes;
        state.1 -= 1;
        self.budget.freed.notify_all();
    }
}

/// Gives the memory that the jobs freed back to the system. glibc keeps what a thread frees in the arena
/// of that thread, where the jobs on other threads do not take it: without this the resident set grows by
/// the peak of a job per thread, and the budget, which reads the resident set, starves.
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn release_freed() {
    extern "C" {
        fn malloc_trim(pad: usize) -> std::ffi::c_int;
    }
    // SAFETY: malloc_trim only gives free pages of the allocator back to the system.
    unsafe { malloc_trim(0) };
}

#[cfg(not(all(target_os = "linux", target_env = "gnu")))]
fn release_freed() {}

/// The bytes of memory that this process holds: `memory` counts what was free before it started.
fn resident() -> usize {
    status("VmRSS:")
}

/// The most bytes of memory that this process has held so far.
fn peak() -> usize {
    status("VmHWM:")
}

/// A field in kB of the status of this process, in bytes; 0 where unknown.
fn status(field: &str) -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
    let line = status.lines().find(|l| l.starts_with(field));
    line.and_then(|l| l.split_whitespace().nth(1)?.parse::<usize>().ok()).unwrap_or(0) << 10
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use super::*;

    fn budget(memory: usize) -> Budget {
        Budget { state: Mutex::new((memory, 0)), freed: Condvar::new() }
    }

    #[test]
    fn with_no_memory_left_one_job_runs_at_a_time() {
        let budget = budget(0);
        let first = budget.take(100);
        let running = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _second = budget.take(100);
                running.store(true, Ordering::SeqCst);
            });
            std::thread::sleep(Duration::from_millis(50));
            assert!(!running.load(Ordering::SeqCst), "a second job started beside the first");
            drop(first);
        });
        assert!(running.load(Ordering::SeqCst));
        assert_eq!(*budget.state.lock().unwrap(), (0, 0));
    }

    #[test]
    fn jobs_that_fit_run_together_and_a_panic_gives_its_bytes_back() {
        let budget = budget(300);
        let (a, b) = (budget.take(100), budget.take(200));
        assert_eq!(*budget.state.lock().unwrap(), (0, 2));
        drop((a, b));
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    let _held = budget.take(300);
                    panic!("a job fails");
                });
            });
        }));
        assert!(panicked.is_err());
        // The bytes are back: a job of the whole budget starts at once.
        drop(budget.take(300));
        assert_eq!(*budget.state.lock().unwrap(), (300, 0));
    }
}
