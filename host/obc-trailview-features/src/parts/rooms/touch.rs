//! The check of the rooms (Rooms in the spec): two rooms that draw and still come nearer than a wall on their final
//! floors, halves and heights.

use std::collections::BTreeMap;

use super::super::floor::Floor;
use super::super::{along_dir, left_of, offset, segment_m, Curve, Input, Point, Tube};
use super::axes::{Axes, Foot};
use super::{Rooms, Seam, MERGE_M};

/// Two spaces lie apart: their gap is a wall or more, or one lies clear over the other, a crown's shell and a slab over
/// its crown, each within the bake's centimetre; `rise` is the height of the second's floor over the first's.
fn apart((gap, rise): (f64, f64), (ha, hb): (f64, f64), input: &Input) -> bool {
    let b = &input.rules.bake;
    let (wall, roof) = (2.0 * b.shell_m, b.shell_m + b.slab_m);
    gap >= wall - MERGE_M || rise >= ha + roof - MERGE_M || -rise >= hb + roof - MERGE_M
}

/// A section of a room: its two ends.
type Section = [[f64; 2]; 2];

/// The chord of a room at a seam as the builder draws it: from the room's outline on the seam's line (its hole, or of
/// the room before the line its own half-width) to its ring at the next vertex, the edges straight; and its metres.
#[derive(Clone, Copy)]
pub(super) struct EndChord {
    line: Section,
    ring: Section,
    len: f64,
}

/// The chord of tube t at its end e on a seam's line of direction `dir`, its outline there `half` to each side.
pub(super) fn end_chord(t: &Tube, e: usize, (dir, half): ([f64; 2], f64), mitre: f64) -> Option<EndChord> {
    let ps = &t.curve.stretches.first()?.1;
    let n = ps.len();
    let at = |k: usize| if e == 0 { ps[k] } else { ps[n - 1 - k] };
    let unit = |a: [f64; 2], b: [f64; 2]| {
        let d = [b[0] - a[0], b[1] - a[1]];
        let l = d[0].hypot(d[1]).max(f64::MIN_POSITIVE);
        [d[0] / l, d[1] / l]
    };
    let (v0, v1) = (at(0), at(1));
    let din = unit(v0.p, v1.p);
    // The ring in the plane between the two chords, its width grown by the bend up to `mitre_max`, as the builder's.
    let (m, scale) = match (n > 2).then(|| unit(v1.p, at(2).p)) {
        Some(dout) if (din[0] + dout[0]).hypot(din[1] + dout[1]) > 1e-9 => {
            let m = unit([0.0, 0.0], [din[0] + dout[0], din[1] + dout[1]]);
            let cos = (m[0] * din[0] + m[1] * din[1] + m[0] * dout[0] + m[1] * dout[1]) / 2.0;
            (m, 1.0 / cos.max(1.0 / mitre))
        }
        _ => (din, 1.0),
    };
    let side = |c: [f64; 2], l: [f64; 2], h: f64| [offset(c, l, -h), offset(c, l, h)];
    let ring = side(v1.p, left_of(m), t.half * scale);
    Some(EndChord { line: side(v0.p, left_of(dir), half), ring, len: (v1.s - v0.s).abs() })
}

/// The metres from a section that lies behind a seam's line to the space of a chord at that seam: to its outline on
/// the line and its two edges.
pub(super) fn from_end(c: &EndChord, x: Section) -> f64 {
    let edges = [c.line, [c.line[0], c.ring[0]], [c.line[1], c.ring[1]]];
    edges.into_iter().map(|e| segments_m(x, e)).fold(f64::INFINITY, f64::min)
}

/// The rooms that draw as the builder draws them: at their final floors, halves and heights (another room has no axis),
/// with their chords at the seams and their axes.
pub(crate) struct Drawn<'a> {
    pub tubes: Vec<Tube>,
    pub axes: Axes,
    ends: Vec<[Option<EndChord>; 2]>,
    floors: &'a [Option<Floor>],
    lanes: Vec<bool>,
}

impl<'a> Drawn<'a> {
    pub fn new(rooms: &Rooms, (floors, built): (&'a [Option<Floor>], &[bool]), half: &[f64]) -> Drawn<'a> {
        let tubes: Vec<Tube> = (rooms.tubes.iter().zip(floors).zip(half).zip(built))
            .map(|(((t, f), &half), &built)| {
                let mut t = t.clone();
                let Some(f) = f.as_ref().filter(|_| built) else {
                    t.curve.stretches.clear();
                    return t;
                };
                let (s0, s1) = (f.s[0], f.s[f.s.len() - 1]);
                let points: Vec<Point> =
                    f.s.iter().zip(&f.f).map(|(&s, &z)| Point { z, s: s - s0, ..t.curve.at(s).0 }).collect();
                t.curve = Curve { stretches: vec![(t.widest, points)], len: s1 - s0 };
                Tube { half, ..t }
            })
            .collect();
        let mitre = obc_trailview_parts::Rules::embedded().design.mitre_max;
        let mut ends: Vec<[Option<EndChord>; 2]> = vec![[None; 2]; tubes.len()];
        for seam in &rooms.seams {
            let (pr, pe) = seam.parent;
            ends[pr][pe] = end_chord(&tubes[pr], pe, (seam.dir, tubes[pr].half), mitre);
            for (&(r, e), &(_, hole)) in seam.children.iter().zip(&seam.holes) {
                ends[r][e] = end_chord(&tubes[r], e, (seam.dir, hole), mitre);
            }
        }
        let axes = Axes::new(&tubes);
        Drawn { tubes, axes, ends, floors, lanes: rooms.lanes.iter().map(|l| l.len() > 1).collect() }
    }

    /// The chord at a seam that metres s of tube t lie on, and the share along it from the line.
    fn end_chord(&self, t: usize, s: f64) -> Option<(EndChord, f64)> {
        (0..2).find_map(|e| {
            let c = self.ends[t][e]?;
            let m = if e == 0 { s } else { self.tubes[t].curve.len - s };
            (m < c.len).then(|| (c, (m / c.len).max(0.0)))
        })
    }

    /// The section of tube t at metres s: on its chord at a seam between its outline on the line and its ring; else
    /// square to its axis, `half` to each side.
    pub fn at(&self, t: usize, s: f64) -> Section {
        if let Some((c, u)) = self.end_chord(t, s) {
            let mix = |a: [f64; 2], b: [f64; 2]| [a[0] + u * (b[0] - a[0]), a[1] + u * (b[1] - a[1])];
            return [mix(c.line[0], c.ring[0]), mix(c.line[1], c.ring[1])];
        }
        let tube = &self.tubes[t];
        let (p, l) = (tube.curve.at(s).0.p, left_of(tube.curve.dir(s)));
        [offset(p, l, -tube.half), offset(p, l, tube.half)]
    }

    /// A room's height at metres s as the builder draws it: of a room of lanes, between its vertices; of a lone tube,
    /// its chord's.
    pub fn height(&self, t: usize, s: f64) -> f64 {
        self.floors[t].as_ref().map_or(0.0, |f| {
            let s = s + f.s[0];
            let k = f.s.partition_point(|&x| x <= s).clamp(1, f.s.len() - 1);
            if self.lanes[t] {
                f.at(s).1
            } else {
                f.h[k - 1].min(f.h[k])
            }
        })
    }
}

/// The metres between two segments, 0 where they cross.
fn segments_m([a, b]: Section, [c, d]: Section) -> f64 {
    if super::super::measure::meet(a, b, c, d).is_some() {
        return 0.0;
    }
    [segment_m(a, c, d), segment_m(b, c, d), segment_m(c, a, b), segment_m(d, a, b)]
        .into_iter()
        .fold(f64::INFINITY, f64::min)
}

/// The part of a section that lies more than the bake's centimetre behind a seam's line (`sign` 1) or before it (−1).
fn beyond([x, y]: Section, seam: &Seam, sign: f64) -> Option<Section> {
    let [ax, ay] = [x, y].map(|q| sign * along_dir(seam.p, q, seam.dir) - MERGE_M);
    if ax <= 0.0 && ay <= 0.0 {
        return None;
    }
    if ax > 0.0 && ay > 0.0 {
        return Some([x, y]);
    }
    let u = ax / (ax - ay);
    let c = [x[0] + u * (y[0] - x[0]), x[1] + u * (y[1] - x[1])];
    Some(if ax > 0.0 { [x, c] } else { [c, y] })
}

/// Per two rooms that draw (`built`) and come beside each other (not `apart`), the first place where they do: each
/// room's centre line read every metre and at each vertex against the axes near it, from both rooms, on the final
/// floors, halves and heights. The gap is the metres between the spaces (each room's half to each side of its axis);
/// past a room's end, and on a chord at a seam, between the sections as the builder draws them. A room before a seam
/// and a room behind it are open to each other through the hole: of the two only the part of a section on the other's
/// side of the line counts.
pub(crate) fn touching(
    rooms: &Rooms,
    (floors, built): (&[Option<Floor>], &[bool]),
    half: &[f64],
    input: &Input,
) -> Vec<[f64; 2]> {
    let wall = 2.0 * input.rules.bake.shell_m;
    let drawn = Drawn::new(rooms, (floors, built), half);
    let tubes = &drawn.tubes;
    let widest = tubes.iter().map(|t| t.half).fold(0.0, f64::max);
    // The seam that one of two rooms lies before and the other behind, and the side of each: 1 behind, −1 before.
    let across = |a: usize, c: usize| {
        let behind = |s: &Seam, r: usize| s.children.iter().any(|x| x.0 == r);
        let seam = rooms.seams.iter().find(|s| s.parent.0 == a && behind(s, c) || s.parent.0 == c && behind(s, a))?;
        Some((seam, if seam.parent.0 == a { [-1.0, 1.0] } else { [1.0, -1.0] }))
    };
    // The gap from room a at metres s to the room of foot f.
    let gap = |a: usize, s: f64, f: &Foot| {
        let (x, y) = (drawn.at(a, s), drawn.at(f.tube, f.s));
        if let Some((seam, [side_a, side_b])) = across(a, f.tube) {
            let one = beyond(x, seam, side_b).map_or(f64::INFINITY, |x| segments_m(x, y));
            return one.min(beyond(y, seam, side_a).map_or(f64::INFINITY, |y| segments_m(x, y)));
        }
        if f.past.is_some() || drawn.end_chord(a, s).is_some() || drawn.end_chord(f.tube, f.s).is_some() {
            segments_m(x, y)
        } else {
            f.d - tubes[a].half - tubes[f.tube].half
        }
    };
    let mut out: BTreeMap<(usize, usize), [f64; 2]> = BTreeMap::new();
    for (a, t) in tubes.iter().enumerate().filter(|(_, t)| !t.curve.stretches.is_empty()) {
        let steps = t.curve.len.ceil().max(1.0) as usize;
        let vertices = t.curve.stretches.iter().flat_map(|(_, ps)| ps.iter().map(|q| q.s));
        for s in (0..=steps).map(|k| t.curve.len * k as f64 / steps as f64).chain(vertices) {
            let q = t.curve.at(s).0;
            for f in drawn.axes.near(q.p, a, t.half + widest + wall) {
                let pair = (a.min(f.tube), a.max(f.tube));
                if out.contains_key(&pair) {
                    continue;
                }
                if !apart((gap(a, s, &f), f.z - q.z), (drawn.height(a, s), drawn.height(f.tube, f.s)), input) {
                    out.insert(pair, q.p);
                }
            }
        }
    }
    out.into_values().collect()
}
