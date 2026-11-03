//! The minimum width: the pinch search over the faces of a map (cell lines are no boundary), and the
//! thin pieces that go to the face of the lesser key beside them.
//!
//! A thin piece is one thin region of a face: the runs of its boundary that pinch each other, joined by
//! two closing edges. It follows the face's own edges, so applying it adds only the closing edges.

use std::collections::BTreeMap;

use super::cands::{Key, Labels};
use super::geom::{closest, hull, round, winding, P};
use super::slice::{Map, NONE};
use super::Ctx;
use crate::frame::metres_per_unit;

/// The pinch grid's cell: 512 frame units, about 0.8 m.
const GRID_SHIFT: u32 = 9;
/// Bisection steps for the end of a part.
const BISECT: u32 = 30;
/// A thin piece reaches to where the face is this many times the minimum width: what stays is wider.
const MARGIN: f64 = 1.25;
/// Parameters along a segment this close to an end are at the end.
const END: f64 = 1e-9;

/// A thin piece: the region that a face `from` loses to the face `to`.
#[derive(Clone, Debug)]
pub struct Thin {
    pub hull: Vec<P>,
    pub from: u32,
    pub to: u32,
    pub key: Key,
}

/// The target of a patch of label `label` under one pass of thin pieces: the lesser key among the
/// pieces of that label that hold its inner point (or the middle of its vertices, for a patch with
/// none).
pub fn apply(pass: &[Thin], label: u32, q: Option<[i64; 2]>, ring: &[P]) -> Option<u32> {
    let q = q.unwrap_or_else(|| {
        let n = ring.len() as i64;
        let sum = ring.iter().fold([0i64; 2], |s, p| [s[0] + i64::from(p[0]), s[1] + i64::from(p[1])]);
        [2 * sum[0] / n, 2 * sum[1] / n]
    });
    let mut best: Option<(Key, u32)> = None;
    for t in pass.iter().filter(|t| t.from == label) {
        if winding(std::slice::from_ref(&t.hull), q) != 0 && best.is_none_or(|(k, _)| t.key < k) {
            best = Some((t.key, t.to));
        }
    }
    best.map(|(_, to)| to)
}

struct Seg {
    h: u32,
    a: P,
    b: P,
}

/// One pinch: the parts (half-edge, from t, to t) of the boundary that pinch each other, and the
/// groups across them.
struct Pinch {
    parts: Vec<(u32, f64, f64)>,
    targets: [u32; 2],
}

/// The pinches of the map's faces (patches of one label joined across cell lines) and their thin
/// pieces; with `ground`, only faces and targets whose labels pass it. Also the thin regions with no
/// target.
pub fn pinches(
    ctx: &Ctx,
    map: &Map,
    labels: &[u32],
    names: &Labels,
    ground: Option<&dyn Fn(u32) -> bool>,
) -> (Vec<Thin>, u32) {
    let plan = &ctx.rules.plan;
    let groups = map.groups(labels, true);
    let label_of = |g: u32| labels[g as usize];
    let segs: Vec<Seg> = (0..map.half.len() as u32)
        .filter(|&h| map.bounds(&groups, h))
        .map(|h| {
            let e = map.half[h as usize];
            Seg { h, a: map.point(e.a), b: map.point(e.b) }
        })
        .collect();
    let mpu = metres_per_unit(ctx.origin.y);
    // A little more than the widest part in frame units anywhere in the window.
    let reach = (MARGIN * plan.min_width_m / mpu * 1.05).ceil() as i32 + 1;
    let mut cells: Vec<(u64, u32)> = Vec::new();
    let key = |x: i32, y: i32| ((x as u32 as u64) << 32) | y as u32 as u64;
    for (i, s) in segs.iter().enumerate() {
        let (x0, x1) = (s.a[0].min(s.b[0]) - reach, s.a[0].max(s.b[0]) + reach);
        let (y0, y1) = (s.a[1].min(s.b[1]) - reach, s.a[1].max(s.b[1]) + reach);
        for gx in (x0 >> GRID_SHIFT)..=(x1 >> GRID_SHIFT) {
            for gy in (y0 >> GRID_SHIFT)..=(y1 >> GRID_SHIFT) {
                cells.push((key(gx, gy), i as u32));
            }
        }
    }
    cells.sort_unstable();
    let mut found: BTreeMap<u32, Vec<Pinch>> = BTreeMap::new();
    for run in cells.chunk_by(|a, b| a.0 == b.0) {
        let cell = run[0].0;
        for (x, &(_, i)) in run.iter().enumerate() {
            for &(_, j) in &run[x + 1..] {
                let (s1, s2) = (&segs[i as usize], &segs[j as usize]);
                let g = groups[map.half[s1.h as usize].patch as usize];
                if g != groups[map.half[s2.h as usize].patch as usize] || ground.is_some_and(|f| !f(label_of(g))) {
                    continue;
                }
                // Each pair once: in the cell of the low corner of their grown boxes' overlap.
                let lo = |s: &Seg, axis: usize| s.a[axis].min(s.b[axis]) - reach;
                if key(lo(s1, 0).max(lo(s2, 0)) >> GRID_SHIFT, lo(s1, 1).max(lo(s2, 1)) >> GRID_SHIFT) != cell {
                    continue;
                }
                if let Some(p) = pinch(ctx, map, &groups, s1, s2) {
                    found.entry(g).or_default().push(p);
                }
            }
        }
    }
    let [wx0, wy0, wx1, wy1] = ctx.window;
    let border = 2 * reach;
    let mut thin = Vec::new();
    let mut left = 0;
    for (g, list) in found {
        let face = label_of(g);
        for (pieces, targets) in regions(map, &groups, &list) {
            if pieces
                .iter()
                .any(|p| p[0] < wx0 + border || p[0] > wx1 - border || p[1] < wy0 + border || p[1] > wy1 - border)
            {
                continue;
            }
            let best = (targets.into_iter())
                .filter(|&t| t != NONE)
                .map(label_of)
                .filter(|&l| l != face && ground.is_none_or(|f| f(l)))
                .min_by_key(|&l| names.keys[l as usize]);
            match best {
                Some(to) if pieces.len() >= 3 => {
                    thin.push(Thin { hull: pieces, from: face, to, key: names.keys[to as usize] })
                }
                _ => left += 1,
            }
        }
    }
    (resolve(thin, names), left)
}

/// The thin regions of one face's pinches: each a polygon of the face's boundary runs that pinch each
/// other, joined by closing edges, with the groups across those runs.
fn regions(map: &Map, groups: &[u32], list: &[Pinch]) -> Vec<(Vec<P>, Vec<u32>)> {
    // The parts of each half-edge, merged where they overlap: the nodes.
    let mut by_edge: BTreeMap<u32, Vec<(f64, f64, usize)>> = BTreeMap::new();
    for (i, p) in list.iter().enumerate() {
        for &(h, t0, t1) in &p.parts {
            by_edge.entry(h).or_default().push((t0.min(t1), t0.max(t1), i));
        }
    }
    let mut nodes: Vec<(u32, f64, f64)> = Vec::new();
    let mut of_pinch: Vec<Vec<usize>> = vec![Vec::new(); list.len()];
    for (&h, parts) in &mut by_edge {
        parts.sort_by(|a, b| a.0.total_cmp(&b.0));
        for &(t0, t1, i) in parts.iter() {
            match nodes.last_mut() {
                Some(n) if n.0 == h && t0 <= n.2 + END => n.2 = n.2.max(t1),
                _ => nodes.push((h, t0, t1)),
            }
            of_pinch[i].push(nodes.len() - 1);
        }
    }
    let at: BTreeMap<u32, Vec<usize>> = nodes.iter().enumerate().fold(BTreeMap::new(), |mut m, (i, n)| {
        m.entry(n.0).or_insert_with(Vec::new).push(i);
        m
    });
    // A node that reaches its edge's end runs on into a node at the start of the next edge.
    let mut succ = vec![usize::MAX; nodes.len()];
    let mut pred = vec![usize::MAX; nodes.len()];
    for (i, n) in nodes.iter().enumerate() {
        if n.2 >= 1.0 - END {
            let next = map.next(groups, n.0);
            if let Some(&j) = at.get(&next).and_then(|v| v.iter().find(|&&j| nodes[j].1 <= END)) {
                if pred[j] == usize::MAX && j != i {
                    succ[i] = j;
                    pred[j] = i;
                }
            }
        }
    }
    let mut root: Vec<usize> = (0..nodes.len()).collect();
    fn find(root: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while root[i] != i {
            root[i] = root[root[i]];
            i = root[i];
        }
        i
    }
    let mut join = |root: &mut Vec<usize>, a: usize, b: usize| {
        let (x, y) = (find(root, a), find(root, b));
        root[x.max(y)] = x.min(y);
    };
    for i in 0..nodes.len() {
        if succ[i] != usize::MAX {
            join(&mut root, i, succ[i]);
        }
    }
    for ns in &of_pinch {
        for w in ns.windows(2) {
            join(&mut root, w[0], w[1]);
        }
    }
    let mut members: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for i in 0..nodes.len() {
        members.entry(find(&mut root, i)).or_default().push(i);
    }
    let mut region_of = vec![0usize; nodes.len()];
    for (r, list) in members.values().enumerate() {
        for &i in list {
            region_of[i] = r;
        }
    }
    let mut targets: Vec<Vec<u32>> = vec![Vec::new(); members.len()];
    for (i, p) in list.iter().enumerate() {
        targets[region_of[of_pinch[i][0]]].extend(p.targets);
    }
    let point = |h: u32, t: f64| -> P {
        let e = map.half[h as usize];
        let (a, b) = (map.point(e.a), map.point(e.b));
        [a[0] + round(f64::from(b[0] - a[0]) * t) as i32, a[1] + round(f64::from(b[1] - a[1]) * t) as i32]
    };
    members
        .values()
        .zip(targets)
        .map(|(list, targets)| {
            // Each run from its first node; a closed run (a whole thin ring) from its least node.
            let mut seen = vec![false; nodes.len()];
            let mut runs: Vec<Vec<P>> = Vec::new();
            let starts: Vec<usize> =
                list.iter().copied().filter(|&i| pred[i] == usize::MAX).chain(list.iter().copied()).collect();
            for s in starts {
                if seen[s] {
                    continue;
                }
                let mut run = vec![point(nodes[s].0, nodes[s].1)];
                let mut i = s;
                loop {
                    seen[i] = true;
                    let (h, _, t1) = nodes[i];
                    let j = succ[i];
                    if j == usize::MAX || seen[j] {
                        run.push(point(h, t1));
                        break;
                    }
                    run.push(map.point(map.half[h as usize].b));
                    i = j;
                }
                run.dedup();
                runs.push(run);
            }
            let mut ring: Vec<P> = if runs.len() <= 2 { runs.concat() } else { hull(&mut runs.concat()) };
            ring.dedup();
            while ring.len() > 1 && ring.first() == ring.last() {
                ring.pop();
            }
            if super::geom::area2(&ring) < 0 {
                ring.reverse();
            }
            (ring, targets)
        })
        .collect()
}

/// Two faces that are thin beside each other would swap their thin parts: where a piece moves a
/// face into one of lesser rank and a touching piece moves that one back, only the move up stays.
fn resolve(thin: Vec<Thin>, names: &Labels) -> Vec<Thin> {
    let bbox = |t: &Thin| super::geom::bbox(std::slice::from_ref(&t.hull));
    let touch = |a: [i32; 4], b: [i32; 4]| a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3];
    let boxes: Vec<[i32; 4]> = thin.iter().map(bbox).collect();
    let down = |t: &Thin| names.keys[t.to as usize] > names.keys[t.from as usize];
    let mut order: Vec<usize> = (0..thin.len()).collect();
    order.sort_by_key(|&i| (boxes[i][0], i));
    let mut drop = vec![false; thin.len()];
    for (k, &i) in order.iter().enumerate() {
        for &j in &order[k + 1..] {
            if boxes[j][0] > boxes[i][2] {
                break;
            }
            let (a, b) = (&thin[i], &thin[j]);
            if a.from == b.to && a.to == b.from && touch(boxes[i], boxes[j]) {
                drop[i] |= down(a);
                drop[j] |= down(b);
            }
        }
    }
    thin.into_iter().zip(drop).filter(|(_, d)| !d).map(|(t, _)| t).collect()
}

/// Whether two boundary segments of one face pinch it, and if so their parts that do and the groups
/// across each segment.
fn pinch(ctx: &Ctx, map: &Map, groups: &[u32], s1: &Seg, s2: &Seg) -> Option<Pinch> {
    let plan = &ctx.rules.plan;
    let anchor = [ctx.origin.x + i64::from(s1.a[0]), ctx.origin.y + i64::from(s1.a[1])];
    let w = plan.min_width_m / metres_per_unit(anchor[1]);
    let sin_tip = libm::sin(plan.tip_deg.to_radians() / 2.0);
    let rel = |p: P| [f64::from(p[0] - s1.a[0]), f64::from(p[1] - s1.a[1])];
    let (a1, b1, a2, b2) = (rel(s1.a), rel(s1.b), rel(s2.a), rel(s2.b));
    let at = |a: [f64; 2], b: [f64; 2], t: f64| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
    let dist = |p: [f64; 2], q: [f64; 2]| ((p[0] - q[0]) * (p[0] - q[0]) + (p[1] - q[1]) * (p[1] - q[1])).sqrt();
    let (len1, len2) = (dist(a1, b1), dist(a2, b2));
    let (next1, next2) = (map.next(groups, s1.h), map.next(groups, s2.h));
    let targets = [map.across(groups, s1.h), map.across(groups, s2.h)];
    // Adjacent segments pinch only as a tip sharper than `tip_deg`.
    if next1 == s2.h || next2 == s1.h {
        let (inc, out) = if next1 == s2.h { (s1, s2) } else { (s2, s1) };
        let v = inc.b;
        let back = [f64::from(inc.a[0] - v[0]), f64::from(inc.a[1] - v[1])];
        let fwd = [f64::from(out.b[0] - v[0]), f64::from(out.b[1] - v[1])];
        let interior = libm::atan2(fwd[0] * back[1] - fwd[1] * back[0], fwd[0] * back[0] + fwd[1] * back[1]);
        if interior <= 0.0 || interior >= plan.tip_deg.to_radians() {
            return None;
        }
        let reach = MARGIN * w / libm::sin(interior).max(1e-9);
        let (lb, lf) = (dist(back, [0.0, 0.0]), dist(fwd, [0.0, 0.0]));
        let parts = vec![(inc.h, (1.0 - reach / lb).max(0.0), 1.0), (out.h, 0.0, (reach / lf).min(1.0))];
        return Some(Pinch { parts, targets });
    }
    let (d, s, t) = closest(s1.a, s1.b, s2.a, s2.b);
    if d >= w {
        return None;
    }
    let (p, q) = (at(a1, b1, s), at(a2, b2, t));
    if d > 0.0 {
        // The two segments face each other across the face: each lies on the face's side of the other.
        let side =
            |a: [f64; 2], b: [f64; 2], x: [f64; 2]| (b[0] - a[0]) * (x[1] - a[1]) - (b[1] - a[1]) * (x[0] - a[0]);
        if side(a1, b1, q) <= 0.0 || side(a2, b2, p) <= 0.0 {
            return None;
        }
    }
    // The shorter walk along the face's boundary from p to q, up to the limit that any pinch needs.
    let limit = w / sin_tip;
    let walk = |from: &Seg, f: f64, flen: f64, to: &Seg, tf: f64| -> f64 {
        let mut m = (1.0 - f) * flen;
        let mut h = map.next(groups, from.h);
        let mut guard = 0;
        while h != to.h {
            if m > limit || guard > 64 {
                return f64::INFINITY;
            }
            let e = map.half[h as usize];
            let (x, y) = (map.point(e.a), map.point(e.b));
            m += dist([f64::from(x[0]), f64::from(x[1])], [f64::from(y[0]), f64::from(y[1])]);
            h = map.next(groups, h);
            guard += 1;
        }
        m + tf * dist(rel(to.a), rel(to.b))
    };
    let around = walk(s1, s, len1, s2, t).min(walk(s2, t, len2, s1, s));
    if around <= d / sin_tip {
        return None;
    }
    // The parts of each segment within MARGIN times w of the other.
    let part = |a: [f64; 2], b: [f64; 2], c: P, dd: P, mid: f64| -> (f64, f64) {
        let near = |u: f64| {
            let x = at(a, b, u);
            let p = [s1.a[0] + round(x[0]) as i32, s1.a[1] + round(x[1]) as i32];
            closest(p, p, c, dd).0 < MARGIN * w
        };
        let edge = |mut inside: f64, mut outside: f64| {
            for _ in 0..BISECT {
                let m = (inside + outside) / 2.0;
                if near(m) {
                    inside = m;
                } else {
                    outside = m;
                }
            }
            inside
        };
        let lo = if near(0.0) { 0.0 } else { edge(mid, 0.0) };
        let hi = if near(1.0) { 1.0 } else { edge(mid, 1.0) };
        (lo, hi)
    };
    let (l1, h1) = part(a1, b1, s2.a, s2.b, s);
    let (l2, h2) = part(a2, b2, s1.a, s1.b, t);
    Some(Pinch { parts: vec![(s1.h, l1, h1), (s2.h, l2, h2)], targets })
}
