//! Shaping points: few points on a relation line that make the router follow that line.
//!
//! The deviation of a routed line from a relation line is the length of the routed line that is
//! farther than `NEAR_M` from the relation line, plus the length of the relation line that is
//! farther than `NEAR_M` from the routed line. A plan reproduces a relation when the deviation of
//! its route is at most `SHARE` of the relation length, for every profile.
use route_engine::{
    data::RoutingData,
    model::{Pace, Point},
    Control, Error, Request, Route, Router,
};
use std::collections::{HashMap, HashSet};

/// Longitude and latitude in microdegrees.
pub type P = [i32; 2];

pub const NEAR_M: f64 = 30.0;
pub const SHARE: f64 = 0.02;
pub const MAX_VIA: usize = 62;
/// The line keeps every point of the plan route within this distance.
pub const LINE_TOLERANCE_M: f64 = 50.0;
const STEP_M: f64 = 10.0;
/// Rounds of the whole-plan check before a route leaves the catalog.
const ROUNDS: usize = 6;
/// Router calls for the search of one route. A search that needs more does not converge, and the
/// route leaves the catalog; the bound keeps the bake of a large region within minutes.
pub const SEARCH_CALLS: usize = 400;
/// Shaping points in a row that do not cut the deviation over the budget by 2 %: the search stops.
const STALE_POINTS: usize = 8;

pub fn distance(a: P, b: P) -> f64 {
    let point = |p: P| Point { lon: p[0], lat: p[1], elevation: 0.0 };
    point(a).distance(point(b))
}

pub fn length(line: &[P]) -> f64 {
    line.windows(2).map(|w| distance(w[0], w[1])).sum()
}

pub fn request(profile: &str, points: &[P], turnarounds: Vec<usize>) -> Request {
    Request {
        points: points.iter().map(|p| [p[0] as f64 * 1e-6, p[1] as f64 * 1e-6]).collect(),
        profile: profile.into(),
        pace: Pace::default(),
        alternatives: false,
        alternatives_only: false,
        turnarounds,
        start_position: None,
        end_position: None,
    }
}

pub fn vertices(route: &Route) -> Vec<P> {
    route.geometry.iter().map(|p| [(p[0] * 1e6).round() as i32, (p[1] * 1e6).round() as i32]).collect()
}

/// Metres from `p` to the segment `a`–`b`.
fn to_segment(p: P, a: P, b: P) -> f64 {
    let scale = (p[1] as f64 * 1e-6).to_radians().cos() * 0.111195;
    let xy = |q: P| [(q[0] - p[0]) as f64 * scale, (q[1] - p[1]) as f64 * 0.111195];
    let (a, b) = (xy(a), xy(b));
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
    (a[0] + d[0] * t).hypot(a[1] + d[1] * t)
}

/// Answers whether a point lies within `NEAR_M` of a line.
struct Near<'a> {
    line: &'a [P],
    cell: [f64; 2],
    cells: HashMap<(i32, i32), Vec<u32>>,
}

impl<'a> Near<'a> {
    fn new(line: &'a [P]) -> Self {
        let lat = line.first().map_or(0.0, |p| p[1] as f64 * 1e-6);
        let cell_lat = NEAR_M / 0.111195;
        let cell = [cell_lat / lat.to_radians().cos().max(0.01), cell_lat];
        let mut cells = HashMap::<_, Vec<u32>>::new();
        let key = |p: P, axis: usize| (p[axis] as f64 / cell[axis]).floor() as i32;
        for k in 0..line.len() {
            let (a, b) = (line[k], line[(k + 1).min(line.len() - 1)]);
            for x in key(a, 0).min(key(b, 0))..=key(a, 0).max(key(b, 0)) {
                for y in key(a, 1).min(key(b, 1))..=key(a, 1).max(key(b, 1)) {
                    cells.entry((x, y)).or_default().push(k as u32);
                }
            }
        }
        Self { line, cell, cells }
    }

    fn near(&self, p: P) -> bool {
        let (x, y) = ((p[0] as f64 / self.cell[0]).floor() as i32, (p[1] as f64 / self.cell[1]).floor() as i32);
        (x - 1..=x + 1).any(|x| {
            (y - 1..=y + 1).any(|y| {
                self.cells.get(&(x, y)).is_some_and(|segments| {
                    segments.iter().any(|&k| {
                        let k = k as usize;
                        to_segment(p, self.line[k], self.line[(k + 1).min(self.line.len() - 1)]) <= NEAR_M
                    })
                })
            })
        })
    }
}

/// The length of `a` that is not near `b`, and the middle of the longest such stretch as a
/// distance along `a`.
fn off(a: &[P], b: &Near) -> (f64, Option<f64>) {
    let mut total = 0.0;
    let mut along = 0.0;
    let mut run: Option<f64> = None;
    let mut longest = (0.0, None);
    let close = |run: &mut Option<f64>, at: f64, longest: &mut (f64, Option<f64>)| {
        if let Some(start) = run.take() {
            if at - start > longest.0 {
                *longest = (at - start, Some((start + at) / 2.0));
            }
        }
    };
    for w in a.windows(2) {
        let length = distance(w[0], w[1]);
        let steps = (length / STEP_M).ceil().max(1.0);
        for s in 0..steps as usize {
            let t = (s as f64 + 0.5) / steps;
            let p = [
                w[0][0] + ((w[1][0] - w[0][0]) as f64 * t).round() as i32,
                w[0][1] + ((w[1][1] - w[0][1]) as f64 * t).round() as i32,
            ];
            let step = length / steps;
            if b.near(p) {
                close(&mut run, along, &mut longest);
            } else {
                total += step;
                run.get_or_insert(along);
            }
            along += step;
        }
    }
    close(&mut run, along, &mut longest);
    (total, longest.1)
}

/// The deviation of a routed line from a relation line, in metres.
pub fn deviation(routed: &[P], line: &[P]) -> f64 {
    off(routed, &Near::new(line)).0 + off(line, &Near::new(routed)).0
}

#[derive(Clone, Copy)]
struct Leg {
    off: f64,
    /// The line vertex that a new shaping point in this leg should use.
    via: Option<usize>,
}

pub struct Plan {
    /// Start, shaping points and finish, as the client sends them.
    pub points: Vec<P>,
    /// Indices in `points` where the plan route turns back.
    pub turnarounds: Vec<usize>,
    /// The route of the first profile.
    pub route: Route,
}

#[derive(Debug, PartialEq)]
pub enum Failure {
    TooManyPoints,
    Check,
    /// The router reached its resource limit.
    Limit,
    /// The search used all its router calls.
    Budget,
}

/// The search for one line. A plan is a list of ascending line vertex indices.
struct Search<'a, D> {
    router: &'a mut Router<D>,
    profiles: &'a [&'a str],
    line: &'a [P],
    along: Vec<f64>,
    budget: f64,
    /// Legs routed alone, by profile and line vertices.
    legs: HashMap<(usize, usize, usize), Leg>,
    /// Vertices that stay in the plan.
    fixed: HashSet<usize>,
    /// Vertices that were in the plan once; a second try does not help.
    tried: HashSet<usize>,
    /// A leg reached the router limit, so a failed search is the limit's fault.
    limited: bool,
    /// Router calls so far, for the caller.
    calls: &'a mut usize,
}

impl<D: RoutingData> Search<'_, D> {
    fn leg(&mut self, p: usize, i: usize, j: usize) -> Leg {
        if let Some(leg) = self.legs.get(&(p, i, j)) {
            return *leg;
        }
        let points = [self.line[i], self.line[j]];
        let routed = match self.route(self.profiles[p], &points, &[]) {
            Ok(route) => vertices(&route),
            Err(failure) => {
                self.limited |= failure == Failure::Limit;
                Vec::new()
            }
        };
        let leg = self.compare(&routed, i, j);
        self.legs.insert((p, i, j), leg);
        leg
    }

    /// Compares a routed line with the part of the line between vertices `i` and `j`.
    fn compare(&self, routed: &[P], i: usize, j: usize) -> Leg {
        let part = &self.line[i..=j];
        let (missed, middle) = off(part, &Near::new(routed));
        let (extra, detour) = off(routed, &Near::new(part));
        let nearest = |key: &dyn Fn(usize) -> f64| (i + 1..j).min_by(|&a, &b| key(a).total_cmp(&key(b)));
        let via = match (middle, detour) {
            (Some(m), _) => nearest(&|k| (self.along[k] - self.along[i] - m).abs()),
            (None, Some(m)) => {
                let point = point_at(routed, m);
                nearest(&|k| distance(self.line[k], point))
            }
            _ => None,
        };
        Leg { off: missed + extra, via }
    }

    /// The deviation of each profile.
    fn totals(&mut self, plan: &[usize]) -> Vec<f64> {
        (0..self.profiles.len()).map(|p| plan.windows(2).map(|w| self.leg(p, w[0], w[1]).off).sum()).collect()
    }

    /// The shaping point of the worst leg, of a profile over the budget, that has an untried one.
    fn next(&mut self, plan: &[usize], totals: &[f64]) -> Option<usize> {
        let mut legs: Vec<Leg> = (0..self.profiles.len())
            .filter(|&p| totals[p] > self.budget)
            .flat_map(|p| plan.windows(2).map(move |w| (p, w[0], w[1])))
            .collect::<Vec<_>>()
            .into_iter()
            .map(|(p, i, j)| self.leg(p, i, j))
            .collect();
        untried(&mut legs, &self.tried)
    }

    /// Removes each shaping point whose removal keeps every profile within the budget, and does
    /// not make a profile over the budget worse.
    fn prune(&mut self, plan: &mut Vec<usize>, totals: &mut [f64]) {
        for k in plan.clone() {
            let at = plan.binary_search(&k).unwrap();
            if at == 0 || at == plan.len() - 1 || self.fixed.contains(&k) {
                continue;
            }
            let (a, b) = (plan[at - 1], plan[at + 1]);
            let changes: Vec<f64> = (0..self.profiles.len())
                .map(|p| self.leg(p, a, b).off - self.leg(p, a, k).off - self.leg(p, k, b).off)
                .collect();
            if totals.iter().zip(&changes).all(|(total, change)| total + change <= self.budget.max(*total)) {
                plan.remove(at);
                totals.iter_mut().zip(&changes).for_each(|(total, change)| *total += change);
            }
        }
    }

    /// Why the search found no plan.
    fn failure(&self) -> Failure {
        if *self.calls >= SEARCH_CALLS {
            Failure::Budget
        } else if self.limited {
            Failure::Limit
        } else {
            Failure::Check
        }
    }

    fn insert(&mut self, plan: &mut Vec<usize>, via: Option<usize>) -> Result<(), Failure> {
        let via = via.ok_or_else(|| self.failure())?;
        self.tried.insert(via);
        let at = plan.binary_search(&via).err().ok_or_else(|| self.failure())?;
        plan.insert(at, via);
        Ok(())
    }

    fn route(&mut self, profile: &str, points: &[P], turnarounds: &[usize]) -> Result<Route, Failure> {
        if *self.calls >= SEARCH_CALLS {
            return Err(Failure::Budget);
        }
        *self.calls += 1;
        self.router.route(&request(profile, points, turnarounds.to_vec()), &Control::default()).map_err(|error| {
            if matches!(error, Error::Limit) {
                Failure::Limit
            } else {
                Failure::Check
            }
        })
    }
}

/// The shaping point of the worst leg whose shaping point was never in the plan.
fn untried(legs: &mut [Leg], tried: &HashSet<usize>) -> Option<usize> {
    legs.sort_by(|a, b| b.off.total_cmp(&a.off));
    legs.iter().filter_map(|leg| leg.via).find(|k| !tried.contains(k))
}

fn point_at(line: &[P], at: f64) -> P {
    let mut along = 0.0;
    for w in line.windows(2) {
        let length = distance(w[0], w[1]);
        if along + length >= at {
            let t = ((at - along) / length.max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
            return [
                w[0][0] + ((w[1][0] - w[0][0]) as f64 * t).round() as i32,
                w[0][1] + ((w[1][1] - w[0][1]) as f64 * t).round() as i32,
            ];
        }
        along += length;
    }
    line[line.len() - 1]
}

/// Finds a plan whose route follows `line` with each profile. The first profile gives the plan
/// route. The budget is `SHARE` of `length`, the length of the main line without its patches.
///
/// Each leg is first routed alone. Of the profiles over the budget, the worst leg with an untried
/// shaping point gets that point, in the middle of its longest stretch away from the line, until
/// the plan is within the budget. Then every shaping point that does not help goes. The whole
/// plan is routed last; when it fails, its worst leg with an untried point gets that point and the
/// search continues.
pub fn shape<D: RoutingData>(
    router: &mut Router<D>,
    profiles: &[&str],
    line: &[P],
    length: f64,
    closed: bool,
    calls: &mut usize,
) -> Result<Plan, Failure> {
    let n = line.len();
    let mut along = vec![0.0];
    for w in line.windows(2) {
        along.push(along[along.len() - 1] + distance(w[0], w[1]));
    }
    let total = along[n - 1];
    // An out-and-back spur retraces its nodes; its tip is a turnaround.
    let tips: Vec<usize> = (1..n.saturating_sub(1)).filter(|&k| line[k - 1] == line[k + 1]).collect();
    let mut plan: Vec<usize> = [0, n - 1].into_iter().chain(tips.iter().copied()).collect();
    if closed {
        for share in [1.0 / 3.0, 2.0 / 3.0] {
            plan.push(along.partition_point(|&a| a < share * total).clamp(1, n - 2));
        }
    }
    plan.sort_unstable();
    plan.dedup();
    let mut search = Search {
        router,
        profiles,
        line,
        along,
        budget: SHARE * length,
        legs: HashMap::new(),
        fixed: tips.iter().copied().collect(),
        tried: plan.iter().copied().collect(),
        limited: false,
        calls,
    };
    let mut pruned_full = false;
    for _ in 0..ROUNDS {
        // The deviation over the budget, at its lowest so far, and the shaping points since then.
        let mut best = (f64::MAX, 0);
        loop {
            let mut totals = search.totals(&plan);
            if *search.calls >= SEARCH_CALLS {
                return Err(Failure::Budget);
            }
            if totals.iter().all(|&total| total <= search.budget) {
                search.prune(&mut plan, &mut totals);
                break;
            }
            let excess: f64 = totals.iter().map(|&total| (total - search.budget).max(0.0)).sum();
            best = if excess < 0.98 * best.0 { (excess, 0) } else { (best.0, best.1 + 1) };
            if best.1 >= STALE_POINTS {
                return Err(search.failure());
            }
            // One prune makes room; a search that fills the plan again does not converge.
            if plan.len() >= MAX_VIA + 2 {
                if pruned_full {
                    return Err(Failure::TooManyPoints);
                }
                pruned_full = true;
                search.prune(&mut plan, &mut totals);
            }
            let via = search.next(&plan, &totals);
            search.insert(&mut plan, via)?;
        }
        if plan.len() > MAX_VIA + 2 {
            return Err(Failure::TooManyPoints);
        }
        let turnarounds: Vec<usize> =
            plan.iter().enumerate().filter(|(_, k)| tips.contains(k)).map(|(i, _)| i).collect();
        let points: Vec<P> = plan.iter().map(|&k| line[k]).collect();
        // The client sends points on the road, where the first route attached them.
        let first = search.route(profiles[0], &points, &turnarounds)?;
        let geometry = vertices(&first);
        let mut snapped: Vec<P> = first.legs.iter().map(|leg| geometry[leg.from_index]).collect();
        snapped.push(if closed { snapped[0] } else { geometry[geometry.len() - 1] });
        let mut routes = Vec::new();
        let mut legs = Vec::new();
        for profile in profiles {
            let route = search.route(profile, &snapped, &turnarounds)?;
            let geometry = vertices(&route);
            if deviation(&geometry, line) > search.budget {
                legs.extend(
                    route
                        .legs
                        .iter()
                        .enumerate()
                        .map(|(k, l)| search.compare(&geometry[l.from_index..=l.to_index], plan[k], plan[k + 1])),
                );
            }
            routes.push(route);
        }
        if legs.is_empty() {
            return Ok(Plan { points: snapped, turnarounds, route: routes.swap_remove(0) });
        }
        let via = untried(&mut legs, &search.tried);
        search.insert(&mut plan, via)?;
        search.fixed.extend(via);
    }
    Err(search.failure())
}

/// Simplifies a line, keeping the vertices at `keep` (ascending indices). Returns the line and the
/// position of each kept vertex in it.
pub fn simplify(line: &[P], keep: &[usize]) -> (Vec<P>, Vec<usize>) {
    let mut out = vec![line[keep[0]]];
    let mut positions = vec![0];
    for w in keep.windows(2) {
        let mut selected = vec![false; w[1] - w[0] + 1];
        let mut pending = vec![(w[0], w[1])];
        while let Some((a, b)) = pending.pop() {
            let farthest = (a + 1..b)
                .map(|k| (to_segment(line[k], line[a], line[b]), k))
                .max_by(|x, y| x.0.total_cmp(&y.0))
                .filter(|(d, _)| *d > LINE_TOLERANCE_M);
            if let Some((_, k)) = farthest {
                selected[k - w[0]] = true;
                pending.extend([(a, k), (k, b)]);
            }
        }
        out.extend((w[0] + 1..w[1]).filter(|k| selected[k - w[0]]).map(|k| line[k]));
        out.push(line[w[1]]);
        positions.push(out.len() - 1);
    }
    (out, positions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deviation_counts_both_lines_and_simplify_keeps_marked_vertices() {
        // A 1.6 km line east. The route leaves it for 311 m, 100 m to the north.
        let line: Vec<P> = (0..=10).map(|i| [i * 1_400, 0]).collect();
        let routed: Vec<P> = [&line[..5], &[[5_600, 900], [8_400, 900]], &line[6..]].concat();
        // Off the line: 70 m up, 311 m east, 70 m down. Off the route: 311 m less 30 m at each end.
        let off = deviation(&routed, &line);
        assert!((680.0..720.0).contains(&off), "{off}");
        assert_eq!(deviation(&line, &line), 0.0);
        let (simple, positions) = simplify(&routed, &[0, 6, 11]);
        assert_eq!(simple, [line[0], line[4], routed[5], routed[6], routed[7], line[10]]);
        assert_eq!(positions, [0, 3, 5]);
    }
}
