//! The rows that tie the heights of a solve to each other: the approaches of side roads, the fords,
//! the chains of tunnels and bridges, the tracks side by side, the decks over what they cross, and
//! the ways that pass over each other.

use std::collections::{BTreeSet, HashMap, HashSet};

use rstar::primitives::GeomWithData;
use rstar::{RTree, AABB};

use super::network::{dot, hypot, sub, Network};
use super::*;
use crate::geometry::key;
use crate::{metres, Terrain};

/// Rows of up to four variables: Σ coefficient × z_variable, against a value.
#[derive(Default)]
pub(crate) struct Pins<const K: usize> {
    pub variables: Vec<[usize; K]>,
    pub coefficients: Vec<[f64; K]>,
    pub values: Vec<f64>,
}

impl<const K: usize> Pins<K> {
    fn push(&mut self, variables: [usize; K], coefficients: [f64; K], value: f64) {
        self.variables.push(variables);
        self.coefficients.push(coefficients);
        self.values.push(value);
    }
}

/// Per side direction of a junction, the rows that put its vertices within its approach distance on
/// the surface of the through road: z_P − (1 − τ)·z_J − τ·z_Q = 0, with Q the next vertex of the
/// through direction that the side leans along most and τ ≥ 0 the place of P along J-Q. A side pins
/// only the vertices at least a metre nearer to its junction than to the next junction along its
/// way, so that the two ends of a short link do not pin one vertex. Also, per segment, whether it
/// lies within the approach distance of a junction where its way is a side. A tunnel that leaves a
/// junction as a side runs straight (`chains`) and has no approach.
pub(crate) fn approaches(net: &Network) -> (Pins<3>, Vec<bool>) {
    let mut approached = vec![false; net.segment_start.len()];
    let mut segment = vec![usize::MAX; net.vertex_xy.len()];
    for (s, &v) in net.segment_start.iter().enumerate() {
        segment[v] = s;
    }
    // (side, vertex, metres along) in the order of the steps of the walk.
    let mut found: Vec<Vec<(usize, usize, f64)>> = Vec::new();
    let mut sides = Vec::new();
    for i in 0..net.sides.occurrence.len() {
        let occurrence = net.sides.occurrence[i];
        if net.tunnel[net.osm_way[occurrence]] {
            continue;
        }
        let step = net.sides.step[i];
        let j = net.osm_position[occurrence];
        let way = net.osm_way[occurrence];
        let side = sub(net.osm_xy[(occurrence as i64 + step) as usize], net.osm_xy[occurrence]);
        let (mut lean, mut q) = (f64::NEG_INFINITY, 0usize);
        for (o, st) in net.sides.through[i].iter().flatten() {
            let d = sub(net.osm_xy[(*o as i64 + st) as usize], net.osm_xy[*o]);
            let along = dot(side, d) / hypot(d).max(1e-300);
            if along > lean {
                (lean, q) = (along, (net.osm_position[*o] as i64 + st) as usize);
            }
        }
        // Walk the side to the next junction, or as far as it can decide a pin.
        let unit = metres(net.vertex_xy[j][1]);
        let reach = net.sides.distance[i] + 1.01;
        let (mut p, mut along, mut run) = (j, 0.0, f64::INFINITY);
        let (mut walking, mut inside, mut steps) = (true, true, 0);
        while walking {
            let ahead = p as i64 + step;
            walking &= ahead >= net.way_first[way] as i64 && ahead <= net.way_last[way] as i64;
            let ahead = if walking { ahead as usize } else { p };
            along += hypot(sub(net.vertex_xy[ahead], net.vertex_xy[p])) * unit;
            if inside && walking && along <= reach {
                approached[segment[p.min(ahead)]] = true;
            }
            p = ahead;
            let meets = walking && net.vertex_junction[p];
            if meets {
                run = along;
            }
            inside &= walking && !meets && along <= reach;
            if found.len() <= steps {
                found.push(Vec::new());
            }
            if inside {
                found[steps].push((sides.len(), p, along));
            }
            steps += 1;
            walking &= !meets && along <= 2.0 * reach + 1.0;
        }
        sides.push((j, q, run));
    }
    let mut pins = Pins::default();
    for (i, p, along) in found.into_iter().flatten() {
        let (j, q, run) = sides[i];
        if 2.0 * along + 1.0 > run {
            continue;
        }
        let origin = net.vertex_xy[j];
        let axis = sub(net.vertex_xy[q], origin);
        let tau = (dot(sub(net.vertex_xy[p], origin), axis) / dot(axis, axis).max(1e-300)).max(0.0);
        pins.push([net.vertex_var[p], net.vertex_var[j], net.vertex_var[q]], [1.0, -(1.0 - tau), -tau], 0.0);
    }
    (pins, approached)
}

/// Rows z_v − (1 − τ)·z_A − τ·z_B per vertex inside a chain of the ways that `members` marks, with A
/// and B the chain's ends and τ the vertex's share of its length: = 0 holds a tunnel straight, ≥ 0
/// keeps a bridge from sagging. Where the ends of member ways meet, a chain runs on through the two
/// that continue most nearly straight, and the others are side tunnels. So a side tunnel or a shaft
/// follows the tunnel that it joins and never lifts it: nothing but its ends gives a long tunnel its
/// heights.
pub(crate) fn chains(net: &Network, members: &[bool]) -> Pins<3> {
    let chained: Vec<usize> = (0..members.len()).filter(|&w| members[w]).collect();
    let mut meetings: Vec<Vec<(usize, usize, [f64; 2])>> = Vec::new();
    let mut slot: HashMap<usize, usize> = HashMap::new();
    for &w in &chained {
        for (end, v, u) in [(0, net.way_first[w], net.way_first[w] + 1), (1, net.way_last[w], net.way_last[w] - 1)] {
            let d = sub(net.vertex_xy[u], net.vertex_xy[v]);
            let norm = hypot(d).max(1e-300);
            let at = *slot.entry(net.vertex_var[v]).or_insert_with(|| {
                meetings.push(Vec::new());
                meetings.len() - 1
            });
            meetings[at].push((w, end, [d[0] / norm, d[1] / norm]));
        }
    }
    // At each meeting of ends, pair the ends that run most nearly straight on, the straightest first.
    let mut partner: HashMap<(usize, usize), (usize, usize)> = HashMap::new();
    for meeting in &meetings {
        let mut pairs = Vec::new();
        for (i, p) in meeting.iter().enumerate() {
            for (k, q) in meeting.iter().enumerate().skip(i + 1) {
                if p.0 != q.0 {
                    pairs.push((dot(p.2, q.2), i, k));
                }
            }
        }
        pairs.sort_by(|x, y| x.partial_cmp(y).unwrap());
        for (_, i, k) in pairs {
            let (p, q) = ((meeting[i].0, meeting[i].1), (meeting[k].0, meeting[k].1));
            if !partner.contains_key(&p) && !partner.contains_key(&q) {
                partner.insert(p, q);
                partner.insert(q, p);
            }
        }
    }
    let mut seen = HashSet::new();
    let mut pins = Pins::default();
    for &start in &chained {
        if seen.contains(&start) {
            continue;
        }
        // Back to the first way of the chain, then along it.
        let (mut w, mut end, mut steps) = (start, 0, 0);
        while let Some(&(other, other_end)) = partner.get(&(w, end)) {
            if other == start || steps >= chained.len() {
                break;
            }
            (w, end) = (other, 1 - other_end);
            steps += 1;
        }
        let (mut chain, mut flipped): (Vec<usize>, bool) = (Vec::new(), end == 1);
        while seen.insert(w) {
            let span: Vec<usize> = if flipped {
                (net.way_first[w]..=net.way_last[w]).rev().collect()
            } else {
                (net.way_first[w]..=net.way_last[w]).collect()
            };
            let skip = usize::from(!chain.is_empty());
            chain.extend_from_slice(&span[skip..]);
            let Some(&(next, next_end)) = partner.get(&(w, if flipped { 0 } else { 1 })) else { break };
            (w, flipped) = (next, next_end == 1);
        }
        let (a, b) = (net.vertex_var[chain[0]], net.vertex_var[chain[chain.len() - 1]]);
        if chain.len() < 3 || a == b {
            continue;
        }
        let mut along = vec![0.0];
        for pair in chain.windows(2) {
            let (p, q) = (net.vertex_xy[pair[0]], net.vertex_xy[pair[1]]);
            along.push(along[along.len() - 1] + hypot(sub(q, p)) * metres((p[1] + q[1]) / 2.0));
        }
        let total = along[along.len() - 1];
        for (n, &v) in chain.iter().enumerate().take(chain.len() - 1).skip(1) {
            let tau = along[n] / total;
            pins.push([net.vertex_var[v], a, b], [1.0, -(1.0 - tau), -tau], 0.0);
        }
    }
    pins
}

/// Rows z_P − (1 − τ)·z_A − τ·z_B = 0 for each vertex P of a track on the ground and the nearest
/// segment A-B of a track on the ground with a higher way number within PARALLEL_M beside it that
/// runs its way, τ the foot of P on A-B, where the terrain at P and at its foot is known and differs
/// by no more than PARALLEL_RISE_M, here and within PARALLEL_FREE_M along P's way. Each track
/// follows only tracks after it, so no two rows pull against each other, and the tracks of a double
/// line or a station lie at one height.
pub(crate) fn parallels(net: &Network, classes: &Classes, terrain: &dyn Terrain) -> Pins<3> {
    let rail: Vec<bool> = (0..net.rule.len()).map(|w| classes.rows[net.rule[w]].rail && !net.free[w]).collect();
    let segments: Vec<usize> = (0..net.segment_way.len()).filter(|&s| rail[net.segment_way[s]]).collect();
    if segments.is_empty() {
        return Pins::default();
    }
    let pa: Vec<[f64; 2]> = segments.iter().map(|&s| net.vertex_xy[net.segment_start[s]]).collect();
    let d: Vec<[f64; 2]> =
        segments.iter().zip(&pa).map(|(&s, &a)| sub(net.vertex_xy[net.segment_start[s] + 1], a)).collect();
    let length: Vec<f64> = d.iter().map(|&d| hypot(d)).collect();
    let unit: Vec<f64> = pa.iter().map(|p| metres(p[1])).collect();
    let reach = (PARALLEL_M + VERTEX_M / 2.0) / unit.iter().copied().fold(f64::INFINITY, f64::min);
    let tree: RTree<GeomWithData<[f64; 2], usize>> = RTree::bulk_load(
        (0..segments.len())
            .map(|k| GeomWithData::new([pa[k][0] + d[k][0] / 2.0, pa[k][1] + d[k][1] / 2.0], k))
            .collect(),
    );
    let mut found = Vec::new();
    for w in (0..rail.len()).filter(|&w| rail[w]) {
        for v in net.way_first[w]..=net.way_last[w] {
            let p = net.vertex_xy[v];
            let u = if v < net.way_last[w] { v + 1 } else { v - 1 };
            let o = sub(net.vertex_xy[u], p);
            let norm = hypot(o).max(1e-300);
            let own = [o[0] / norm, o[1] / norm];
            let mut near: Vec<usize> = tree
                .locate_in_envelope(&AABB::from_corners([p[0] - reach, p[1] - reach], [p[0] + reach, p[1] + reach]))
                .map(|g| g.data)
                .collect();
            near.sort_unstable();
            let mut best: Option<(f64, usize, f64)> = None;
            for k in near {
                let other = net.segment_way[segments[k]];
                if other <= w || (dot(own, d[k]) / length[k].max(1e-300)).abs() < PARALLEL_COS {
                    continue;
                }
                let t = (dot(sub(p, pa[k]), d[k]) / (length[k] * length[k]).max(1e-300)).clamp(0.0, 1.0);
                let gap = hypot([p[0] - pa[k][0] - t * d[k][0], p[1] - pa[k][1] - t * d[k][1]]) * unit[k];
                if gap <= PARALLEL_M && best.is_none_or(|b| gap < b.0) {
                    best = Some((gap, k, t));
                }
            }
            if let Some((_, k, t)) = best {
                found.push((v, k, t));
            }
        }
    }
    if found.is_empty() {
        return Pins::default();
    }
    let at: Vec<[f64; 2]> = found.iter().map(|&(v, _, _)| net.vertex_xy[v]).collect();
    let feet: Vec<[f64; 2]> = found.iter().map(|&(_, k, t)| [pa[k][0] + t * d[k][0], pa[k][1] + t * d[k][1]]).collect();
    let (here, there) = (terrain.heights(&at), terrain.heights(&feet));
    let mut level: Vec<bool> = here.iter().zip(&there).map(|(&h, &f)| (h - f).abs() <= PARALLEL_RISE_M).collect();
    // Metres along its way at each vertex, and the pins of each way that lie too near a parting.
    let mut steps = vec![0.0; net.vertex_xy.len()];
    for (s, &v) in net.segment_start.iter().enumerate() {
        steps[v + 1] = net.length[s];
    }
    let mut along = steps;
    for v in 1..along.len() {
        along[v] += along[v - 1];
    }
    let mut way = vec![0; net.vertex_xy.len()];
    for w in 0..net.way_first.len() {
        way[net.way_first[w]..=net.way_last[w]].fill(w);
    }
    let at_along = |v: usize| along[v] - along[net.way_first[way[v]]];
    let parted: BTreeSet<usize> = found.iter().zip(&level).filter(|(_, &l)| !l).map(|(f, _)| way[f.0]).collect();
    for w in parted {
        let mine: Vec<usize> = (0..found.len()).filter(|&i| way[found[i].0] == w).collect();
        let partings: Vec<f64> = mine.iter().filter(|&&i| !level[i]).map(|&i| at_along(found[i].0)).collect();
        for &i in &mine {
            let s = at_along(found[i].0);
            level[i] &= partings.iter().map(|&p| (s - p).abs()).fold(f64::INFINITY, f64::min) > PARALLEL_FREE_M;
        }
    }
    let mut pins = Pins::default();
    for (&(v, k, t), _) in found.iter().zip(&level).filter(|(_, &l)| l) {
        let s = segments[k];
        pins.push([net.vertex_var[v], net.a[s], net.b[s]], [1.0, -(1.0 - t), -t], 0.0);
    }
    pins
}

/// The rows of the fords: a road vertex that a waterway shares, and the point where a way tagged as
/// a ford crosses a waterway, lie FORD_M below the water there. A bridge or a tunnel has no ford.
pub(crate) fn fords(net: &Network, rivers: &[&River]) -> Pins<3> {
    let mut pins = Pins::default();
    if rivers.is_empty() {
        return pins;
    }
    let mut levels: HashMap<[u64; 2], f64> = HashMap::new();
    let mut segments = Vec::new();
    for river in rivers {
        for (point, &level) in river.points.iter().zip(&river.levels) {
            if level.is_finite() {
                levels.entry(key(*point)).or_insert(level);
            }
        }
        for k in 1..river.points.len() {
            let (zp, zq) = (river.levels[k - 1], river.levels[k]);
            if !zp.is_nan() && !zq.is_nan() {
                segments.push((river.points[k - 1], river.points[k], zp, zq));
            }
        }
    }
    for o in 0..net.osm_xy.len() {
        if !net.free[net.osm_way[o]] {
            if let Some(level) = levels.get(&key(net.osm_xy[o])) {
                pins.push([net.osm_var[o]; 3], [1.0, 0.0, 0.0], level - FORD_M);
            }
        }
    }
    let fords: Vec<usize> = (0..net.ford.len()).filter(|&w| net.ford[w] && !net.free[w]).collect();
    if fords.is_empty() {
        return pins;
    }
    let grid = Grid::new(segments.iter().map(|s| (s.0, s.1)));
    for w in fords {
        for v in net.way_first[w]..net.way_last[w] {
            let (p, q) = (net.vertex_xy[v], net.vertex_xy[v + 1]);
            for number in grid.near(p, q) {
                let (r0, r1, z0, z1) = segments[number];
                if let Some((u, tr)) = crossing(p, q, r0, r1) {
                    let variables = [net.vertex_var[v], net.vertex_var[v + 1], net.vertex_var[v]];
                    pins.push(variables, [1.0 - u, u, 0.0], z0 + (z1 - z0) * tr - FORD_M);
                }
            }
        }
    }
    pins
}

/// The rows of the decks over what they cross, and per row its bridge: where a bridge segment
/// crosses a waterway, z ≥ water + CLEAR_WATER_M + its deck; where it crosses a road, a path or a
/// track on the ground that it does not meet, z − z_below ≥ CLEAR_ROAD_M, CLEAR_PATH_M or
/// CLEAR_RAIL_M + its deck; but a bridge whose end lies on that road, within its half-width, meets it
/// there and has no row. Where a bridge lands at a height that it cannot move (`landings`), its deck
/// climbs no steeper than it runs: a crossing that asks more lies where the bridge lands and has no row.
pub(crate) fn clearances(
    net: &Network,
    rivers: &[&River],
    classes: &Classes,
    terrain: &dyn Terrain,
) -> (Pins<4>, Vec<usize>) {
    let deck = |w: usize| if net.rule[w] >= classes.path { DECK_PATH_M } else { DECK_M };
    let bridge: Vec<bool> = (0..net.free.len()).map(|w| net.free[w] && !net.tunnel[w]).collect();
    let bridges: Vec<usize> = (0..bridge.len()).filter(|&w| bridge[w]).collect();
    // Waterway segments with a level at both ends, and the segments on the ground in the cells of the bridges.
    enum Below {
        Water(f64, f64),
        Road(usize, usize),
    }
    let mut below: Vec<Segment<Below>> = Vec::new();
    for river in rivers {
        for k in 1..river.points.len() {
            let (zp, zq) = (river.levels[k - 1], river.levels[k]);
            if !zp.is_nan() && !zq.is_nan() {
                below.push((river.points[k - 1], river.points[k], Below::Water(zp, zq)));
            }
        }
    }
    let ground: Vec<bool> = net.free.iter().map(|&f| !f).collect();
    below.extend(near(net, &bridges, &ground).into_iter().map(|(p, q, (w, k))| (p, q, Below::Road(w, k))));
    let grid = Grid::new(below.iter().map(|b| (b.0, b.1)));
    let landed = landings(net);
    let mut continued = vec![0usize; net.osm_count];
    for o in 0..net.osm_xy.len() {
        if bridge[net.osm_way[o]] {
            continued[net.osm_var[o]] += 1;
        }
    }
    let mut pins = Pins::default();
    let mut ways = Vec::new();
    for w in bridges {
        let (first, last) = (net.way_first[w], net.way_last[w]);
        let mut along = vec![0.0];
        for s in first - w..last - w {
            along.push(along[along.len() - 1] + net.length[s]);
        }
        let tips: Vec<f64> =
            [first, last].iter().filter(|&&v| continued[net.vertex_var[v]] < 2).map(|&v| along[v - first]).collect();
        let ends: Vec<(f64, f64)> = [first, last]
            .iter()
            .filter(|&&v| landed.contains(&net.vertex_var[v]))
            .map(|&v| (along[v - first], terrain.height(net.vertex_xy[v])))
            .collect();
        let mut add = |variables: [usize; 4], coefficients: [f64; 4], value: f64, top: f64, at: f64| {
            if ends.iter().all(|&(place, ground)| top - ground <= (at - place).abs()) {
                pins.push(variables, coefficients, value);
                ways.push(w);
            }
        };
        for v in first..last {
            let (p, q) = (net.vertex_xy[v], net.vertex_xy[v + 1]);
            for number in grid.near(p, q) {
                let (r0, r1, ref crossed) = below[number];
                let Some((u, tr)) = crossing(p, q, r0, r1) else { continue };
                let at = along[v - first] + u * (along[v - first + 1] - along[v - first]);
                let (va, vb) = (net.vertex_var[v], net.vertex_var[v + 1]);
                let (other, k) = match *crossed {
                    Below::Water(z0, z1) => {
                        let value = z0 + (z1 - z0) * tr + CLEAR_WATER_M + deck(w);
                        add([va, vb, va, va], [1.0 - u, u, 0.0, 0.0], value, value, at);
                        continue;
                    }
                    Below::Road(other, k) => (other, k),
                };
                let (vk, vl) = (net.vertex_var[k], net.vertex_var[k + 1]);
                if va == vk || va == vl || vb == vk || vb == vl {
                    continue;
                }
                if !((0.0..1.0).contains(&tr) && (0.0..1.0).contains(&u))
                    || tips.iter().any(|&tip| (at - tip).abs() <= net.half[other])
                {
                    continue;
                }
                let clear = clearance(net, classes, other) + deck(w);
                let top = terrain.height([p[0] + u * (q[0] - p[0]), p[1] + u * (q[1] - p[1])]) + clear;
                add([va, vb, vk, vl], [1.0 - u, u, -(1.0 - tr), -tr], clear, top, at);
            }
        }
    }
    (pins, ways)
}

fn clearance(net: &Network, classes: &Classes, lower: usize) -> f64 {
    if classes.rows[net.rule[lower]].rail {
        CLEAR_RAIL_M
    } else if net.rule[lower] >= classes.path {
        CLEAR_PATH_M
    } else {
        CLEAR_ROAD_M
    }
}

/// The variables where a bridge lands at a height that it cannot move: an end that meets no other way
/// stands on the terrain, and one that leaves a junction as a side lies on its through road.
pub(crate) fn landings(net: &Network) -> HashSet<usize> {
    let mut ends = HashSet::new();
    for w in (0..net.free.len()).filter(|&w| net.free[w] && !net.tunnel[w]) {
        let last = net.osm_first.get(w + 1).copied().unwrap_or(net.osm_var.len()) - 1;
        ends.insert(net.osm_first[w]);
        ends.insert(last);
    }
    let loose = ends.iter().filter(|&&o| !net.junction[net.osm_var[o]]);
    let sides = net.sides.occurrence.iter().filter(|o| ends.contains(o));
    loose.chain(sides).map(|&o| net.osm_var[o]).collect()
}

/// The ways that cross without meeting, one over the other: a tunnel under the way that it crosses,
/// by its bore and COVER_M; of two bridges that no bridge joins or that both tag different layers,
/// and of two ways on the ground of different layers that are not `indoor`, the one of the higher
/// layer, at an equal layer of the better class, and of one class the later one, over the other, by
/// the clearance of the lower and the deck of the upper. A road on the ground over a tunnel keeps its
/// height, and the tunnel keeps the margin under its centreline and both edges; any other pair moves
/// apart, the stiffer less. (1 − u)·z_a + u·z_b ≷ (1 − r)·z_c + r·z_d ± the margin.
pub(crate) struct Passes {
    pub upper: Vec<([usize; 2], [f64; 2])>,
    pub lower: Vec<([usize; 2], [f64; 2])>,
    pub margin: Vec<f64>,
    pub lower_way: Vec<usize>,
    /// Whether the upper way moves too.
    pub moves: Vec<bool>,
    /// Whether the lower way is a tunnel under a road on the ground.
    pub underpass: Vec<bool>,
}

pub(crate) fn passes(net: &Network, classes: &Classes) -> Passes {
    let mut out =
        Passes { upper: vec![], lower: vec![], margin: vec![], lower_way: vec![], moves: vec![], underpass: vec![] };
    let mut grounded = vec![false; net.count];
    for s in 0..net.a.len() {
        if !net.free[net.segment_way[s]] {
            grounded[net.a[s]] = true;
            grounded[net.b[s]] = true;
        }
    }
    let mut add = |upper: usize, v: usize, lower: usize, k: usize, margin: f64| {
        let (p, q) = (net.vertex_xy[v], net.vertex_xy[v + 1]);
        let Some((u, r)) = crossing(p, q, net.vertex_xy[k], net.vertex_xy[k + 1]) else { return };
        let (va, vb) = (net.vertex_var[v], net.vertex_var[v + 1]);
        let under = [net.vertex_var[k], net.vertex_var[k + 1]];
        if under.contains(&va) || under.contains(&vb) || !((0.0..1.0).contains(&u) && (0.0..1.0).contains(&r)) {
            return;
        }
        let above = ([va, vb], [1.0 - u, u]);
        let mut places = vec![r];
        let tunnel_under_road = net.tunnel[lower] && !net.free[upper];
        if tunnel_under_road {
            // Where the tunnel's line meets each edge of the road, on this segment of the tunnel at most.
            let e = sub(q, p);
            let (norm, half, unit) = (hypot(e), net.half[upper], metres(p[1]));
            let side = [-e[1] / norm * half / unit, e[0] / norm * half / unit];
            for offset in [side, [-side[0], -side[1]]] {
                let Some((edge, _)) =
                    lines(net.vertex_xy[k], net.vertex_xy[k + 1], [p[0] + offset[0], p[1] + offset[1]], e)
                else {
                    continue;
                };
                let x = edge.clamp(0.0, 1.0);
                // An edge past a portal would pin the portal, and with it the way on the ground that leaves it.
                if x != edge && grounded[under[x as usize]] {
                    continue;
                }
                places.push(x);
            }
        }
        for x in places {
            out.upper.push(above);
            out.lower.push((under, [1.0 - x, x]));
            out.margin.push(margin);
            out.lower_way.push(lower);
            out.moves.push(!tunnel_under_road);
            out.underpass.push(tunnel_under_road);
        }
    };
    let bore = |w: usize| {
        if classes.rows[net.rule[w]].rail {
            BORE_RAIL_M
        } else if net.rule[w] >= classes.path {
            BORE_PATH_M
        } else {
            BORE_M
        }
    };
    let ways = |mask: &dyn Fn(usize) -> bool| (0..net.free.len()).filter(|&w| mask(w)).collect::<Vec<_>>();
    let not_tunnel: Vec<bool> = net.tunnel.iter().map(|&t| !t).collect();
    for (w, v, other, k) in crossed(net, &ways(&|w| net.tunnel[w]), &not_tunnel) {
        add(other, k, w, v, bore(w) + COVER_M);
    }
    let bridge: Vec<bool> = (0..net.free.len()).map(|w| net.free[w] && !net.tunnel[w]).collect();
    // The bridges that bridge ways join are one structure, such as a spiral stair: its ways set their
    // heights, but where both tag different layers, as the ramps of an interchange. Of two of one layer
    // and class, the later one passes over.
    let structure = structures(net, &bridge);
    let margin = |upper: usize, lower: usize| {
        clearance(net, classes, lower) + if net.rule[upper] >= classes.path { DECK_PATH_M } else { DECK_M }
    };
    let rank = |i: usize| (net.layer[i], -(net.rule[i] as i64), i);
    for (w, v, other, k) in crossed(net, &ways(&|w| bridge[w]), &bridge) {
        let layers = net.layered[w] && net.layered[other] && net.layer[w] != net.layer[other];
        if rank(w) > rank(other) && (structure[w] != structure[other] || layers) {
            add(w, v, other, k, margin(w, other));
        }
    }
    // A pair of ways on the ground of different layers has one of layer 0 at most; the other one finds it.
    let ground: Vec<bool> = (0..net.free.len()).map(|w| !net.free[w] && !net.indoor[w]).collect();
    for (w, v, other, k) in crossed(net, &ways(&|w| ground[w] && net.layer[w] != 0), &ground) {
        if net.layer[w] > net.layer[other] {
            add(w, v, other, k, margin(w, other));
        } else if net.layer[w] < net.layer[other] && net.layer[other] == 0 {
            add(other, k, w, v, margin(other, w));
        }
    }
    out
}

/// Per way, the structure of bridges that share a point with it; a way that is no bridge is its own.
fn structures(net: &Network, bridge: &[bool]) -> Vec<usize> {
    let ways = net.free.len();
    let mut parent: Vec<usize> = (0..ways + net.osm_count).collect();
    for o in 0..net.osm_xy.len() {
        if bridge[net.osm_way[o]] {
            union(&mut parent, net.osm_way[o], ways + net.osm_var[o]);
        }
    }
    (0..ways).map(|w| root(&mut parent, w)).collect()
}

pub(crate) fn root(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

pub(crate) fn union(parent: &mut [usize], a: usize, b: usize) {
    let (a, b) = (root(parent, a), root(parent, b));
    parent[a] = b;
}

/// Per segment of `ways` and segment of a way that `among` marks in its cells: (way, its first
/// vertex, other way, its first vertex).
fn crossed(net: &Network, ways: &[usize], among: &[bool]) -> Vec<(usize, usize, usize, usize)> {
    let segments = near(net, ways, among);
    let grid = Grid::new(segments.iter().map(|s| (s.0, s.1)));
    let mut out = Vec::new();
    for &w in ways {
        for v in net.way_first[w]..net.way_last[w] {
            for number in grid.near(net.vertex_xy[v], net.vertex_xy[v + 1]) {
                let (other, k) = segments[number].2;
                out.push((w, v, other, k));
            }
        }
    }
    out
}

/// The segments of the ways that `among` marks in the grid cells of `ways`: (start, end, (way, its
/// first vertex)).
fn near(net: &Network, ways: &[usize], among: &[bool]) -> Vec<Segment<(usize, usize)>> {
    let mut cells_of_ways = HashSet::new();
    for &w in ways {
        for v in net.way_first[w]..net.way_last[w] {
            cells_of_ways.extend(cells(net.vertex_xy[v], net.vertex_xy[v + 1]));
        }
    }
    let mut out = Vec::new();
    if cells_of_ways.is_empty() {
        return out;
    }
    for s in 0..net.segment_way.len() {
        if !among[net.segment_way[s]] {
            continue;
        }
        let v = net.segment_start[s];
        let (p, q) = (net.vertex_xy[v], net.vertex_xy[v + 1]);
        let cell = |x: f64| (x / GRID_UNITS).floor() as i64;
        let (lo, hi) = ([cell(p[0].min(q[0])), cell(p[1].min(q[1]))], [cell(p[0].max(q[0])), cell(p[1].max(q[1]))]);
        // A segment is shorter than a cell: its corner cells hold it.
        if [(lo[0], lo[1]), (hi[0], lo[1]), (lo[0], hi[1]), (hi[0], hi[1])].iter().any(|c| cells_of_ways.contains(c)) {
            out.push((p, q, (net.segment_way[s], v)));
        }
    }
    out
}

/// A segment (start, end) and what it is.
type Segment<T> = ([f64; 2], [f64; 2], T);

/// Per cell of the grid, the numbers of the segments whose boxes meet it.
struct Grid(HashMap<(i64, i64), Vec<usize>>);

impl Grid {
    fn new(segments: impl Iterator<Item = ([f64; 2], [f64; 2])>) -> Self {
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        for (number, (p, q)) in segments.enumerate() {
            for cell in cells(p, q) {
                grid.entry(cell).or_default().push(number);
            }
        }
        Grid(grid)
    }

    /// The numbers of the segments in the cells of segment p-q, in order.
    fn near(&self, p: [f64; 2], q: [f64; 2]) -> Vec<usize> {
        let found: BTreeSet<usize> = cells(p, q).iter().filter_map(|c| self.0.get(c)).flatten().copied().collect();
        found.into_iter().collect()
    }
}

/// The cells of the grid that the box of segment p-q meets.
fn cells(p: [f64; 2], q: [f64; 2]) -> Vec<(i64, i64)> {
    let cell = |x: f64| (x / GRID_UNITS).floor() as i64;
    let (x0, x1) = (cell(p[0].min(q[0])), cell(p[0].max(q[0])));
    let (y0, y1) = (cell(p[1].min(q[1])), cell(p[1].max(q[1])));
    (x0..=x1).flat_map(|cx| (y0..=y1).map(move |cy| (cx, cy))).collect()
}

/// The parameters (along p-q, along e from r) where the lines through p-q and along e from r meet.
fn lines(p: [f64; 2], q: [f64; 2], r: [f64; 2], e: [f64; 2]) -> Option<(f64, f64)> {
    let d = sub(q, p);
    let denominator = d[0] * e[1] - d[1] * e[0];
    if denominator == 0.0 {
        return None;
    }
    let f = sub(r, p);
    Some(((f[0] * e[1] - f[1] * e[0]) / denominator, (f[0] * d[1] - f[1] * d[0]) / denominator))
}

/// The parameters (along p-q, along r-s) where two segments cross.
fn crossing(p: [f64; 2], q: [f64; 2], r: [f64; 2], s: [f64; 2]) -> Option<(f64, f64)> {
    lines(p, q, r, sub(s, r)).filter(|&(u, t)| (0.0..=1.0).contains(&u) && (0.0..=1.0).contains(&t))
}
