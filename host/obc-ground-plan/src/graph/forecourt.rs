//! The forecourt of a built room with a clear front (`openings::Mouth::clear`): a ground way of its own before the
//! whole reveal, so the ground there is the room's, wherever its ways turn.

use std::collections::BTreeMap;

use super::{Arm, End, Level, Part, PointRef, Sample, SampleKind, Span, Station, Vertex, Way, WayGraph, ROOM_MAX_M};
use crate::frame::{metres_per_unit, GlobalPoint};
use crate::openings::Mouth;

/// The forecourt way of a room, the half-width of its front (metres): half the room's reveal and the frame, and the
/// ways whose frames its paint follows: its lanes' ground ways, so their lines run on over it, else its own line, and
/// it then wears no lines (`faces::approach`).
#[derive(Clone, Debug, PartialEq)]
pub struct Forecourt {
    pub way: u32,
    pub half_m: f64,
    pub frames: Vec<u32>,
}

/// The ways of a lane (`lane`) at its vertex (cell units): its tunnel way, that way's open stretch from the vertex to
/// the opening, and the arm that carries the lane on outward from the vertex.
#[derive(Default)]
struct Lane {
    vertex: [i32; 2],
    tunnel: Option<u32>,
    open: Option<Arm>,
    on: Option<Arm>,
}

impl Lane {
    /// Its first ground way out from the opening.
    fn first(&self) -> Option<u32> {
        self.open.as_ref().or(self.on.as_ref()).map(|a| a.way)
    }

    /// The way that carries it on outward.
    fn on(&self) -> Option<u32> {
        self.on.as_ref().map(|a| a.way)
    }
}

/// Segments of a forecourt from the front of the head wall to its far end.
const SEGMENTS: usize = 4;

/// Adds the forecourt of each room with a clear front whose front lies in the cell grown by its margin, and whose ways
/// the graph holds at their openings. It runs straight from the middle of the reveal on the frame line out along the
/// room, to the front of the head wall (`recess_m`: the opening's box drops the ground over the floor there) and on for
/// the reveal's half-width, so its sides, which close in from the reveal's edges there (`faces::approach`), stay inside
/// the head's width. Its height is the room's floor up to its first vertex past the head's front, and outward the
/// lowest lane profile at as many metres from the opening, so a ray from inside that leaves the head over the floor
/// meets the ground from above. Where its lanes have no ground way (their ways end at the opening, end on a T or go on
/// as a bridge), it is the tunnel way's own ground: its look without lines, its own line as its frame, and the floor
/// run on straight from the frame line.
pub(crate) fn add(g: &mut WayGraph) {
    let design = obc_trailview_parts::Rules::embedded().design;
    let mut rooms: BTreeMap<usize, Vec<Mouth>> = BTreeMap::new();
    for m in g.mouths.iter().filter(|m| m.clear) {
        rooms.entry(m.room).or_default().push(*m);
    }
    let [x0, y0, x1, y1] = g.cell.grown();
    for lanes in rooms.values() {
        let m0 = &lanes[0];
        let mpu = metres_per_unit(m0.at[1] as i64);
        let left = [m0.into[1], -m0.into[0]];
        // The reveal: the span of the lanes' jamb lines, in metres to the left of the first lane's point.
        let off = |m: &Mouth| ((m.at[0] - m0.at[0]) * left[0] + (m.at[1] - m0.at[1]) * left[1]) * mpu;
        let lo = lanes.iter().map(|m| off(m) + m.jamb_m[0]).fold(f64::INFINITY, f64::min);
        let hi = lanes.iter().map(|m| off(m) + m.jamb_m[1]).fold(f64::NEG_INFINITY, f64::max);
        let mid = (lo + hi) / 2.0;
        let front = [m0.at[0] + left[0] * mid / mpu, m0.at[1] + left[1] * mid / mpu];
        if !(x0 as f64..=x1 as f64).contains(&front[0]) || !(y0 as f64..=y1 as f64).contains(&front[1]) {
            continue;
        }
        let half = (hi - lo) / 2.0 + design.frame_m;
        let len = design.recess_m + half;
        let ground: Vec<Lane> = lanes.iter().map(|m| lane(g, m)).collect();
        let per_lane: Vec<Vec<(f64, f64)>> =
            ground.iter().map(|l| profile(g, l, len)).filter(|p| !p.is_empty()).collect();
        // Of the lanes nearest the middle, the first by their global owner: of their first ground ways (else their
        // tunnel ways), the host, whose owner the forecourt keeps, so its records relate to the lanes' as the room's
        // ground; of the ways that carry them on outward (else the host), the source of its surface, as the eye sees it
        // outside.
        let near = lanes.iter().map(|m| (off(m) - mid).abs()).fold(f64::INFINITY, f64::min);
        let nearest = || lanes.iter().zip(&ground).filter(|(m, _)| (off(m) - mid).abs() == near).map(|x| x.1);
        let owner = |w: u32| {
            let way = &g.ways[w as usize];
            g.parts[way.part as usize].spans[g.samples[way.samples.start as usize].span as usize].owner
        };
        let pick = |way: fn(&Lane) -> Option<u32>| nearest().filter_map(way).min_by_key(|&w| owner(w));
        // None only where the graph holds no tunnel arm at those lanes' vertices: they lie past its area
        // (`work_area`, `MARGIN_M` beyond the cell), and the front at most their shift nearer. This cell then builds no
        // forecourt there; it reaches the cell only where a shift exceeds `MARGIN_M` less its length and feather.
        let Some(host) = pick(Lane::first).or_else(|| pick(|l| l.tunnel)) else {
            continue;
        };
        let source = pick(Lane::on).unwrap_or(host);
        // A label holds four host lines: each lane's first ground way, then the ways that carry them on, the first
        // four, else its own line. So every lane of a room of up to four keeps its frame, and of up to two all of its
        // ground ways.
        let id = g.ways.len() as u32;
        let mut frames: Vec<u32> = Vec::new();
        for w in ground.iter().filter_map(Lane::first).chain(ground.iter().filter_map(Lane::on)) {
            if !frames.contains(&w) && frames.len() < 4 {
                frames.push(w);
            }
        }
        if frames.is_empty() {
            frames.push(id);
        }
        let lowest = |d: f64| per_lane.iter().map(|p| at(p, d)).fold(f64::INFINITY, f64::min);
        let out = [-m0.into[0], -m0.into[1]];
        let d: Vec<f64> = [0.0]
            .into_iter()
            .chain((0..=SEGMENTS).map(|k| design.recess_m + half * k as f64 / SEGMENTS as f64))
            .collect();
        // The floor up to the front of the head wall, and on to the next vertex: the grade at the head's front, the
        // slope between its two neighbours, is then a share of the floor's. A grade to the lanes' height there would
        // lift the ground in the recess, and the lanes' faces at the frame line, over the floor + lift that the
        // opening's box keeps.
        let grade = (m0.floor_m[1] - m0.floor_m[0]) / design.recess_m;
        let z: Vec<f64> = (d.iter().enumerate())
            .map(|(k, &x)| match k {
                0 | 1 => m0.floor_m[k],
                _ if per_lane.is_empty() => m0.floor_m[0] + grade * x,
                2 => m0.floor_m[1],
                _ => lowest(x),
            })
            .collect();
        let n = d.len() - 1;
        let tangent = libm::atan2(out[1], out[0]);
        let vertices: Vec<Vertex> = (0..=n)
            .map(|k| {
                let (a, b) = (k.saturating_sub(1), (k + 1).min(n));
                Vertex {
                    pos: [0, 1].map(|c| libm::round(front[c] + out[c] * d[k] / mpu) as i64),
                    z_m: z[k],
                    tangent,
                    grade: (z[b] - z[a]) / (d[b] - d[a]),
                    bank: 0.0,
                    widen_m: 0.0,
                    walk: 0,
                    side: 0,
                }
            })
            .collect();
        let src = &g.parts[g.ways[source as usize].part as usize];
        let spans: Vec<Span> = (0..n)
            .map(|k| Span { owner: owner(host), ..Span::new(&vertices[k], &vertices[k + 1], true, d[k]) })
            .collect();
        let o = g.cell.origin();
        let part = g.parts.len() as u32;
        let samples: Vec<Sample> = (vertices.iter().zip(&d))
            .enumerate()
            .map(|(k, (v, &along_m))| Sample {
                pos: [(v.pos[0] - o.x) as i32, (v.pos[1] - o.y) as i32],
                span: k.min(n - 1) as u32,
                u: if k == n { 1.0 } else { 0.0 },
                s_m: if k == n { spans[n - 1].len_m } else { 0.0 },
                along_m,
                z_m: v.z_m,
                tangent: out,
                grade: v.grade,
                bank: 0.0,
                widen_m: 0.0,
                walk: 0,
                kind: SampleKind::Vertex,
            })
            .collect();
        let (from, to) = (Station { part, span: 0, u: 0.0 }, Station { part, span: n as u32 - 1, u: 1.0 });
        let first = g.samples.len() as u32;
        let ends = g.ends.len() as u32;
        for (k, (at, dir)) in [(from, 1i8), (to, -1i8)].into_iter().enumerate() {
            let heading = [out[0] * f64::from(dir), out[1] * f64::from(dir)];
            let pos = samples[if k == 0 { 0 } else { n }].pos;
            let arm = Arm {
                way: id,
                at,
                dir,
                heading,
                angle: libm::atan2(heading[1], heading[0]),
                host: false,
                level: Level::Ground,
                room_m: len.min(ROOM_MAX_M),
                next: Some(PointRef::End(ends + 1 - k as u32)),
            };
            let global = GlobalPoint { x: o.x + i64::from(pos[0]), y: o.y + i64::from(pos[1]) };
            g.ends.push(End { pos, network: src.family.network(), arm, owned: g.cell.contains(global) });
        }
        g.ways.push(Way {
            id,
            part,
            from,
            to,
            family: src.family,
            class: src.class,
            level: Level::Ground,
            deck: None,
            link: None,
            samples: first..first + samples.len() as u32,
        });
        g.parts.push(Part {
            feature: src.feature,
            family: src.family,
            class: src.class,
            level: Level::Ground,
            backward: false,
            vertices,
            spans,
        });
        g.samples.extend(samples);
        g.forecourts.push(Forecourt { way: id, half_m: half, frames });
    }
}

/// The ground ways of lane `m` at its vertex. Its tunnel way's arm there is the arm of a tunnel part that leaves the
/// vertex nearest the way into the tube; it is the open stretch where the opening lies past the vertex. Only the arms
/// of the point that holds it count: another network's point at that position is no part of the lane. The arm that
/// the vertex pairs with it carries the lane on outward, if that is a ground way of a ground part: the other arm of a
/// joint, the other host of a junction where it is one of two (the through road), else the arm that continues it
/// straightest of those that the vertex pairs with no other arm. A junction's two hosts are a pair, and so are the two
/// arms of a way that runs on through the vertex: where the lane ends on such a pair (a T), no way carries it on.
fn lane(g: &WayGraph, m: &Mouth) -> Lane {
    let o = g.cell.origin();
    let vertex = [(m.way.x - o.x) as i32, (m.way.y - o.y) as i32];
    let joints = g.joints.iter().filter(|j| j.pos == vertex).map(|j| &j.arms[..]);
    let junctions = g.junctions.iter().filter(|j| j.pos == vertex).map(|j| &j.arms[..]);
    let ends = g.ends.iter().filter(|e| e.pos == vertex).map(|e| std::slice::from_ref(&e.arm));
    let part = |a: &Arm| g.parts[g.ways[a.way as usize].part as usize].level;
    let dot = |u: [f64; 2], v: [f64; 2]| u[0] * v[0] + u[1] * v[1];
    let points = joints.chain(junctions).chain(ends);
    let tunnel = points.flat_map(|arms| arms.iter().map(move |a| (arms, a))).filter(|&(_, a)| part(a) == Level::Tunnel);
    let Some((arms, t)) = tunnel.max_by(|(_, a), (_, b)| dot(a.heading, m.into).total_cmp(&dot(b.heading, m.into)))
    else {
        return Lane { vertex, ..Lane::default() };
    };
    let others = || arms.iter().filter(|a| !std::ptr::eq(*a, t));
    let through = arms.iter().filter(|a| a.host).count() == 2;
    let paired = |a: &&Arm| (through && a.host) || others().any(|b| b.way == a.way && b.at == a.at && b.dir != a.dir);
    let straight = |a: &&Arm| -dot(a.heading, t.heading);
    let pair = match through && t.host {
        true => others().find(|a| a.host),
        false => others().filter(|a| !paired(a)).max_by(|a, b| straight(a).total_cmp(&straight(b))),
    };
    Lane {
        vertex,
        tunnel: Some(t.way),
        open: (m.shift_m > 0.0 && t.level == Level::Ground).then(|| t.clone()),
        on: pair.filter(|a| a.level == Level::Ground && part(a) != Level::Tunnel).cloned(),
    }
}

/// A lane's heights out from its opening: (metres, height) along its open stretch to the vertex and on along the way
/// that carries it on, for `max_m` metres past the vertex.
fn profile(g: &WayGraph, lane: &Lane, max_m: f64) -> Vec<(f64, f64)> {
    let mut points: Vec<([i32; 2], f64)> = Vec::new();
    if let Some(a) = &lane.open {
        let r = g.ways[a.way as usize].samples.clone();
        let point = |k: u32| (g.samples[k as usize].pos, g.samples[k as usize].z_m);
        // The arm leaves the vertex along its way: the opening is the way's other end.
        match a.dir > 0 {
            true => points.extend(r.rev().map(point)),
            false => points.extend(r.map(point)),
        }
    }
    if let Some(a) = &lane.on {
        let walk = g.walk(a.way, g.sample_near(a.way, lane.vertex), a.dir, max_m, false).swap_remove(0);
        points.extend(walk.iter().skip(usize::from(!points.is_empty())).map(|p| (p.pos, p.z_m)));
    }
    let mpu = |a: [i32; 2], b: [i32; 2]| metres_per_unit(g.cell.origin().y + i64::from(a[1] + b[1]) / 2);
    let mut s = 0.0;
    let mut out: Vec<(f64, f64)> = points.first().map(|p| (0.0, p.1)).into_iter().collect();
    for p in points.windows(2) {
        let (a, b) = (p[0].0, p[1].0);
        s += libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])) * mpu(a, b);
        out.push((s, p[1].1));
    }
    out
}

/// The height of a profile at `d` metres: linear between its points, its last past its end.
fn at(p: &[(f64, f64)], d: f64) -> f64 {
    let k = p.partition_point(|x| x.0 <= d);
    if k == 0 || k == p.len() {
        return p[k.min(p.len() - 1)].1;
    }
    let ((s0, z0), (s1, z1)) = (p[k - 1], p[k]);
    if s1 <= s0 {
        return z1;
    }
    z0 + (d - s0) / (s1 - s0) * (z1 - z0)
}
