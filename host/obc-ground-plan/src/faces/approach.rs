//! The widths of the ground lines at a built tunnel opening (`WayGraph::mouths`): a forecourt's, and the lines that
//! widen at an opening whose front is not clear.

use super::geom::inner_point;
use super::stroke::{self, Rings};
use super::{Ctx, WayLine, SHOULDER, VERGE};
use crate::frame::metres_per_unit;
use crate::graph::{Forecourt, Level, MIN_STEP_M};

/// An end of a line at the end vertex of a mouth's way: the end (0 first, 1 last), the change of the offset of each
/// band from the shoulder out at the opening (units), whether the line runs from the end into the opening, the
/// opening's distance from the end along the way (units), and the length over which the change narrows to none (units).
struct End {
    k: usize,
    wider: f64,
    ahead: bool,
    shift: f64,
    taper: f64,
}

impl End {
    /// The share of the change at `s` units from the end: all of it at the opening and past it.
    fn share(&self, s: f64) -> f64 {
        let off = if self.ahead { (self.shift - s).max(0.0) } else { self.shift + s };
        (1.0 - off / self.taper).max(0.0)
    }

    /// The distance from the end where the change starts, where it starts on the line.
    fn start(&self) -> Option<f64> {
        let s = if self.ahead { self.shift - self.taper } else { self.taper - self.shift };
        (s > 0.0).then_some(s)
    }
}

/// Gives each forecourt line its widths, and widens the lines at the mouths of the graph whose front is not clear: the
/// line that runs from the end vertex of the opening's way into the opening has the approach's width from the opening
/// on, and the approach narrows back to the way's own width over `approach_m` from the opening, along that line and on
/// along each line that leaves the vertex, or over that line where it is shorter. Its core widens to the farther of the
/// jamb lines, measured from the opening's point, so its walls or banks stand on them and meet the head wall.
pub(super) fn widen(ctx: &Ctx, lines: &mut [WayLine]) {
    if ctx.graph.mouths.is_empty() {
        return;
    }
    let design = obc_trailview_parts::Rules::embedded().design;
    for line in lines.iter_mut().filter(|l| l.p.len() >= 2) {
        if let Some(f) = ctx.graph.forecourts.iter().find(|f| f.way == line.way) {
            forecourt(ctx, line, f);
            continue;
        }
        let n = line.p.len();
        let mut ends: Vec<End> = Vec::new();
        for (k, at) in [0, n - 1].into_iter().enumerate() {
            let g = ctx.global(line.p[at]);
            let Some(m) = ctx.graph.mouths.iter().find(|m| [m.way.x, m.way.y] == g && !m.clear) else { continue };
            // The tunnel way's own ground stretch runs from the vertex into the opening; every other line leaves it.
            let inward = ctx.graph.parts[ctx.graph.ways[line.way as usize].part as usize].level == Level::Tunnel;
            // The vertex's metres to the left of the opening's point: the jamb lines about the line's axis.
            let left = [m.into[1], -m.into[0]];
            let d = ((g[0] as f64 - m.at[0]) * left[0] + (g[1] as f64 - m.at[1]) * left[1]) * metres_per_unit(g[1]);
            let reach = (m.jamb_m[0] - d).abs().max((m.jamb_m[1] - d).abs());
            let u = |metres: f64| ctx.units(metres, line.p[at]);
            let wider = (u(reach) - line.bands[at][VERGE]).max(0.0);
            let (shift, approach) = (u(m.shift_m), u(design.approach_m));
            let taper = if inward { approach } else { approach.min(shift + length(&line.p)) };
            if (inward || taper > shift) && wider > 0.0 {
                ends.push(End { k, wider, ahead: inward, shift, taper });
            }
        }
        if ends.is_empty() {
            continue;
        }
        // A vertex at the opening and where each change starts, unless one lies near.
        let gap = ctx.units(MIN_STEP_M / 2.0, line.p[0]);
        for e in &ends {
            for at in [e.ahead.then_some(e.shift), e.start()].into_iter().flatten().filter(|&s| s > 0.0) {
                let last = line.p.len() - 1;
                let s = along(&line.p, if e.k == 0 { 0 } else { last });
                let Some(i) = (0..last).find(|&i| (s[i] - at) * (s[i + 1] - at) < 0.0) else { continue };
                let x = (at - s[i]) / (s[i + 1] - s[i]);
                if (s[i + 1] - s[i]) * x.min(1.0 - x) >= gap {
                    insert(line, i, x);
                }
            }
        }
        // Each band's offset changes by the change of the end with the largest share there; the layers stay in their
        // order out of the carriageway.
        let n = line.p.len();
        let s: Vec<Vec<f64>> = ends.iter().map(|e| along(&line.p, if e.k == 0 { 0 } else { n - 1 })).collect();
        for i in 0..n {
            let Some((e, f)) =
                (ends.iter().zip(&s)).map(|(e, s)| (e, e.share(s[i]))).max_by(|a, b| a.1.total_cmp(&b.1))
            else {
                continue;
            };
            let mut inner = line.h[i];
            for (j, b) in line.bands[i].iter_mut().enumerate() {
                *b = (*b + if j >= SHOULDER { e.wider * f } else { 0.0 }).max(inner);
                inner = *b;
            }
        }
    }
}

/// A forecourt's line: the approach, the painter's first record, square on the frame line, its carriageway and core
/// `half_m` (the reveal, the jamb lines grown by `frame_m`) up to the front of the head wall and narrowing from there
/// to none at its far end, its slopes their widths out of the core. So the ground before the whole reveal is its
/// surface, the room's floor at the frame line. Its sides cut the room's own embankment, not rock: they are banks, as
/// a path's, never walls (no `leg_wall`). A forecourt whose paint its own line frames wears no lines: that line's
/// half-width narrows with its sides, so lines framed by it would run across the apron.
fn forecourt(ctx: &Ctx, line: &mut WayLine, f: &Forecourt) {
    let half_m = f.half_m;
    line.sec.leg_wall = false;
    line.sec.marked &= f.frames != [f.way];
    let s = along(&line.p, 0);
    let (front, len) = (s[line.index[1]], s[s.len() - 1]);
    for (i, &si) in s.iter().enumerate() {
        let core = ctx.units(half_m, line.p[i]) * ((len - si) / (len - front)).min(1.0);
        let by = core - line.bands[i][VERGE];
        line.h[i] = core;
        for (j, b) in line.bands[i].iter_mut().enumerate() {
            *b = if j <= VERGE { core } else { *b + by };
        }
    }
    line.round[0] = false;
    line.approach = true;
    // The recess is a leg of its own, a face of its own (`legs`) whose edge lies on the front of the head wall, so the
    // drawn ground over the whole recess is the floor, which the opening's box keeps (vertex 1 lies there,
    // `graph::forecourt`). Its record runs on over the whole forecourt, as a leg's past its cut: past the head it is
    // the forecourt's own surface, not the floor carried on at the recess's grade.
    line.cuts = vec![line.index[1]];
    line.spans = vec![(0, line.p.len())];
}

/// A forecourt's carriageway `rings` per leg: the mitre line of each cut splits it across its whole width, and each
/// piece takes the leg on its side. So no face, and no triangle of the near mesh, spans a cut: the medial lines of
/// `faces::legs` are traced and reach only the class's width, so they need not split the widened carriageway there.
pub(super) fn legs(line: &WayLine, rings: &Rings) -> Vec<(u8, Rings)> {
    let unit = |a: [i32; 2], b: [i32; 2]| {
        let v = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])];
        let n = libm::hypot(v[0], v[1]).max(1e-9);
        [v[0] / n, v[1] / n]
    };
    // Per cut its point and the line's direction there, the bisector of its two chords.
    let mitres: Vec<([i32; 2], [f64; 2])> = (line.cuts.iter())
        .map(|&k| {
            let (a, b) = (unit(line.p[k - 1], line.p[k]), unit(line.p[k], line.p[k + 1]));
            let n = libm::hypot(a[0] + b[0], a[1] + b[1]).max(1e-9);
            (line.p[k], [(a[0] + b[0]) / n, (a[1] + b[1]) / n])
        })
        .collect();
    let lines: Vec<[[i32; 2]; 2]> = (line.cuts.iter().zip(&mitres))
        .map(|(&k, &(o, t))| {
            let w = 2.0 * line.h[k];
            let end = |s: f64| [0, 1].map(|c| o[c] + libm::round(s * w * [-t[1], t[0]][c]) as i32);
            [end(-1.0), end(1.0)]
        })
        .collect();
    let mut out: Vec<(u8, Rings)> = Vec::new();
    for piece in stroke::cut(rings, &lines, line.p[0]) {
        let Some(q) = inner_point(&piece) else { continue };
        // The cuts the piece lies past (`inner_point` is in half units).
        let past = |&(o, t): &([i32; 2], [f64; 2])| {
            (q[0] as f64 / 2.0 - f64::from(o[0])) * t[0] + (q[1] as f64 / 2.0 - f64::from(o[1])) * t[1] > 0.0
        };
        let leg = mitres.iter().filter(|m| past(m)).count() as u8;
        match out.iter_mut().find(|(l, _)| *l == leg) {
            Some((_, r)) => r.extend(piece),
            None => out.push((leg, piece)),
        }
    }
    out.into_iter().map(|(leg, r)| (leg, stroke::union(&r, line.p[0]))).collect()
}

/// The length of a line (units).
fn length(p: &[[i32; 2]]) -> f64 {
    along(p, 0)[p.len() - 1]
}

/// The distance of each vertex from vertex `at` along the line (units).
fn along(p: &[[i32; 2]], at: usize) -> Vec<f64> {
    let step = |a: [i32; 2], b: [i32; 2]| libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1]));
    let mut s = vec![0.0; p.len()];
    for i in at + 1..p.len() {
        s[i] = s[i - 1] + step(p[i - 1], p[i]);
    }
    for i in (0..at).rev() {
        s[i] = s[i + 1] + step(p[i], p[i + 1]);
    }
    s
}

/// A vertex at share x of segment i, with the segment's offsets there and its first vertex's key and walk.
fn insert(line: &mut WayLine, i: usize, x: f64) {
    let (a, b) = (line.p[i], line.p[i + 1]);
    let q = [0, 1].map(|c| a[c] + libm::round(x * f64::from(b[c] - a[c])) as i32);
    let mix = |u: f64, v: f64| u + x * (v - u);
    let bands = std::array::from_fn(|k| mix(line.bands[i][k], line.bands[i + 1][k]));
    line.p.insert(i + 1, q);
    line.h.insert(i + 1, mix(line.h[i], line.h[i + 1]));
    line.bands.insert(i + 1, bands);
    line.key.insert(i + 1, line.key[i]);
    line.walk.insert(i + 1, line.walk[i]);
    let spans = line.spans.iter_mut().flat_map(|(a, b)| [a, b]);
    for k in line.index.iter_mut().chain(line.cuts.iter_mut()).chain(spans).filter(|k| **k > i) {
        *k += 1;
    }
}
