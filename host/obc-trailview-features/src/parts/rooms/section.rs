//! A room's centre line and section (Rooms in the spec): its lanes' places on the ring at each vertex, the frame lines
//! of its lanes at its ends with openings, and the height that holds its lanes.

use super::super::{along_dir, distance, first_way_m, left_of, offset, Curve, Input, Opening, Point, Tube};
use super::holes::Start;
use super::{Bound, Lane, Place, Seam};
use crate::metres;

/// The shift that moves arm opening o in along its way onto the frame line through p square to `dir`, within `most`
/// metres more, half its chain and its own way; None where its way does not reach the line.
pub(crate) fn onto(arm: &Tube, o: &Opening, (p, dir): ([f64; 2], [f64; 2]), most: f64) -> Option<f64> {
    let most = (o.shift + most).min(arm.curve.len / 2.0).min(first_way_m(arm, o.end));
    let off = |shift: f64| along_dir(p, o.at(arm, shift).p, dir);
    let (mut lo, mut hi) = (o.shift, most.max(o.shift));
    if off(hi) < 0.0 {
        return None;
    }
    if off(lo) >= 0.0 {
        return Some(lo);
    }
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if off(mid) < 0.0 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    Some(hi)
}

/// Where the line through p along the unit direction d crosses arm `arm`'s axis nearest its metres `near`: the metres
/// along the arm there, and the metres from p along d.
pub(super) fn crossing(arm: &Tube, p: [f64; 2], d: [f64; 2], near: f64) -> Option<(f64, f64)> {
    let m = metres(p[1]);
    let side = |q: [f64; 2]| ((q[0] - p[0]) * d[1] - (q[1] - p[1]) * d[0]) * m;
    let points: Vec<&Point> = arm.curve.stretches.iter().flat_map(|(_, ps)| ps.iter()).collect();
    let mut best: Option<(f64, f64)> = None;
    for w in points.windows(2) {
        let (x, y) = (side(w[0].p), side(w[1].p));
        if (x > 0.0 && y > 0.0) || (x < 0.0 && y < 0.0) || x == y {
            continue;
        }
        let u = x / (x - y);
        let s = w[0].s + u * (w[1].s - w[0].s);
        let q = [w[0].p[0] + u * (w[1].p[0] - w[0].p[0]), w[0].p[1] + u * (w[1].p[1] - w[0].p[1])];
        if best.is_none_or(|b| (s - near).abs() < (b.0 - near).abs()) {
            best = Some((s, along_dir(p, q, d)));
        }
    }
    best
}

/// An outline in a room: its metres across, its half-width, its floor over the room's, its height and the segments of
/// its arch, as the builder draws it (`arch_segments` per lane of its room).
pub(crate) type Outline = (f64, f64, f64, f64, usize);

/// The least height over the floor at which a room's outline (round or of box section, half-width `half`, its arch in
/// `segments` springing at the share `spring` of its height) holds each outline on its floor. A round room is read at
/// the corners of its outline and of theirs, as the builder draws them; its spring lies at least `wall` over the floor,
/// the highest floor at its walls, so each wall rises from the floor to the spring.
pub(crate) fn hold(round: bool, half: f64, spring: f64, outlines: &[Outline], segments: usize, wall: f64) -> f64 {
    if !round {
        return outlines.iter().map(|l| l.2 + l.3).fold(0.0, f64::max);
    }
    // The room's outline of height 1 from its right spring to its left, and its top at x.
    let arch = |n: usize, half: f64, sp: f64, h: f64| -> Vec<[f64; 2]> {
        (0..=n)
            .map(|k| {
                let a = std::f64::consts::PI * k as f64 / n as f64;
                [-half * a.cos(), sp + (h - sp) * a.sin()]
            })
            .collect()
    };
    let unit = arch(segments, half, spring, 1.0);
    let top = |x: f64| {
        let x = x.clamp(-half, half);
        let k = unit.partition_point(|q| q[0] < x).clamp(1, unit.len() - 1);
        let (p, q) = (unit[k - 1], unit[k]);
        p[1] + (q[1] - p[1]) * ((x - p[0]) / (q[0] - p[0]).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0)
    };
    let mut need = wall / spring;
    for &(off, h, up, height, n) in outlines {
        let corners = arch(n, h, spring * height, height).into_iter().chain([[-h, 0.0], [h, 0.0]]);
        for [x, y] in corners {
            need = need.max((up + y) / top(off + x));
        }
    }
    need
}

/// The metres along arm `to` at the foot of the point at metres s along arm `from`: where `to` crosses the square
/// through that point, nearest `near`.
pub(super) fn foot(arms: &[Tube], (from, s): (usize, f64), to: usize, near: f64) -> Option<f64> {
    let (p, d) = (arms[from].curve.at(s).0.p, arms[from].curve.dir(s));
    crossing(&arms[to], p, left_of(d), near).map(|x| x.0)
}

/// How far each vertex of a room behind a seam lies from its hole at least, the room `half` wide and its own line
/// `sideways` metres from the hole's point: its first chord, as long as this or longer, leaves its own line within
/// `pair_max_deg`, and its ring at its first vertex, turned by that much at most, lies wholly behind the line.
pub(super) fn clear(sideways: f64, half: f64, b: &obc_trailview_parts::Bake) -> f64 {
    let a = b.pair_max_deg.to_radians();
    (sideways / a.sin()).max(half * a.tan()).max(b.vertex_gap_m)
}

/// The tube of a room of one lane started at its end `end` at the hole's point p: no vertex but its other end lies
/// within `clear` metres of p, a join of two ways neither. A way with no vertex left has no stretch; a stretch whose
/// first vertex went starts at the vertex before, at its metres along this way's stroke.
pub(super) fn started(t: &mut Tube, end: usize, p: [f64; 2], clear: f64) {
    let curve = &mut t.curve;
    if end == 1 {
        flip(curve);
    }
    let len = curve.len;
    let mut out: Vec<(u32, Vec<Point>)> = Vec::new();
    let mut last = Point { p, ..curve.stretches[0].1[0] };
    for (way, ps) in &curve.stretches {
        let kept: Vec<Point> = ps[1..].iter().filter(|q| q.s >= len || distance(p, q.p) >= clear).copied().collect();
        if kept.is_empty() {
            continue;
        }
        let start = if last.s == ps[0].s {
            Point { p: last.p, ..ps[0] }
        } else {
            let sign = if ps[ps.len() - 1].along >= ps[0].along { 1.0 } else { -1.0 };
            Point { along: ps[0].along + sign * (last.s - ps[0].s), ..last }
        };
        last = kept[kept.len() - 1];
        out.push((*way, [start].into_iter().chain(kept).collect()));
    }
    curve.stretches = out;
    if end == 1 {
        flip(curve);
    }
}

/// The curve run from its other end: its stretches and their points in turn, their metres from that end.
fn flip(curve: &mut Curve) {
    let len = curve.len;
    curve.stretches.reverse();
    for (_, ps) in &mut curve.stretches {
        ps.reverse();
        ps.iter_mut().for_each(|q| q.s = len - q.s);
    }
}

/// The tube of a stretch of an arm from metres s0 to s1: its points between, its metres from s0 on.
pub(super) fn stretch(t: &Tube, [s0, s1]: [f64; 2]) -> Tube {
    let mut curve = Curve { stretches: Vec::new(), len: s1 - s0 };
    for (way, ps) in &t.curve.stretches {
        let (a, z) = (ps[0].s.max(s0), ps[ps.len() - 1].s.min(s1));
        if z - a <= 1e-9 {
            continue;
        }
        let mut points = vec![t.curve.at(a).0];
        points.extend(ps.iter().filter(|q| q.s > a && q.s < z).copied());
        points.push(t.curve.at(z).0);
        points.iter_mut().for_each(|q| q.s -= s0);
        curve.stretches.push((*way, points));
    }
    let mut ids: Vec<u32> = curve.stretches.iter().map(|w| w.0).collect();
    ids.dedup();
    Tube { curve, ids, ..t.clone() }
}

/// An end of a room on its line: the room's centre there, its left, and per lane its metres across, its half-width
/// along the line and its metres along its arm.
type EndRow = ([f64; 2], [f64; 2], Vec<(f64, f64, f64)>);

/// What `room_of` gives: the room's tube, its lanes from the right wall to the left, its section per vertex of its
/// centre line, its openings at its ends with openings, each lane opening moved onto its frame line (its place among
/// the arms' openings, the moved opening and the metres moved), the place of each lane whose way does not reach the
/// line, and per end behind a seam the metres from its hole within which no vertex lies (`clear`).
pub(super) type Built = (
    Tube,
    Vec<Lane>,
    Vec<(f64, Vec<Place>)>,
    [Option<Opening>; 2],
    Vec<(usize, Opening, f64)>,
    Vec<[f64; 2]>,
    [Option<f64>; 2],
);

/// Room r of `lanes`, the first its base, with its two ends (`bounds`, the seams' lines): at an end with openings,
/// the lanes' openings move in onto the frame line of the one that none lies past by more than `PAST_M`, then the
/// farthest in; at a seam the lanes end on its line; elsewhere on the line square to the base at its end. Its centre
/// line runs in the middle of its lanes' outermost edges.
pub(super) fn room_of(
    r: usize,
    lanes: &[Lane],
    (bounds, seams, starts): (&[Bound; 2], &[Seam], [Option<&Start>; 2]),
    arms: &[Tube],
    arm_openings: &[Opening],
    input: &Input,
) -> Built {
    let (b, mitre) = (&input.rules.bake, obc_trailview_parts::Rules::embedded().design.mitre_max);
    let mut lanes = lanes.to_vec();
    let mut openings = arm_openings.to_vec();
    let (mut moves, mut off_line) = (Vec::new(), Vec::new());
    let base_arm = &arms[lanes[0].arm];
    // Per end its line: a point, the unit direction into the room, and per lane its metres along its arm there and
    // its point on the line.
    let mut lines: Vec<([f64; 2], [f64; 2])> = Vec::new();
    let mut fars: [Option<usize>; 2] = [None, None];
    for end in 0..2 {
        let sign = if end == 0 { 1.0 } else { -1.0 };
        match bounds[end] {
            Bound::Open => {
                let list: Vec<usize> = lanes.iter().map(|l| l.ends[end].expect("an open end")).collect();
                let way = list.iter().fold([0.0, 0.0], |w, &i| [w[0] + openings[i].dir[0], w[1] + openings[i].dir[1]]);
                let along = |i: usize| along_dir(openings[list[0]].p, openings[i].p, way);
                let past = |f: usize, i: usize| {
                    along_dir(openings[f].p, openings[i].p, openings[f].dir) > obc_trailview_parts::PAST_M
                };
                let clear: Vec<usize> = list.iter().copied().filter(|&f| !list.iter().any(|&i| past(f, i))).collect();
                let pick = if clear.is_empty() { &list } else { &clear };
                let far = pick.iter().copied().max_by(|&x, &y| along(x).total_cmp(&along(y))).unwrap();
                let (pf, df) = (openings[far].p, openings[far].dir);
                for (k, &i) in list.iter().enumerate().filter(|&(_, &i)| i != far) {
                    let arm = &arms[lanes[k].arm];
                    let Some(shift) = onto(arm, &openings[i], (pf, df), f64::INFINITY) else {
                        off_line.push(openings[i].p);
                        continue;
                    };
                    let o = openings[i].clone();
                    openings[i] = Opening { others: o.others, ..o.at(arm, shift) };
                    moves.push((i, openings[i].clone(), shift - o.shift));
                    lanes[k].span[end] = openings[i].s;
                }
                fars[end] = Some(far);
                lines.push((pf, df));
            }
            Bound::Seam(s) => {
                let seam = &seams[s];
                lines.push((seam.p, if seam.parent.0 == r { seam.dir.map(|v| -v) } else { seam.dir }));
            }
            Bound::None => {
                let d = base_arm.curve.dir(lanes[0].span[end]).map(|v| v * sign);
                lines.push((base_arm.curve.at(lanes[0].span[end]).0.p, d));
            }
        }
    }
    let ends_open = [0, 1].map(|end| bounds[end] == Bound::Open && off_line.is_empty());
    // A lane of a narrow bore (`narrow_bore_m` or less) in a room with a wider one lies at the side of the nearest
    // wider lane, edge to edge, with its way's half-width and no margin: a walk beside that tunnel, not a place of its
    // own. Narrow lanes on one side of a wider one lie side by side outward, the nearest first.
    let narrow: Vec<bool> = lanes.iter().map(|l| arms[l.arm].height <= b.narrow_bore_m).collect();
    let snug = |mut places: Vec<(f64, f64, f64)>| {
        let wide: Vec<usize> = (0..lanes.len()).filter(|&k| !narrow[k]).collect();
        if wide.is_empty() || wide.len() == lanes.len() {
            return places;
        }
        let nearest = |t: f64| {
            *wide.iter().min_by(|&&x, &&y| (places[x].0 - t).abs().total_cmp(&(places[y].0 - t).abs())).unwrap()
        };
        let mut order: Vec<(usize, usize)> =
            (0..lanes.len()).filter(|&k| narrow[k]).map(|k| (k, nearest(places[k].0))).collect();
        order.sort_by(|x, y| (places[x.0].0 - places[x.1].0).abs().total_cmp(&(places[y.0].0 - places[y.1].0).abs()));
        let mut edges: Vec<[f64; 2]> = places.iter().map(|p| [p.0 - p.1, p.0 + p.1]).collect();
        for (k, w) in order {
            let half = places[k].1 * (arms[lanes[k].arm].half - b.margin_narrow_m) / arms[lanes[k].arm].half;
            let side = if places[k].0 >= places[w].0 { 1 } else { 0 };
            let t = if side == 1 { edges[w][1] + half } else { edges[w][0] - half };
            edges[w][side] = if side == 1 { t + half } else { t - half };
            places[k] = (t, half, places[k].2);
        }
        places
    };
    // The places of the lanes on a line through p with the unit direction `left`, whose normal is `normal`: per lane its
    // metres along `left`, its half-width along it, and its metres along its arm.
    let across = |p: [f64; 2], left: [f64; 2], normal: [f64; 2], near: &[f64]| -> Option<Vec<(f64, f64, f64)>> {
        let places = (lanes.iter().zip(near))
            .map(|(l, &s)| {
                let arm = &arms[l.arm];
                let (sl, t) = crossing(arm, p, left, s)?;
                let d = arm.curve.dir(sl);
                let cos = (d[0] * normal[0] + d[1] * normal[1]).abs().max(1.0 / mitre);
                Some((t, arm.half / cos, sl))
            })
            .collect::<Option<Vec<_>>>()?;
        Some(snug(places))
    };
    let middle = |places: &[(f64, f64, f64)]| {
        let lo = places.iter().map(|p| p.0 - p.1).fold(f64::INFINITY, f64::min);
        let hi = places.iter().map(|p| p.0 + p.1).fold(f64::NEG_INFINITY, f64::max);
        (lo + hi) / 2.0
    };
    // The places on each end's line, in the room's way: across to the left of its way.
    let own_rows: Vec<EndRow> = (0..2)
        .map(|end| {
            let (p, into) = lines[end];
            let way = if end == 0 { into } else { into.map(|v| -v) };
            let left = left_of(way);
            let places: Vec<(f64, f64, f64)> = (lanes.iter())
                .map(|l| {
                    let arm = &arms[l.arm];
                    let s = l.span[end];
                    let d = arm.curve.dir(s);
                    let cos = (d[0] * way[0] + d[1] * way[1]).abs().max(1.0 / mitre);
                    (along_dir(p, arm.curve.at(s).0.p, left), arm.half / cos, s)
                })
                .collect();
            let places = snug(places);
            let c = offset(p, left, middle(&places));
            let places = places.into_iter().map(|(t, h, s)| (along_dir(c, offset(p, left, t), left), h, s)).collect();
            (c, left, places)
        })
        .collect();
    // Behind a seam the room starts at its hole, with the places of the row before the line (across to the left of the
    // heading, which is the room's way at its end 0), and its next vertex lies far enough that its first chord leaves
    // the line within `pair_max_deg`.
    let end_rows: Vec<EndRow> = (0..2)
        .map(|end| {
            let (Some(st), (_, left, own)) = (starts[end], &own_rows[end]) else { return own_rows[end].clone() };
            let sign = if end == 0 { 1.0 } else { -1.0 };
            let at = |(k, l): (usize, &Lane)| {
                st.lanes.iter().find(|x| x.0 == l.arm).map_or(own[k], |x| (sign * x.1, x.2, x.3))
            };
            (st.p, *left, lanes.iter().enumerate().map(at).collect())
        })
        .collect();
    let clear = [0, 1].map(|end| {
        let half = own_rows[end].2.iter().map(|p| p.0.abs() + p.1).fold(0.0, f64::max);
        clear(distance(end_rows[end].0, own_rows[end].0), half, b)
    });
    // The centre line: on each end's line, and at each vertex of the base and the foot of each vertex of another lane
    // between them, the middle between the outermost edges of the lanes; its vertices `vertex_gap_m` apart at least.
    let [s0, s1] = lanes[0].span;
    let (lo_base, hi_base) = (s0.min(s1), s0.max(s1));
    let mut samples: Vec<f64> = base_arm.curve.stretches.iter().flat_map(|(_, ps)| ps.iter().map(|q| q.s)).collect();
    for l in lanes.iter().skip(1) {
        let (lo, hi) = (l.span[0].min(l.span[1]), l.span[0].max(l.span[1]));
        // The foot on the base of each vertex of the lane: where the base crosses the lane's normal there.
        for q in arms[l.arm].curve.stretches.iter().flat_map(|(_, ps)| ps.iter()).filter(|q| q.s > lo && q.s < hi) {
            if let Some((s, _)) = crossing(base_arm, q.p, left_of(arms[l.arm].curve.dir(q.s)), s0) {
                samples.push(s);
            }
        }
    }
    samples.retain(|&s| s > lo_base && s < hi_base);
    samples.sort_by(f64::total_cmp);
    let near_of = |s: f64| -> Vec<f64> {
        let u = (s - s0) / (s1 - s0);
        lanes.iter().map(|l| l.span[0] + (l.span[1] - l.span[0]) * u).collect()
    };
    let (start, stop) = (end_rows[0].0, end_rows[1].0);
    let mut points: Vec<[f64; 2]> = vec![start];
    for &s in &samples {
        let (q, d) = (base_arm.curve.at(s).0.p, base_arm.curve.dir(s));
        let Some(places) = across(q, left_of(d), d, &near_of(s)) else { continue };
        let c = offset(q, left_of(d), middle(&places));
        let spaced = distance(points[points.len() - 1], c) >= b.vertex_gap_m;
        if spaced && distance(start, c) >= clear[0] && distance(c, stop) >= clear[1] {
            points.push(c);
        }
    }
    points.push(stop);
    // The section at each vertex, on the ring there: the line at an end, else the mitre of the two chords.
    let n = points.len();
    let mut table: Vec<Vec<(f64, f64, f64)>> = Vec::with_capacity(n);
    let (mut len, mut metres_at) = (0.0, Vec::with_capacity(n));
    for i in 0..n {
        if i > 0 {
            len += distance(points[i - 1], points[i]);
        }
        metres_at.push(len);
        let row = if i == 0 || i + 1 == n {
            end_rows[if i == 0 { 0 } else { 1 }].2.clone()
        } else {
            let unit = |a: [f64; 2], b: [f64; 2]| {
                let d = [b[0] - a[0], b[1] - a[1]];
                let l = d[0].hypot(d[1]);
                [d[0] / l, d[1] / l]
            };
            let (din, dout) = (unit(points[i - 1], points[i]), unit(points[i], points[i + 1]));
            let sum = [din[0] + dout[0], din[1] + dout[1]];
            let l = sum[0].hypot(sum[1]);
            let m = [sum[0] / l, sum[1] / l];
            let cos = ((m[0] * din[0] + m[1] * din[1] + m[0] * dout[0] + m[1] * dout[1]) / 2.0).max(1.0 / mitre);
            let near = table[i - 1].iter().map(|p| p.2).collect::<Vec<f64>>();
            match across(points[i], left_of(m), m, &near) {
                Some(places) => places.into_iter().map(|(t, h, s)| (t * cos, h * cos, s)).collect(),
                None => table[i - 1].clone(),
            }
        };
        table.push(row);
    }
    // The lanes from the right wall to the left, by their places at the first end.
    let mut order: Vec<usize> = (0..lanes.len()).collect();
    order.sort_by(|&x, &y| table[0][x].0.total_cmp(&table[0][y].0));
    let ordered: Vec<Lane> = order.iter().map(|&k| lanes[k].clone()).collect();
    let table: Vec<(f64, Vec<Place>)> = (metres_at.iter().zip(&table))
        .map(|(&m, row)| (m, order.iter().map(|&k| Place { off: row[k].0, half: row[k].1, s: row[k].2 }).collect()))
        .collect();
    // Behind a seam the room starts as wide as its hole; its half-width is what its lanes need elsewhere.
    let rows = table
        .iter()
        .enumerate()
        .filter(|&(i, _)| !(i == 0 && starts[0].is_some() || i + 1 == n && starts[1].is_some()));
    let half = rows.flat_map(|(_, (_, row))| row.iter().map(|p| p.off.abs() + p.half)).fold(0.0, f64::max);
    // The tube of the room: its centre line at the least height of its lanes, as wide as they need and at its ends as
    // high as their outlines need at the heights of the road solver, the height that its openings read.
    let design = &obc_trailview_parts::Rules::embedded().design;
    let segments = design.arch_segments * lanes.len();
    let mut height: f64 = 0.0;
    let mut curve_points = Vec::with_capacity(n);
    for (i, (m, row)) in table.iter().enumerate() {
        let z: Vec<f64> = (ordered.iter().zip(row)).map(|(l, p)| arms[l.arm].curve.at(p.s).0.z).collect();
        let low = z.iter().copied().fold(f64::INFINITY, f64::min);
        let outlines: Vec<Outline> = (ordered.iter().zip(row).zip(&z))
            .map(|((l, p), &zl)| (p.off, p.half, zl - low, arms[l.arm].height, design.arch_segments))
            .collect();
        if i == 0 || i + 1 == n {
            let wall = (outlines[0].2).max(outlines[outlines.len() - 1].2) + design.kerb_m;
            height = height.max(hold(true, half, design.spring, &outlines, segments, wall));
        }
        let along = arms[ordered[0].arm].curve.at(row[0].s).0.along;
        curve_points.push(Point { p: points[i], z: low, s: *m, along });
    }
    let mut ids: Vec<u32> = lanes.iter().flat_map(|l| arms[l.arm].ids.iter().copied()).collect();
    ids.sort_unstable();
    ids.dedup();
    let tube = Tube {
        curve: Curve { stretches: vec![(base_arm.widest, curve_points)], len },
        ids,
        heights: true,
        half,
        height,
        ..base_arm.clone()
    };
    let room_opening = |end: usize| {
        let far = &openings[fars[end]?];
        let (s, p) = if end == 0 { (0.0, start) } else { (len, stop) };
        ends_open[end].then(|| Opening { shift: 0.0, s, p, floor: tube.curve.at(s).0.z, ..far.clone() })
    };
    let ends = [room_opening(0), room_opening(1)];
    let behind = [0, 1].map(|end| starts[end].map(|_| clear[end]));
    (tube, ordered, table, ends, moves, off_line, behind)
}
