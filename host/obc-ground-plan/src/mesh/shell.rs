//! The meshes of the storeys over the ground: each storey's sheet as a constrained triangulation of its faces alone
//! (no ground ring, no strip, no grid point), and the shell about it, each part a closed solid of a real thickness:
//! the underside (the top again, turned over, at the slab's depth, with a riser where two depths meet), the edge
//! faces on the free outline, a parapet or a railing on it, and the piers. A corner is a point with a label of the
//! cell's block and a height offset in metres: its height is the label's height there plus the offset.
//!
//! The free outline is every face edge between the sheet and the rest that lies on no cell line and on no end line
//! where storey 0 meets the deck (`EdgeKind::DeckEdge`): those stay open, as the neighbour cell or storey 0 goes on
//! there. Each part is cut by the cell square; its faces on a cell line stay open as well. The face edges split
//! into equal pieces no longer than the `l_max_m` of their sheet faces, as the cell mesh's, a rule that reads only the
//! edge: two cells, and storey 0 on an end line by the same rule, give it the same points.
//!
//! Every triangle turns counter-clockwise seen from outside its solid (x east, y north, up), and every slot turns as
//! the cell mesh's: its first two corners are those of the face left of its edge.
//!
//! # Bytes (`StoreyMesh::to_bytes`), little-endian u32 words
//!
//! | Words | Holds |
//! | --- | --- |
//! | head | `HEAD` words: `MAGIC`, the cell's x and y (zoom 16), the counts of corners, triangles, triangle runs, slots, slot runs and of the tops' slots, then the triangles of each part in the order of `PARTS` |
//! | corners | 3 words each, sorted by point (y, then x): x in bits 0–18 and the label's bits 0–12 in bits 19–31; y in bits 0–18 and the label's bits 13–24 in bits 19–30; the offset (f32, metres) |
//! | triangle runs, triangles | as the cell mesh's: 2 words a run (count, base corner), 3 u16 a triangle less its run's base; each part in runs of its own |
//! | slot runs, slots | 2 words a run, 4 u16 a slot (`a_p, a_q, b_q, b_p`): the tops' slots between two labels, then the undersides' risers |

use std::collections::BTreeMap;

use super::cdt::{self, Segment};
use super::params::l_max_m;
use super::{by_least, face_labels, packed, runs, MeshError, NO_LABEL};
use crate::faces::records::{self, find, LINE, SAMPLE};
use crate::faces::{EdgeKind, FacePlan, Storeys, Surface, OUTSIDE};
use crate::frame::{metres_per_unit_at, CellKey, CELL_UNITS};
use crate::graph::Structures;

/// "OBS" and the layout's version.
pub const MAGIC: u32 = 0x4f42_5301;
/// The parts of a storey's mesh, in the order of their triangles.
pub const PARTS: [&str; 7] = ["top", "underside", "edge", "parapet", "railing", "post", "pier"];
const TOP: usize = 0;
const UNDER: usize = 1;
const EDGE: usize = 2;
const PARAPET: usize = 3;
const RAILING: usize = 4;
const POST: usize = 5;
const PIER: usize = 6;
pub const HEAD: usize = 9 + PARTS.len();

/// A parapet: its width and its height over the deck (the crossing block's thickness table).
const PARAPET_M: [f64; 2] = [0.35, 1.0];
/// A railing: its height, and the side of its square rail and of its posts (the crossing block's thickness table).
const RAIL_M: f64 = 1.05;
const BAR_M: f64 = 0.08;

type P = [i32; 2];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ShellStats {
    pub piers: u32,
    /// Piers whose foot stands in a paved face of storey 0 or on a lower storey's sheet: left out.
    pub piers_left_out: u32,
    /// Sheet faces with no label in the block: none expected.
    pub unlabelled: u32,
    /// Pieces between two deck ways whose lines lie more than a parapet's height apart: each deck has its own edge.
    pub torn: u32,
}

/// The meshes of a cell's storeys over the ground: see the module's bytes. Triangles and slots hold whole corner
/// indices; `open` holds the pieces that may stay open (on a cell line or an end line where storey 0 meets the deck).
#[derive(Clone, Debug, PartialEq)]
pub struct StoreyMesh {
    pub cell: CellKey,
    pub corners: Vec<[u32; 3]>,
    pub triangles: Vec<[u32; 3]>,
    pub triangle_runs: Vec<[u32; 2]>,
    /// Triangles per part, in the order of `PARTS`.
    pub parts: [u32; PARTS.len()],
    pub slots: Vec<[u32; 4]>,
    pub slot_runs: Vec<[u32; 2]>,
    pub top_slots: u32,
    pub open: Vec<[P; 2]>,
    pub stats: ShellStats,
}

impl StoreyMesh {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut head = vec![
            MAGIC,
            self.cell.x,
            self.cell.y,
            self.corners.len() as u32,
            self.triangles.len() as u32,
            self.triangle_runs.len() as u32,
            self.slots.len() as u32,
            self.slot_runs.len() as u32,
            self.top_slots,
        ];
        head.extend(self.parts);
        let words = (head.into_iter())
            .chain(self.corners.iter().flatten().copied())
            .chain(self.triangle_runs.iter().flatten().copied())
            .chain(packed(&self.triangles, &self.triangle_runs))
            .chain(self.slot_runs.iter().flatten().copied())
            .chain(packed(&self.slots, &self.slot_runs));
        words.flat_map(u32::to_le_bytes).collect()
    }

    /// A corner's point, label and offset (metres).
    pub fn corner(&self, k: usize) -> (P, u32, f64) {
        let [w0, w1, w2] = self.corners[k];
        let mask = (1 << 19) - 1;
        ([(w0 & mask) as i32, (w1 & mask) as i32], w0 >> 19 | (w1 >> 19 & 0xfff) << 13, f64::from(f32::from_bits(w2)))
    }

    /// The range of `triangles` of part k of `PARTS`.
    pub fn part(&self, k: usize) -> std::ops::Range<usize> {
        let start = self.parts[..k].iter().sum::<u32>() as usize;
        start..start + self.parts[k] as usize
    }
}

/// The corners and the items of the mesh as it grows.
#[derive(Default)]
struct Build {
    /// Each corner as it is asked for, its key (`corner_key`) and its offset's bits; equal ones are one corner.
    keys: Vec<(u64, u32)>,
    tris: [Vec<[u32; 3]>; PARTS.len()],
    /// The tops' slots, then the undersides' risers.
    slots: [Vec<[u32; 4]>; 2],
    /// A corner of a label at a point that takes another label (`stub_label`): its key (`corner_key`) and that label,
    /// sorted.
    relabel: Vec<(u64, u32)>,
}

/// A point in one word, in the order y, then x.
fn yx(p: P) -> u64 {
    u64::from(p[1] as u32 ^ 1 << 31) << 32 | u64::from(p[0] as u32 ^ 1 << 31)
}

fn of_yx(k: u64) -> P {
    [(k as u32 ^ 1 << 31) as i32, ((k >> 32) as u32 ^ 1 << 31) as i32]
}

/// A corner's point and label in one word, in the order y, then x, then the label.
fn corner_key(p: P, label: u32) -> u64 {
    (p[1] as u64) << 44 | (p[0] as u64) << 25 | u64::from(label & NO_LABEL)
}

impl Build {
    fn corner(&mut self, p: P, label: u32, offset: f64) -> u32 {
        let label = self.relabelled(p, label).unwrap_or(label);
        self.keys.push((corner_key(p, label), (offset as f32).to_bits()));
        self.keys.len() as u32 - 1
    }

    /// The label that a corner of label l at p takes instead, if any.
    fn relabelled(&self, p: P, l: u32) -> Option<u32> {
        let k = corner_key(p, l);
        self.relabel.binary_search_by_key(&k, |x| x.0).ok().map(|i| self.relabel[i].1)
    }

    /// A quad with its corners counter-clockwise seen from outside.
    fn quad(&mut self, part: usize, c: [u32; 4]) {
        self.tris[part].extend([[c[0], c[1], c[2]], [c[0], c[2], c[3]]]);
    }
}

fn cross(o: P, a: P, b: P) -> i64 {
    (i64::from(a[0]) - i64::from(o[0])) * (i64::from(b[1]) - i64::from(o[1]))
        - (i64::from(a[1]) - i64::from(o[1])) * (i64::from(b[0]) - i64::from(o[0]))
}

/// Twice the signed area in the frame (x east, y south): positive where the ring turns from +x toward +y.
fn area2(ring: &[P]) -> i64 {
    (0..ring.len()).map(|i| cross([0, 0], ring[i], ring[(i + 1) % ring.len()])).sum()
}

/// The points of `points` (`yx` keys, sorted) within a unit of the segment a → b and strictly between its ends, in
/// order from a.
fn on_segment(points: &[(u64, u32)], a: P, b: P) -> Vec<P> {
    let d = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1])];
    let l2 = d[0] * d[0] + d[1] * d[1];
    if l2 <= 0.0 {
        return Vec::new();
    }
    let (lo, hi) = (yx([i32::MIN, a[1].min(b[1]) - 1]), yx([i32::MAX, a[1].max(b[1]) + 1]));
    let first = points.partition_point(|p| p.0 < lo);
    let mut out: Vec<(f64, P)> = Vec::new();
    for p in points[first..].iter().take_while(|p| p.0 <= hi).map(|p| of_yx(p.0)).filter(|&p| p != a && p != b) {
        let w = [f64::from(p[0] - a[0]), f64::from(p[1] - a[1])];
        let t = (w[0] * d[0] + w[1] * d[1]) / l2;
        if t > 0.0 && t < 1.0 && (w[0] * d[1] - w[1] * d[0]).abs() / l2.sqrt() <= 1.0 {
            // A few points an edge: in order of t as they come.
            let at = out.iter().position(|x| x.0 > t).unwrap_or(out.len());
            out.insert(at, (t, p));
        }
    }
    out.into_iter().map(|x| x.1).collect()
}

/// Whether a and b lie on one line of the cell square.
fn on_cell_line(a: P, b: P) -> bool {
    let c = CELL_UNITS as i32;
    (a[0] == b[0] && (a[0] == 0 || a[0] == c)) || (a[1] == b[1] && (a[1] == 0 || a[1] == c))
}

fn on_square(p: P) -> bool {
    let c = CELL_UNITS as i32;
    p[0] == 0 || p[0] == c || p[1] == 0 || p[1] == c
}

/// The point at k/n of the way from a to b, rounded to the frame unit (as the cell mesh's pieces).
fn part_of(a: P, b: P, k: i64, n: i64) -> P {
    [0, 1].map(|i| {
        let v = i64::from(a[i]) * (n - k) + i64::from(b[i]) * k;
        (2 * v + n).div_euclid(2 * n) as i32
    })
}

/// A vertical quad on the corners `[a_lo, a_hi, b_lo, b_hi]` of a segment a → b, facing away from the solid, which
/// lies left of a → b in the frame (x east, y south) where `left`, else right.
fn wall_of(b: &mut Build, part: usize, [a_lo, a_hi, b_lo, b_hi]: [u32; 4], left: bool) {
    if left {
        b.quad(part, [b_lo, a_lo, a_hi, b_hi]);
    } else {
        b.quad(part, [a_lo, b_lo, b_hi, a_hi]);
    }
}

/// A wall on the segment a → b from offsets `lo` to `hi` (at a, at b), with the labels at a and at b.
fn wall(
    b: &mut Build,
    part: usize,
    (pa, pb): (P, P),
    left: bool,
    (la, lb): (u32, u32),
    (lo, hi): ([f64; 2], [f64; 2]),
) {
    let c = [b.corner(pa, la, lo[0]), b.corner(pa, la, hi[0]), b.corner(pb, lb, lo[1]), b.corner(pb, lb, hi[1])];
    wall_of(b, part, c, left);
}

/// A closed prism over a convex ring of the cell (cut by the cell square): its corners at each point take the top's
/// label and offset and the foot's. Its walls on a cell line stay open.
fn prism(b: &mut Build, part: usize, ring: &[P], top: (u32, f64), foot: (u32, f64)) {
    let n = ring.len();
    if n < 3 || area2(ring) == 0 {
        return;
    }
    // A positive ring in the frame holds its inside on the left of each edge and turns clockwise seen from above.
    let left = area2(ring) > 0;
    let t: Vec<u32> = ring.iter().map(|&p| b.corner(p, top.0, top.1)).collect();
    let f: Vec<u32> = ring.iter().map(|&p| b.corner(p, foot.0, foot.1)).collect();
    for i in 0..n {
        let j = (i + 1) % n;
        if !on_cell_line(ring[i], ring[j]) {
            wall_of(b, part, [f[i], t[i], f[j], t[j]], left);
        }
    }
    for i in 1..n - 1 {
        let (up, down) = if left {
            ([t[0], t[i + 1], t[i]], [f[0], f[i], f[i + 1]])
        } else {
            ([t[0], t[i], t[i + 1]], [f[0], f[i + 1], f[i]])
        };
        b.tris[part].extend([up, down]);
    }
}

/// A convex ring about the point `o`, given as offsets from it, cut by the cell square, its points rounded to the
/// frame unit. The cut is reckoned from `o`, so two cells that share the ring cut it alike on their shared line.
fn clip(o: P, ring: &[[f64; 2]]) -> Vec<P> {
    let c = CELL_UNITS as f64;
    let mut pts = ring.to_vec();
    let lines = [(0, -f64::from(o[0]), false), (0, c - f64::from(o[0]), true), (1, -f64::from(o[1]), false)];
    for (axis, bound, keep_low) in lines.into_iter().chain([(1, c - f64::from(o[1]), true)]) {
        let inside = |p: &[f64; 2]| if keep_low { p[axis] <= bound } else { p[axis] >= bound };
        let mut out = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            if inside(&a) {
                out.push(a);
            }
            if inside(&a) != inside(&b) {
                let t = (bound - a[axis]) / (b[axis] - a[axis]);
                let mut q = [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])];
                q[axis] = bound;
                out.push(q);
            }
        }
        pts = out;
    }
    let mut ring: Vec<P> = pts.iter().map(|p| [0, 1].map(|k| o[k] + libm::round(p[k]) as i32)).collect();
    ring.dedup();
    if ring.len() > 1 && ring[0] == ring[ring.len() - 1] {
        ring.pop();
    }
    ring
}

/// What the shell reads of a label: its way (the material line's) and that line's samples.
struct Lines<'a> {
    words: &'a [u32],
}

impl Lines<'_> {
    fn line(&self, label: u32) -> Option<&[u32]> {
        let l = records::label(self.words, label)[2];
        (l != u32::MAX).then(|| &self.words[self.words[6] as usize + LINE * l as usize..][..LINE])
    }

    fn way(&self, label: u32) -> Option<u32> {
        self.line(label).map(|l| l[0])
    }

    /// The metres along the label's line and its unit direction at q's foot on its nearest segment.
    fn along(&self, label: u32, q: [f64; 2]) -> Option<(f64, [f64; 2])> {
        self.foot(label, q).map(|(m, d, _)| (m, d))
    }

    /// The metres along the label's line, its unit direction and its height at q's foot on its nearest segment.
    fn foot(&self, label: u32, q: [f64; 2]) -> Option<(f64, [f64; 2], f64)> {
        let l = self.line(label)?;
        let (first, n) = (l[2] as usize, l[3] as usize);
        let s = |i: usize| &self.words[self.words[7] as usize + SAMPLE * (first + i)..][..SAMPLE];
        let pos = |i: usize| [f64::from(s(i)[0] as i32), f64::from(s(i)[1] as i32)];
        let mut best = (f64::INFINITY, 0.0, [1.0, 0.0], 0.0);
        let f = |i: usize, k: usize| f64::from(f32::from_bits(s(i)[k]));
        for i in 0..n.saturating_sub(1) {
            let (a, b) = (pos(i), pos(i + 1));
            let d = [b[0] - a[0], b[1] - a[1]];
            let l2 = d[0] * d[0] + d[1] * d[1];
            if l2 <= 0.0 {
                continue;
            }
            let t = (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0);
            let e = libm::hypot(q[0] - a[0] - t * d[0], q[1] - a[1] - t * d[1]);
            if e < best.0 {
                let len = l2.sqrt();
                let mix = |k: usize| f(i, k) + t * (f(i + 1, k) - f(i, k));
                best = (e, mix(3), [d[0] / len, d[1] / len], mix(5));
            }
        }
        best.0.is_finite().then_some((best.1, best.2, best.3))
    }
}

/// Whether q lies in face f of a plan (even-odd over its rings).
fn in_face(plan: &FacePlan, f: usize, q: [f64; 2]) -> bool {
    let mut inside = false;
    for ring in &plan.faces[f].rings {
        let n = ring.len();
        for i in 0..n {
            let (a, b) = (plan.vertices[ring[i] as usize], plan.vertices[ring[(i + 1) % n] as usize]);
            let (a, b) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
            if (a[1] > q[1]) != (b[1] > q[1]) && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
                inside = !inside;
            }
        }
    }
    inside
}

/// Whether q lies in a carriageway, a rail bed or a crossing of a storey's plan, past its edge.
fn paved_at(plan: &FacePlan, q: [f64; 2]) -> bool {
    (0..plan.faces.len()).any(|f| {
        matches!(plan.faces[f].surface, Surface::Carriageway | Surface::RailBed | Surface::Crossing)
            && in_face(plan, f, q)
            && edge_distance(plan, f, q) > ON_EDGE_UNITS
    })
}

/// The label of storey 0's paved face or shoulder that meets a deck's end at point p of its end line: of those a probe
/// `STUB_PROBE_M` to each side of p finds, the one whose line at p lies nearest label l's, within `STUB_STEP_M`.
fn stub_label(words: &[u32], lines: &Lines, l: u32, p: P, mpu: f64) -> Option<u32> {
    let q = [f64::from(p[0]), f64::from(p[1])];
    let (_, dir, z) = lines.foot(l, q)?;
    let r = STUB_PROBE_M / mpu;
    let probes = [[q[0] + dir[0] * r, q[1] + dir[1] * r], [q[0] - dir[0] * r, q[1] - dir[1] * r]];
    (probes.iter().filter_map(|&x| find(words, x)))
        .filter(|&g| {
            let s = records::label(words, g)[0] & 0xff;
            [Surface::Carriageway, Surface::Shoulder, Surface::RailBed, Surface::Crossing]
                .iter()
                .any(|&x| x as u32 == s)
        })
        .filter_map(|g| lines.foot(g, q).map(|x| (g, (x.2 - z).abs())))
        .filter(|&(_, d)| d <= STUB_STEP_M)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(g, _)| g)
}

/// The probes of `stub_label` stand this far along the deck's line from its end point.
const STUB_PROBE_M: f64 = 0.3;
/// A stub's line and its deck's meet at the end line; past this apart a face's line belongs to another way (metres).
const STUB_STEP_M: f64 = 0.25;

/// A corner within this many frame units of a face's edge stands on it: the rings' corners are rounded.
const ON_EDGE_UNITS: f64 = 4.0;

/// The distance from q to the nearest edge of face f (frame units).
fn edge_distance(plan: &FacePlan, f: usize, q: [f64; 2]) -> f64 {
    let mut best = f64::INFINITY;
    for ring in &plan.faces[f].rings {
        let n = ring.len();
        for i in 0..n {
            let (a, b) = (plan.vertices[ring[i] as usize], plan.vertices[ring[(i + 1) % n] as usize]);
            let (a, b) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
            let d = [b[0] - a[0], b[1] - a[1]];
            let l2 = d[0] * d[0] + d[1] * d[1];
            let t = if l2 > 0.0 { (((q[0] - a[0]) * d[0] + (q[1] - a[1]) * d[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
            best = best.min(libm::hypot(q[0] - a[0] - t * d[0], q[1] - a[1] - t * d[1]));
        }
    }
    best
}

/// The sheet face of a storey's plan that holds q, with its label.
fn sheet_at(plan: &FacePlan, labels: &[Option<u32>], q: [f64; 2]) -> Option<u32> {
    (0..plan.faces.len())
        .filter(|&f| plan.faces[f].surface != Surface::Terrain)
        .find(|&f| in_face(plan, f, q))
        .and_then(|f| labels[f])
}

/// The inner points of a box along a polyline of the free outline (the sheet on its left in the frame), `width_m`
/// into the sheet: mitred at each inner point; at an end on a cell line, on that line, so the neighbour's box meets it.
fn inner_points(pts: &[P], width_m: f64, origin_y: f64, along: &dyn Fn(usize) -> Option<[f64; 2]>) -> Vec<P> {
    let n = pts.len();
    let unit = |a: P, c: P| {
        let d = [f64::from(c[0] - a[0]), f64::from(c[1] - a[1])];
        let l = libm::hypot(d[0], d[1]).max(1e-9);
        [d[0] / l, d[1] / l]
    };
    let left = |d: [f64; 2]| [-d[1], d[0]];
    let w = |p: P| width_m / metres_per_unit_at(origin_y + f64::from(p[1]));
    let c = CELL_UNITS as i32;
    (0..n)
        .map(|i| {
            let p = pts[i];
            let d_in = if i > 0 { Some(unit(pts[i - 1], p)) } else { None };
            let d_out = if i + 1 < n { Some(unit(p, pts[i + 1])) } else { None };
            // The offset from p, rounded as one vector: two cells that share p give it alike.
            let delta = match (d_in, d_out) {
                (Some(a), Some(b)) => {
                    let (na, nb) = (left(a), left(b));
                    let m = [na[0] + nb[0], na[1] + nb[1]];
                    let ml = libm::hypot(m[0], m[1]);
                    let m = if ml > 1e-9 { [m[0] / ml, m[1] / ml] } else { na };
                    let s = w(p) / (m[0] * na[0] + m[1] * na[1]).max(0.5);
                    [s * m[0], s * m[1]]
                }
                (Some(d), None) | (None, Some(d)) => {
                    let on = |axis: usize| p[axis] == 0 || p[axis] == c;
                    // An end on a cell line takes the direction of its way's line there, which both cells read from
                    // the same samples, and its inner point slides along it onto the cell line.
                    let d = match along(i).filter(|_| on(0) || on(1)) {
                        Some(e) if e[0] * d[0] + e[1] * d[1] < 0.0 => [-e[0], -e[1]],
                        Some(e) => e,
                        None => d,
                    };
                    let nrm = left(d);
                    let mut v = [w(p) * nrm[0], w(p) * nrm[1]];
                    for axis in (0..2).filter(|&axis| on(axis) && d[axis].abs() > 0.2) {
                        let t = -v[axis] / d[axis];
                        v = [v[0] + t * d[0], v[1] + t * d[1]];
                        v[axis] = 0.0;
                    }
                    v
                }
                (None, None) => [0.0, 0.0],
            };
            [0, 1].map(|k| (p[k] + libm::round(delta[k]) as i32).clamp(0, c))
        })
        .collect()
}

/// A box along a polyline of the free outline with its inner points (`inner_points`), from offset `lo` to `hi`, one
/// label at each point. Its ends are closed, except on a cell line.
fn strip(b: &mut Build, part: usize, pts: &[P], inner: &[P], labels: &[u32], (lo, hi): (f64, f64)) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    for i in 0..n - 1 {
        let (p0, p1, q0, q1) = (pts[i], pts[i + 1], inner[i], inner[i + 1]);
        let (l0, l1) = (labels[i], labels[i + 1]);
        wall(b, part, (p0, p1), true, (l0, l1), ([lo; 2], [hi; 2]));
        wall(b, part, (q0, q1), false, (l0, l1), ([lo; 2], [hi; 2]));
        let top = [b.corner(p0, l0, hi), b.corner(q0, l0, hi), b.corner(q1, l1, hi), b.corner(p1, l1, hi)];
        b.quad(part, top);
        let bottom = [b.corner(p0, l0, lo), b.corner(p1, l1, lo), b.corner(q1, l1, lo), b.corner(q0, l0, lo)];
        b.quad(part, bottom);
    }
    if !on_square(pts[0]) {
        wall(b, part, (inner[0], pts[0]), true, (labels[0], labels[0]), ([lo; 2], [hi; 2]));
    }
    if !on_square(pts[n - 1]) {
        wall(b, part, (inner[n - 1], pts[n - 1]), false, (labels[n - 1], labels[n - 1]), ([lo; 2], [hi; 2]));
    }
}

/// The meshes of cell `cell`'s storeys over the ground from their plans, the cell's block (`words`, with every
/// storey's records) and what their shells read.
pub fn mesh_storeys(
    cell: CellKey,
    storeys: &Storeys,
    words: &[u32],
    structures: &Structures,
) -> Result<StoreyMesh, MeshError> {
    let origin_y = cell.origin().y as f64;
    let mpu = |y: i32| metres_per_unit_at(origin_y + f64::from(y));
    let lines = Lines { words };
    let mut ground_ends: Vec<(u64, u32)> = storeys.ground_ends.iter().map(|&p| (yx(p), 0)).collect();
    ground_ends.sort_unstable();
    ground_ends.dedup();
    let mut b = Build::default();
    let mut stats = ShellStats::default();
    let mut open = Vec::new();
    let mut all_labels = Vec::new();
    for plan in &storeys.plans {
        let labels = face_labels(&plan.faces, words);
        let sheet = |f: u32| f != OUTSIDE && plan.faces[f as usize].surface != Surface::Terrain;
        stats.unlabelled +=
            (0..plan.faces.len() as u32).filter(|&f| sheet(f) && labels[f as usize].is_none()).count() as u32;
        let label = |f: u32| labels[f as usize].unwrap_or(NO_LABEL);
        let way = |l: u32| lines.way(l).and_then(|w| storeys.way(w));
        let depth = |l: u32| way(l).map_or(0.0, |w| w.depth_m);
        // The face edges of the sheet in pieces.
        let mut pieces: Vec<(P, P, u32, u32, EdgeKind)> = Vec::new();
        for e in plan.edges.iter().filter(|e| sheet(e.left) || sheet(e.right)) {
            let (pa, pb) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
            // The sheet's faces split it: past the sheet lies nothing of the storey.
            let l = [e.left, e.right]
                .iter()
                .filter(|&&f| sheet(f))
                .map(|&f| l_max_m(plan.faces[f as usize].surface, plan.faces[f as usize].rule))
                .fold(f64::INFINITY, f64::min);
            let mid = (pa[1] + pb[1]) / 2;
            let len = libm::hypot(f64::from(pb[0] - pa[0]), f64::from(pb[1] - pa[1])) * mpu(mid);
            // An end line of the free outline takes storey 0's points on it, where it has any.
            let ground = if e.kind == EdgeKind::DeckEdge && sheet(e.left) != sheet(e.right) {
                on_segment(&ground_ends, pa, pb)
            } else {
                Vec::new()
            };
            let n = if ground.is_empty() { (len / l).ceil().max(1.0) as i64 } else { ground.len() as i64 + 1 };
            let mut last = pa;
            for k in 1..=n {
                let p = if k == n {
                    pb
                } else if ground.is_empty() {
                    part_of(pa, pb, k, n)
                } else {
                    ground[k as usize - 1]
                };
                if p != last {
                    pieces.push((last, p, e.left, e.right, e.kind));
                    last = p;
                }
            }
        }
        // Where storey 0 meets the deck on an end line of the free outline, the deck's corners there take the label
        // of storey 0's paved face or shoulder past it: the two meshes stand at one height on the line.
        for &(pa, pc, left, right, _) in pieces.iter().filter(|p| p.4 == EdgeKind::DeckEdge && sheet(p.2) != sheet(p.3))
        {
            let l = label(if sheet(left) { left } else { right });
            for p in [pa, pc] {
                if let Some(g) = stub_label(words, &lines, l, p, mpu(p[1])) {
                    b.relabel.push((corner_key(p, l), g));
                }
            }
        }
        b.relabel.sort_unstable();
        b.relabel.dedup_by_key(|x| x.0);
        let mut keyed: Vec<(u64, u32)> = pieces.iter().flat_map(|p| [(yx(p.0), 0), (yx(p.1), 0)]).collect();
        keyed.sort_unstable();
        keyed.dedup();
        let points: Vec<P> = keyed.iter().map(|k| of_yx(k.0)).collect();
        let at = |p: P| keyed.binary_search(&(yx(p), 0)).unwrap() as u32;
        let segments: Vec<Segment> =
            pieces.iter().map(|&(a, c, left, right, _)| Segment { a: at(a), b: at(c), left, right }).collect();
        let (tris, _) = cdt::triangulate(&points, &segments, segments.len())?;
        for (t, class) in tris.iter().filter(|(_, c)| sheet(*c) && labels[*c as usize].is_some()) {
            let l = label(*class);
            let top = t.map(|v| b.corner(points[v as usize], l, 0.0));
            let under = t.map(|v| b.corner(points[v as usize], l, -depth(l)));
            b.tris[TOP].push(top);
            b.tris[UNDER].push([under[0], under[2], under[1]]);
        }
        // Slots between two labels; the free outline's edge faces; the open pieces. Two deck ways side by side on
        // the sheet are one deck, but where their lines lie more than a parapet's height apart at either end of a
        // piece, each has its own edge there: a slot would be a wall from one deck to the other. A piece decides from
        // its own two points, so two cells decide alike; where a torn run meets a slot, all their corners at the
        // shared point lie on one vertical line, which closes them.
        let mut free: Vec<(P, P, u32)> = Vec::new();
        let deck = |f: u32| lines.way(label(f)).filter(|&w| storeys.way(w).is_some());
        let apart = |p: P, l: u32, r: u32| {
            let z = |f: u32| lines.foot(label(f), [f64::from(p[0]), f64::from(p[1])]).map(|x| x.2);
            z(l).zip(z(r)).is_some_and(|(za, zb)| (za - zb).abs() > PARAPET_M[1])
        };
        for &(pa, pc, left, right, kind) in &pieces {
            let torn = sheet(left)
                && sheet(right)
                && deck(left).zip(deck(right)).is_some_and(|(a, c)| a != c)
                && (apart(pa, left, right) || apart(pc, left, right));
            stats.torn += u32::from(torn);
            let sides: &[bool] = match (sheet(left), sheet(right)) {
                _ if torn => &[true, false],
                (true, true) => &[],
                (s_left, _) => &[s_left],
            };
            if sides.is_empty() {
                let (la, lb) = (label(left), label(right));
                // Two labels that take one label at both ends (`relabel`) need no slot.
                let one = |p: P| b.relabelled(p, la).is_some_and(|g| b.relabelled(p, lb) == Some(g));
                if la != lb && !(one(pa) && one(pc)) {
                    let (da, db) = (-depth(la), -depth(lb));
                    let top =
                        [b.corner(pa, la, 0.0), b.corner(pc, la, 0.0), b.corner(pc, lb, 0.0), b.corner(pa, lb, 0.0)];
                    let under =
                        [b.corner(pa, la, da), b.corner(pc, la, da), b.corner(pc, lb, db), b.corner(pa, lb, db)];
                    b.slots[0].push(top);
                    b.slots[1].push(under);
                }
                continue;
            }
            if !torn && (kind == EdgeKind::DeckEdge || kind == EdgeKind::CellBorder || on_cell_line(pa, pc)) {
                open.push([pa, pc]);
                continue;
            }
            for &s_left in sides {
                let f = if s_left { left } else { right };
                let l = label(f);
                let d = -depth(l);
                wall(&mut b, EDGE, (pa, pc), s_left, (l, l), ([d; 2], [0.0; 2]));
                // The parapet or the railing stands on the band: across a deck's free end it would stand on
                // the carriageway.
                if plan.faces[f as usize].surface == Surface::DeckEdge {
                    free.push(if s_left { (pa, pc, l) } else { (pc, pa, l) });
                }
            }
        }
        // The free outline's chains, the sheet on their left; each run of one edge feature a parapet or a railing. A
        // chain stays on one deck way: past a torn run's end, the next free piece may be the other deck's. The pieces
        // by their first point, and their last points with their way, in sorted lists; a chain starts at a piece that
        // no piece of its way leads to.
        let way_of = |k: usize| lines.way(free[k].2).unwrap_or(u32::MAX);
        let mut from: Vec<(u64, u32)> = free.iter().enumerate().map(|(k, f)| (yx(f.0), k as u32)).collect();
        from.sort_unstable();
        let mut to: Vec<(u64, u32)> = (0..free.len()).map(|k| (yx(free[k].1), way_of(k))).collect();
        to.sort_unstable();
        let mut used = vec![false; free.len()];
        let starts: Vec<usize> = (0..free.len())
            .filter(|&k| to.binary_search(&(yx(free[k].0), way_of(k))).is_err())
            .chain(0..free.len())
            .collect();
        for s in starts {
            if used[s] {
                continue;
            }
            let mut chain = vec![s];
            used[s] = true;
            loop {
                let last = *chain.last().unwrap();
                let end = yx(free[last].1);
                let first = from.partition_point(|e| e.0 < end);
                let next = (from[first..].iter().take_while(|e| e.0 == end).map(|e| e.1 as usize))
                    .find(|&k| !used[k] && way_of(k) == way_of(last));
                let Some(k) = next else { break };
                used[k] = true;
                chain.push(k);
            }
            let railing = |k: usize| way(free[k].2).is_some_and(|w| w.railing);
            let mut i = 0;
            while i < chain.len() {
                let mut j = i + 1;
                while j < chain.len() && railing(chain[j]) == railing(chain[i]) {
                    j += 1;
                }
                let run = &chain[i..j];
                let mut pts: Vec<P> = run.iter().map(|&k| free[k].0).collect();
                pts.push(free[run[run.len() - 1]].1);
                let mut ls: Vec<u32> = run.iter().map(|&k| free[k].2).collect();
                ls.push(*ls.last().unwrap());
                let width = if railing(run[0]) { BAR_M } else { PARAPET_M[0] };
                let inner = inner_points(&pts, width, origin_y, &|i| {
                    lines.along(ls[i], [f64::from(pts[i][0]), f64::from(pts[i][1])]).map(|(_, d)| d)
                });
                // The box stands on the band: where its inner side would stand in a paved face (another way's that
                // meets the band), it ends.
                let ok: Vec<bool> = inner.iter().map(|q| !paved_at(plan, [f64::from(q[0]), f64::from(q[1])])).collect();
                let mut k = 0;
                while k + 1 < pts.len() {
                    if !(ok[k] && ok[k + 1]) {
                        k += 1;
                        continue;
                    }
                    let mut e = k + 1;
                    while e + 1 < pts.len() && ok[e + 1] {
                        e += 1;
                    }
                    let (p, q, l) = (&pts[k..=e], &inner[k..=e], &ls[k..=e]);
                    if railing(run[0]) {
                        strip(&mut b, RAILING, p, q, l, (RAIL_M - BAR_M, RAIL_M));
                        posts(&mut b, &lines, p, l, structures.post_spacing_m, origin_y, &|x| paved_at(plan, x));
                    } else {
                        strip(&mut b, PARAPET, p, q, l, (0.0, PARAPET_M[1]));
                    }
                    k = e;
                }
                i = j;
            }
        }
        all_labels.push(labels);
    }
    piers(&mut b, storeys, words, &lines, &all_labels, structures, &mut stats);
    // The corners by point (y, then x), then label and offset, each once; and the items in runs: each part and each
    // kind of slot apart.
    let mut unique = b.keys.clone();
    unique.sort_unstable();
    unique.dedup();
    let rank: Vec<u32> = b.keys.iter().map(|k| unique.binary_search(k).unwrap() as u32).collect();
    let (mut triangles, mut triangle_runs, mut parts) = (Vec::new(), Vec::new(), [0; PARTS.len()]);
    for (k, tris) in b.tris.iter().enumerate() {
        let sorted = by_least(tris, &rank);
        triangle_runs.extend(runs(&sorted)?);
        parts[k] = sorted.len() as u32;
        triangles.extend(sorted);
    }
    let (mut slots, mut slot_runs) = (Vec::new(), Vec::new());
    for s in &b.slots {
        let sorted = by_least(s, &rank);
        slot_runs.extend(runs(&sorted)?);
        slots.extend(sorted);
    }
    let corners = (unique.iter())
        .map(|&(key, offset)| {
            let (label, x, y) = ((key as u32) & NO_LABEL, (key >> 25) as u32 & 0x7ffff, (key >> 44) as u32);
            [x | label << 19, y | (label >> 13) << 19, offset]
        })
        .collect();
    Ok(StoreyMesh {
        cell,
        corners,
        triangles,
        triangle_runs,
        parts,
        slots,
        slot_runs,
        top_slots: b.slots[0].len() as u32,
        open,
        stats,
    })
}

/// A railing's posts on a polyline of the free outline: where the metres along its labels' line pass a multiple of
/// `spacing_m`, and at each end that lies on no cell line, inside the polyline; each a closed square box under the
/// rail, a closed triangular prism `BAR_M` on a side. A post nearer another one than a quarter of the spacing, or
/// across a cell line, is left out.
fn posts(
    b: &mut Build,
    lines: &Lines,
    pts: &[P],
    labels: &[u32],
    spacing_m: f64,
    origin_y: f64,
    paved: &dyn Fn([f64; 2]) -> bool,
) {
    let f = |p: P| [f64::from(p[0]), f64::from(p[1])];
    let mpu = |p: [f64; 2]| metres_per_unit_at(origin_y + p[1]);
    let mut at: Vec<([f64; 2], [f64; 2], u32)> = Vec::new();
    let n = pts.len();
    for i in 0..n - 1 {
        let (a, c) = (f(pts[i]), f(pts[i + 1]));
        let len = libm::hypot(c[0] - a[0], c[1] - a[1]);
        if len <= 0.0 {
            continue;
        }
        let d = [(c[0] - a[0]) / len, (c[1] - a[1]) / len];
        let l = labels[i];
        let inset = |p: [f64; 2], s: f64| {
            let k = s * BAR_M / mpu(p);
            [p[0] + k * d[0], p[1] + k * d[1]]
        };
        if i == 0 && !on_square(pts[0]) {
            at.push((inset(a, 1.0), d, l));
        }
        if let (Some((ma, _)), Some((mc, _))) = (lines.along(l, a), lines.along(l, c)) {
            let (lo, hi) = (ma.min(mc), ma.max(mc));
            let mut m = libm::ceil(lo / spacing_m) * spacing_m;
            while m < hi {
                let t = if mc != ma { (m - ma) / (mc - ma) } else { 0.0 };
                at.push(([a[0] + t * (c[0] - a[0]), a[1] + t * (c[1] - a[1])], d, l));
                m += spacing_m;
            }
        }
        if i + 2 == n && !on_square(pts[n - 1]) {
            at.push((inset(c, -1.0), d, l));
        }
    }
    let mut kept: Vec<[f64; 2]> = Vec::new();
    for (p, d, l) in at {
        if kept.iter().any(|q| libm::hypot(p[0] - q[0], p[1] - q[1]) * mpu(p) < spacing_m / 4.0) {
            continue;
        }
        kept.push(p);
        let s = BAR_M / mpu(p);
        let n = [-d[1], d[0]];
        let corner = |u: f64, v: f64| [p[0] + u * s * d[0] + v * s * n[0], p[1] + u * s * d[1] + v * s * n[1]];
        // A post across a cell line would stand in two cells, each with its own chain: it is left out.
        // A triangular post: the least closed box, two faces less than a square one.
        let ring = [corner(-0.5, 0.0), corner(0.5, 0.0), corner(0.0, 1.0)];
        let c = CELL_UNITS as f64;
        if ring.iter().flatten().any(|&v| !(0.0..=c).contains(&v)) || ring.iter().any(|&v| paved(v)) {
            continue;
        }
        prism(b, POST, &clip([0, 0], &ring), (l, RAIL_M - BAR_M), (l, 0.0));
    }
}

/// The piers at the baked pier points: each under the highest sheet that holds its point, along the deck's line
/// there, from the underside to `pier_depth_m` under the ground; left out where its foot stands in a paved face of
/// storey 0 or on a lower storey's sheet. A pier on a cell line is one solid in two halves: each cell reads its
/// sheet at the point of its own square nearest the pier's point, and the line, the ground and the tests of the
/// foot at the pier's whole ring, the same in both cells.
fn piers(
    b: &mut Build,
    storeys: &Storeys,
    words: &[u32],
    lines: &Lines,
    labels: &[Vec<Option<u32>>],
    structures: &Structures,
    stats: &mut ShellStats,
) {
    let Some(first) = storeys.plans.first() else { return };
    let origin_y = first.cell.origin().y as f64;
    let c = CELL_UNITS as f64;
    for &pt in &storeys.piers {
        let q = [f64::from(pt[0]), f64::from(pt[1])];
        let own = [q[0].clamp(1.0, c - 1.0), q[1].clamp(1.0, c - 1.0)];
        let Some((k, top)) =
            (0..storeys.plans.len()).rev().find_map(|k| sheet_at(&storeys.plans[k], &labels[k], own).map(|l| (k, l)))
        else {
            continue;
        };
        let (Some(w), Some((_, d))) = (lines.way(top).and_then(|w| storeys.way(w)), lines.along(top, q)) else {
            continue;
        };
        let mpu = metres_per_unit_at(origin_y + q[1]);
        let (along, across) = (w.pier_m[0] / 2.0 / mpu, w.pier_m[1] / 2.0 / mpu);
        let n = [-d[1], d[0]];
        let corner = |u: f64, v: f64| [u * along * d[0] + v * across * n[0], u * along * d[1] + v * across * n[1]];
        let whole = [corner(-1.0, -1.0), corner(1.0, -1.0), corner(1.0, 1.0), corner(-1.0, 1.0)];
        let ring = clip(pt, &whole);
        if ring.len() < 3 {
            continue;
        }
        let paved = |p: [f64; 2]| {
            find(words, p).is_some_and(|l| {
                let s = records::label(words, l)[0] & 0xff;
                [Surface::Carriageway, Surface::RailBed, Surface::Crossing].iter().any(|&x| x as u32 == s)
            })
        };
        let lower = |p: [f64; 2]| (0..k).any(|j| sheet_at(&storeys.plans[j], &labels[j], p).is_some());
        let foot: Vec<[f64; 2]> = whole.iter().map(|p| [q[0] + p[0], q[1] + p[1]]).chain([q]).collect();
        if foot.iter().any(|&p| paved(p) || lower(p)) {
            stats.piers_left_out += 1;
            continue;
        }
        let ground = find(words, q).unwrap_or(NO_LABEL);
        prism(b, PIER, &ring, (top, -w.depth_m), (ground, -structures.pier_depth_m));
        stats.piers += 1;
    }
}

/// The faults of a storey mesh's closure, per solid: the slab (top, underside, edge faces and slots), the parapets,
/// the railings' rails, their posts and the piers. In each, every edge between two corners at two points is the edge of exactly one
/// triangle in each direction (a closed, consistently turned surface), except on a cell line and on an open piece
/// (`StoreyMesh::open`), where storey 0 or the neighbour goes on. Two corners at one point (a seam between two
/// labels, or a vertical edge) span no area seen from above and count for nothing.
pub fn closure(m: &StoreyMesh) -> Vec<String> {
    let point = |c: u32| m.corner(c as usize).0;
    let open =
        |a: P, b: P| on_cell_line(a, b) || m.open.iter().any(|o| (o[0] == a && o[1] == b) || (o[0] == b && o[1] == a));
    let mut faults = Vec::new();
    let slab = |k: usize| m.triangles[m.part(k)].to_vec();
    let mut solids = vec![("slab", [slab(TOP), slab(UNDER), slab(EDGE)].concat())];
    for (k, s) in m.slots.iter().enumerate() {
        let [ap, aq, bq, bp] = *s;
        let two = if (k as u32) < m.top_slots { [[ap, aq, bq], [ap, bq, bp]] } else { [[ap, bq, aq], [ap, bp, bq]] };
        solids[0].1.extend(two);
    }
    for k in [PARAPET, RAILING, POST, PIER] {
        solids.push((PARTS[k], slab(k)));
    }
    for (name, tris) in solids {
        let mut uses: BTreeMap<(u32, u32), u32> = BTreeMap::new();
        // A triangle with two equal corners (a slot whose two labels take one label at an end) spans nothing.
        for t in tris.iter().filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2]) {
            for i in 0..3 {
                *uses.entry((t[i], t[(i + 1) % 3])).or_default() += 1;
            }
        }
        for (&(a, b), &n) in &uses {
            let (pa, pb) = (point(a), point(b));
            if pa == pb {
                continue;
            }
            let back = uses.get(&(b, a)).copied().unwrap_or(0);
            if n > 1 || (back != 1 && !open(pa, pb)) {
                faults.push(format!("{:?} {name}: edge {pa:?}-{pb:?} ({a}->{b}) used {n} times, {back} back", m.cell));
            }
        }
    }
    faults
}

/// The corners of a storey mesh's parapets and railings that stand in a carriageway, a rail bed or a crossing of
/// their storey (`Storeys::plans`), by their label's storey: none expected, as they stand on the free outline.
pub fn on_paved(m: &StoreyMesh, storeys: &Storeys, words: &[u32]) -> Vec<String> {
    let lines = Lines { words };
    let mut out = Vec::new();
    for k in [PARAPET, RAILING, POST] {
        for t in &m.triangles[m.part(k)] {
            for &c in t {
                let (p, label, _) = m.corner(c as usize);
                let storey = lines.line(label).map_or(0, |l| (l[1] >> records::LEG_SHIFT) as i8);
                let Some(plan) = storeys.plans.iter().find(|p| p.level == storey) else { continue };
                let q = [f64::from(p[0]), f64::from(p[1])];
                if paved_at(plan, q) {
                    out.push(format!("{:?}: {} corner at {p:?} in a paved face of storey {storey}", m.cell, PARTS[k]));
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// The faults of two neighbouring cells' storey meshes on their shared line (`east`: b lies east of a, else
/// south): per point of the line and offset exactly the same corners, whose heights (`height_at` of their labels in
/// each cell's block) agree within 1 mm.
pub fn border(a: (&StoreyMesh, &[u32]), b: (&StoreyMesh, &[u32]), east: bool) -> Vec<String> {
    use crate::faces::heights::height_at;
    let c = CELL_UNITS as i32;
    let (axis, along) = if east { (0, 1) } else { (1, 0) };
    let on = |(m, words): (&StoreyMesh, &[u32]), line: i32| {
        let mut out: BTreeMap<(i32, u32), Vec<f64>> = BTreeMap::new();
        for k in 0..m.corners.len() {
            let (p, l, off) = m.corner(k);
            // A pier's foot on the plain terrain (NO_LABEL) stands at the terrain's height, which no record holds.
            if p[axis] == line && l != NO_LABEL {
                let h = height_at(words, [f64::from(p[0]), f64::from(p[1])], Some(l), f64::NAN).h + off;
                out.entry((p[along], (off as f32).to_bits())).or_default().push(h);
            }
        }
        for v in out.values_mut() {
            v.sort_by(f64::total_cmp);
            v.dedup_by(|x, y| (*x - *y).abs() <= 0.001);
        }
        out
    };
    let (x, y) = (on(a, c), on(b, 0));
    let mut faults = Vec::new();
    for (from, to) in [(&x, &y), (&y, &x)] {
        for (&key, p) in from {
            let q = to.get(&key).cloned();
            let same =
                q.as_ref().is_some_and(|q| p.len() == q.len() && p.iter().zip(q).all(|(u, v)| (u - v).abs() <= 0.001));
            if !same {
                faults.push(format!(
                    "{:?}|{:?} at {} offset {}: {p:?} and {q:?}",
                    a.0.cell,
                    b.0.cell,
                    key.0,
                    f32::from_bits(key.1)
                ));
            }
        }
    }
    faults.sort();
    faults.dedup();
    faults
}
