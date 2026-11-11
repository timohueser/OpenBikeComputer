//! The records of a cell: its candidates with the data that `decide` reads, listed per bin, so that
//! a reader gives any point the overlay's label by the overlay's own decision, near and far.
//!
//! A block is little-endian words; coordinates are frame units from the cell's origin. A change of
//! the layout changes `MAGIC`.
//!
//! | Words | Holds |
//! | --- | --- |
//! | head | `HEAD` words: `MAGIC`; the counts of candidates, labels and lines; the offsets of the candidates, labels, lines, samples, extras and bin headers; the cell's row of zoom 16; the offset of the blend edges; the offset of the junction table and the count of its records; the offset of the outline edges |
//! | rules | `heights::RULES` words: the rule values of the heights (`HeightRules::words`) |
//! | candidates | `CAND` words each: kind, key, glob order, label, line, then offset and count of its discs, beyond cuts and parts in the extras |
//! | labels | `LABEL` words each: surface, owner kind, rank, height rule; owner id (a junction's: the index of its junction record); material line; rail line of a crossing; 4 host lines; 4 other lines (none for a junction: its record holds its arms) |
//! | lines | `LINE` words each, one per leg of a way (`legs`), a way's legs in order: way, finish code with flags, material and the leg (`LEG_SHIFT`), first sample, sample count, cut, fill, reach (m), class rank, shoulder, verge (m), return grade, half-width (m), the kind of each end, the steepest fill and cut |
//! | samples | `SAMPLE` words each: x, y, half-width (units), metres along, glob order, height, bank, grade, mitre direction x, y, health radius left and right (units) |
//! | extras | discs (`DISC` words), beyond cuts (a cut and a label), parts (a way and two cuts, each with a flag) |
//! | junctions | the table, a word per junction record: its offset; then the records (`junction`) |
//! | blend edges | `BLEND` words each: the ends a (x, y) and b (x, y) as i32 bits, the label of the blend face, the label of the face beyond (u32::MAX for terrain) |
//! | outline edges | `EDGE` words each (`edge_ends`, `edge_line`): the end a (x, y) as i32 bits with the paved face on the left; b − a as two i16 (x low); the host line, low half, and the terrain's grade outward in 1/`GRADE_STEPS` (i16, `i16::MIN` unknown), high half; the paved height at a and at b; the plain terrain where the cones from a and from b start to let go (NaN unknown) |
//! | bins | `BINS`² headers of `HEADER` words (the first word of the bin's items; the counts of items and segments; of height segments and height discs; the words of the items and the count of blend edges), then per bin its candidate items, segments, height segments, height discs, blend edges, and in half words (the first low) the count of its outline edges followed by them |
//!
//! Floats are f32 bits. A side is right or left of the line's direction in frame axes (y south).
//! A candidate item is its index with its winding number at the bin's reference point (its middle
//! plus half a unit) + 128 << 16, its run count, then per run a vertex count and the vertices (x, y)
//! of a ring's consecutive edges that meet the bin grown by `MARGIN_M`. The vertices are the rings'
//! own, so two cells hold the same edges. A segment item `line | sample << 16` is the centre line
//! from that sample to the next. A blend edge is an edge of the overlay's faces between a blend face
//! (a junction's region) and a face of another label, with the blend face on its left (the side
//! where cross(b − a, q − a) > 0); a bin lists by index every one within `MARGIN_M` of its square. A
//! bin lists by index every outline edge within its host's reach of its square, by host line.
//! Two cells give a point the same distance to the blend edges, bit for bit below
//! `heights.region_fade_m` and within `check::BLEND_TOL` up to `MARGIN_M`: past that, an edge may end
//! elsewhere. Snap rounding does not guarantee the bits; the border check holds them.

pub mod check;
pub mod far;

pub use far::far_block;

use std::collections::BTreeMap;
use std::f64::consts::PI;

use super::geom::P;
use super::heights::{self, Height, Items};
use super::legs::{self, Legs};
use super::points::Circle;
use super::stroke::union;
use super::{
    band, candidates, sample_at, snap, Cand, FacePlan, Glob, HeightRule, Label, Mouth, Owner, PlanRules, Rules, Sliced,
    Surface, WayLine, LAYERS, OUTSIDE, SLOPE,
};
use crate::frame::{metres_per_unit, CellKey, GlobalPoint, CELL_UNITS};
use crate::graph::WayGraph;
use crate::sections::{Finish, Material, WaySection};
use crate::CellInput;

/// "OBR" and the layout's version.
pub const MAGIC: u32 = 0x4f42_5212;

/// Whether a junction's guest `g` stops at its best host `h`: a way with round corners whose pair's
/// mouth in `plan.json` is `stop`, and whose look (finish and material) is not the host's or whose
/// finish keeps its tread (`tread_finishes`). The paint gives the face beside such an arm its own
/// look up to the route's edge. A `reach` pair keeps the junction's look.
fn stops(plan: &PlanRules, h: &WaySection, g: &WaySection) -> bool {
    let tread = g.material == Material::Finish && plan.keeps_tread(g.finish);
    (tread || (g.finish, g.material) != (h.finish, h.material))
        && g.corner_radius_m.max(g.corner_radius_walk_m) > 0.0
        && plan.pair(h.group, g.group).mouth == Mouth::Stop
}
/// Bins per side of the cell square: 4,096 frame units, about 6.5 m.
pub const BINS: i32 = 64;
pub const BIN_UNITS: i32 = CELL_UNITS as i32 / BINS;
/// A bin lists every candidate edge within this of its square: the reach of the anti-aliasing and
/// of the edge lines.
pub const MARGIN_M: f64 = 3.0;
/// A bin lists every centre-line segment within this past the way's half-width: the paint reads
/// the metres across and along of the label's way.
pub const LINE_PAD_M: f64 = 3.0;
/// The heights read a line's segments and the discs within its slope reach and this past a bin.
pub const HEIGHT_PAD_M: f64 = 1.0;
pub const HEAD: usize = 15;
pub const CAND: usize = 11;
pub const LABEL: usize = 12;
pub const LINE: usize = 16;
pub const SAMPLE: usize = 12;
/// Centre, radius, the line at each foot.
pub const DISC: usize = 5;
pub const BEYOND: usize = 5;
pub const PART: usize = 11;
pub const BLEND: usize = 6;
pub const EDGE: usize = 8;
/// A junction record: node x, y; the arm count (bits 0-7) and the graph junction (bits 16-31); then
/// `JR_ARM` words per arm in the graph's order: its record line (bits 0-15) with `ARM_HOST` and
/// `ARM_STOP`; its node sample (bits 0-15) and its last sample (bits 16-31), the first past its seam,
/// both of the record line; its unit direction at the node (`pack2x16snorm`).
pub const JR: usize = 3;
pub const JR_ARM: usize = 3;
pub const ARM_HOST: u32 = 1 << 16;
/// A guest of another look than the host that stops at it (`stops`).
pub const ARM_STOP: u32 = 1 << 17;
/// Offset, candidate items << 16 | segment items, height segment items << 16 | height disc items,
/// blend edge items.
pub const HEADER: usize = 4;

/// Candidate kinds.
pub const WAY: u32 = 0;
pub const POINT: u32 = 1;
pub const REGION: u32 = 2;
pub const BAND: u32 = 3;
pub const FILL: u32 = 4;
/// The paved outline: the union of the paving candidates, for the distance to the road's edge. It
/// takes no part in the decision.
pub const OUTLINE: u32 = 5;

/// Owner kinds.
pub const TERRAIN: u32 = 0;
pub const WAY_OWNER: u32 = 1;
pub const JUNCTION: u32 = 2;
pub const PLATFORM: u32 = 3;

/// Line flags above the finish code's byte: a rail line, a way that wears lines, one that looks as a
/// motorway, a wall class, a path (`WaySection::path`); and from `MATERIAL_SHIFT` its top's
/// `sections::Material`.
pub const RAIL: u32 = 1 << 8;
pub const MARKED: u32 = 1 << 9;
pub const MOTORWAY: u32 = 1 << 10;
/// A class with a wall at its edge where a higher line's bank needs one (`leg_wall`).
pub const WALL: u32 = 1 << 11;
pub const PATH: u32 = 1 << 14;
pub const MATERIAL_SHIFT: u32 = 16;
/// The leg of a line's way, from 0 in the way's direction, in the bits from this one of its finish word.
pub const LEG_SHIFT: u32 = 24;
// The fields of a line's finish word: the finish code, each flag, the material and the leg. No two
// overlap; a new flag joins this list.
const _: () = {
    let fields = [0xff, RAIL, MARKED, MOTORWAY, WALL, PATH, 0xff << MATERIAL_SHIFT, u32::MAX << LEG_SHIFT];
    let (mut seen, mut i) = (0u32, 0);
    while i < fields.len() {
        assert!(seen & fields[i] == 0, "two fields of a line's finish word overlap");
        seen |= fields[i];
        i += 1;
    }
};

/// End kinds of a line: a free (round) end, an arm of a junction or joint or the cut where the next
/// or the previous leg of its way goes on (`legs`), a level change.
pub const FREE: u32 = 0;
pub const MEETS: u32 = 1;
pub const LEVEL: u32 = 2;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecordStats {
    pub cands: u32,
    pub labels: u32,
    pub lines: u32,
    pub samples: u32,
    pub bins_used: u32,
    pub bin_items: u32,
    pub bin_edges: u32,
    /// The most words of one bin.
    pub max_bin_words: u32,
}

pub struct Records {
    pub cell: CellKey,
    pub words: Vec<u32>,
    pub stats: RecordStats,
}

/// A cell's faces, its records near and far, and its far block, from one pass of candidates and one
/// slice.
pub struct Built {
    pub faces: FacePlan,
    pub near: Records,
    /// For points where a pixel spans more than the band layers' detail: the near candidates without
    /// the band layers. A point that the near records give no band layer's label has the same label.
    /// It has no slope faces, so it carries no outline edges.
    pub far_records: Records,
    /// The far block (`far`) of the near records' lines.
    pub far_block: Result<Vec<u32>, String>,
}

/// The faces and records of one cell and level. `slopes = false` leaves out the slope layer.
pub fn build(graph: &WayGraph, input: &CellInput, rules: &Rules, cell: CellKey, level: i8, slopes: bool) -> Built {
    let (faces, near, far_records, far_block) = pass(graph, input, rules, cell, level, slopes, true);
    Built { faces, near, far_records: far_records.expect("asked for"), far_block }
}

/// The faces, near records and far block of `build`, without the far records, which a viewer's cell
/// does not read. An error of the far block leaves the near records.
pub fn near(
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    cell: CellKey,
    level: i8,
    slopes: bool,
) -> (FacePlan, Records, Result<Vec<u32>, String>) {
    let (faces, near, _, far_block) = pass(graph, input, rules, cell, level, slopes, false);
    (faces, near, far_block)
}

/// One pass of candidates and one slice: the faces, the near records, with `far_records` the far
/// records, and the far block of the candidates' lines, the lines of `level` (`far_block` gives level
/// 0's, as every build asks).
fn pass(
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    cell: CellKey,
    level: i8,
    slopes: bool,
    far_records: bool,
) -> (FacePlan, Records, Option<Records>, Result<Vec<u32>, String>) {
    let mut c = candidates(graph, input, rules, cell, level, if slopes { LAYERS } else { SLOPE }, true);
    let s = Sliced::new(&c);
    let faces = s.cell(&c, cell, level);
    let map = s.map();
    let near = encode(cell, graph, input, rules, &c.lines, &c.cands, &s.plan(cell, level, &map, &s.labels), true);
    let far_block = far::encode(cell, graph, input, rules, &c.lines);
    let records = far_records.then(|| {
        let labels = c.far(&s);
        c.cands.retain(|c| kind(c) != BAND);
        encode(cell, graph, input, rules, &c.lines, &c.cands, &s.plan(cell, level, &map, &labels), false)
    });
    (faces, near, records, far_block)
}

/// How far an outline edge's cones reach: the greatest slope reach of a class. The candidates are the
/// neighbours' own only up to `MARGIN_M` past the cell (the input margin holds no more past the faces'
/// longest chain), so past it two cells hold the same outline edges only as far as the border check
/// shows.
pub fn outline_m(sections: &crate::sections::Sections) -> f64 {
    (0..).map_while(|rank| sections.class_faces(rank)).map(|c| c.reach_m).fold(0.0, f64::max)
}

/// Frame units of `m` metres at the cell's edge nearer the pole: at least `m` at every point of it, so
/// a bin lists every outline edge whose cones reach a point of it.
fn outline_units(cell: CellKey, m: f64) -> f64 {
    let o = cell.origin();
    let pole = if o.y < 1 << (crate::frame::FRAME_BITS - 1) { o.y } else { o.y + CELL_UNITS };
    m / metres_per_unit(pole)
}

/// The words of a block's outline edges.
pub fn outline_words(words: &[u32]) -> &[u32] {
    &words[words[14] as usize..words[9] as usize]
}

/// An outline edge's grade in these steps per metre (`i16`; `i16::MIN` unknown).
pub const GRADE_STEPS: f64 = 4096.0;

/// The ends a and b of an outline edge's words (`EDGE`): b as its offset from a.
pub fn edge_ends(e: &[u32]) -> (P, P) {
    let a = [e[0] as i32, e[1] as i32];
    (a, [a[0] + i32::from(e[2] as u16 as i16), a[1] + i32::from((e[2] >> 16) as u16 as i16)])
}

/// The host line of an outline edge's words.
pub fn edge_line(e: &[u32]) -> u32 {
    e[3] & 0xffff
}

/// Segment ab cut where it crosses the lines of the bin grid: its ends and the crossings in order from a,
/// each rounded to the nearest unit from the exact line. Each piece lies in one bin. The cell origins lie
/// on the grid, so the grid is global, and two cells that hold segments of one line cut them at the same
/// points, whatever ends each of them sees.
fn bin_cuts(a: P, b: P) -> Vec<P> {
    let (a64, d) = (a.map(i64::from), [i64::from(b[0] - a[0]), i64::from(b[1] - a[1])]);
    let unit = i64::from(BIN_UNITS);
    // The crossings of one axis's grid lines in order from a: the share num / |d[axis]| of ab and the point.
    let crossings = |axis: usize| -> Vec<(i64, P)> {
        let (lo, hi) = (a64[axis].min(a64[axis] + d[axis]), a64[axis].max(a64[axis] + d[axis]));
        let (den, o) = (d[axis].abs(), 1 - axis);
        let mut out: Vec<(i64, P)> = (lo.div_euclid(unit) + 1..=(hi - 1).div_euclid(unit))
            .map(|k| {
                let num = (k * unit - a64[axis]) * d[axis].signum();
                // The other coordinate on the line through a and b, (a·den + num·d) / den, rounded half up.
                let other = (2 * (a64[o] * den + num * d[o]) + den).div_euclid(2 * den);
                let mut q = [(k * unit) as i32; 2];
                q[o] = other as i32;
                (num, q)
            })
            .collect();
        if d[axis] < 0 {
            out.reverse();
        }
        out
    };
    let (x, y) = (crossings(0), crossings(1));
    let (mut i, mut j, mut out) = (0, 0, vec![a]);
    while i < x.len() || j < y.len() {
        let take_x = j == y.len() || (i < x.len() && x[i].0 * d[1].abs() <= y[j].0 * d[0].abs());
        out.push(if take_x { x[i].1 } else { y[j].1 });
        (i, j) = if take_x { (i + 1, j) } else { (i, j + 1) };
    }
    out.push(b);
    out.dedup();
    out
}

/// Per bin the outline edges (by index, under 2^16) within their host's reach of its square, in the
/// order of their host lines. `reach`: a record line's slope reach in metres.
fn outline_bins(cell: CellKey, edges: &[u32], reach: &dyn Fn(u32) -> f64) -> Vec<Vec<u32>> {
    let mut bins: Vec<Vec<(u32, u32)>> = vec![Vec::new(); (BINS * BINS) as usize];
    let all = edges.as_chunks::<EDGE>().0;
    crate::probe::field(crate::probe::Field::OutlineEdges, all.len());
    assert!(all.len() < 1 << 16, "a cell's outline edges overflow a bin's half words");
    for (k, e) in all.iter().enumerate() {
        let (a, b) = edge_ends(e);
        let (a, b) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
        let r = outline_units(cell, reach(edge_line(e)));
        let bx = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
        let Some((c, rows)) = bins_of(bx, r) else { continue };
        for row in rows[0]..=rows[1] {
            for col in c[0]..=c[1] {
                if near_box(a, b, square(col, row, 0.0), r) {
                    bins[(row * BINS + col) as usize].push((edge_line(e), k as u32));
                }
            }
        }
    }
    bins.into_iter()
        .map(|mut v| {
            v.sort_unstable();
            v.into_iter().map(|(_, k)| k).collect()
        })
        .collect()
}

/// Whether segment ab comes within r of the box [x0, y0, x1, y1]: an end lies within r of it, a corner
/// within r of the segment, or the segment meets it. Squared distances, no roots.
fn near_box(a: [f64; 2], b: [f64; 2], bx: [f64; 4], r: f64) -> bool {
    let r2 = r * r;
    let to_box = |p: [f64; 2]| {
        let (dx, dy) = ((bx[0] - p[0]).max(p[0] - bx[2]).max(0.0), (bx[1] - p[1]).max(p[1] - bx[3]).max(0.0));
        dx * dx + dy * dy
    };
    if to_box(a) <= r2 || to_box(b) <= r2 {
        return true;
    }
    let u = [b[0] - a[0], b[1] - a[1]];
    let l2 = u[0] * u[0] + u[1] * u[1];
    let to_seg = |q: [f64; 2]| {
        let t = if l2 > 0.0 { (((q[0] - a[0]) * u[0] + (q[1] - a[1]) * u[1]) / l2).clamp(0.0, 1.0) } else { 0.0 };
        let (dx, dy) = (q[0] - a[0] - t * u[0], q[1] - a[1] - t * u[1]);
        dx * dx + dy * dy
    };
    let corners = [[bx[0], bx[1]], [bx[2], bx[1]], [bx[0], bx[3]], [bx[2], bx[3]]];
    corners.iter().any(|&c| to_seg(c) <= r2)
        || clip([a[0] as i32, a[1] as i32], [b[0] as i32, b[1] as i32], bx).is_some()
}

/// An outline edge's height runs linearly between its ends: a rim splits at its middle while the paved
/// field leaves that line by more than this at the middle, or on a piece longer than `QUARTER_M` at
/// either quarter, down to `SPLIT_MIN_M`. Half of the least step that the heights check counts
/// (`check::STEP_M`).
const SPLIT_M: f64 = 0.005;
const SPLIT_MIN_M: f64 = 0.25;
/// A rim also splits while the ground where its cones let go leaves the line between its ends by more than
/// this: the cones fit the need at the ends, so a ground they did not see would move the slope with the
/// cuts.
const SPLIT_GROUND_M: f64 = 0.25;
/// And while the need of its cones (the paved height less that ground) differs between its ends by more
/// than this.
const SPLIT_NEED_M: f64 = 0.5;
/// The quarters find a field that crosses the line at the middle. Its offset there grows with the
/// cube of the piece's length, so a shorter piece reads its middle alone.
const QUARTER_M: f64 = 1.0;

/// The outline edges' words (`EDGE` each) of the near plan's rims (a, b, paved label, the paved face on
/// the left): the host line, the paved field at both ends (`heights::paved_end`) from the block so far
/// through the window's bins (`items`), so two cells give an end the same bits, and the plain terrain
/// where the host's cones start to let go. The host is the face's material line, else the paved field's
/// host at the middle of the rim's piece. A rim splits at the bin grid (`bin_cuts`), then while the paved
/// field or the ground where its cones let go leaves the line between a piece's ends, or their need varies
/// (`SPLIT_*`). An edge with an end where the face has no height of its own stays out: a fault that
/// `heights::check` counts.
fn outline_edges<'a>(
    words: &[u32],
    input: &CellInput,
    rules: &Rules,
    rims: &[(P, P, u32)],
    items: &dyn Fn([f64; 2]) -> Items<'a>,
) -> Vec<u32> {
    let origin = input.key.origin();
    let fp = |p: P| [f64::from(p[0]), f64::from(p[1])];
    let terrain = |q: [f64; 2]| {
        let g = GlobalPoint { x: origin.x + libm::round(q[0]) as i64, y: origin.y + libm::round(q[1]) as i64 };
        input.terrain.height(g).unwrap_or(f64::NAN)
    };
    let mut ends: BTreeMap<(P, u32), Option<Height>> = BTreeMap::new();
    let mut end =
        |p: P, id: u32| *ends.entry((p, id)).or_insert_with(|| heights::paved_end(words, fp(p), id, &items(fp(p))));
    let metres = |p: P, q: P| {
        libm::hypot(f64::from(q[0] - p[0]), f64::from(q[1] - p[1])) * metres_per_unit(origin.y + i64::from(p[1]))
    };
    // Whole-unit middles, the same in every cell.
    let mid = |p: P, q: P| [(p[0] + q[0]).div_euclid(2), (p[1] + q[1]).div_euclid(2)];
    let mut out = Vec::with_capacity(EDGE * rims.len());
    let cell = f64::from(CELL_UNITS as i32);
    for &(a, b, id) in rims {
        let material = label(words, id)[2];
        // Outward: the right of a to b, toward the ground.
        let (fa, fb) = (fp(a), fp(b));
        let len = libm::hypot(fb[0] - fa[0], fb[1] - fa[1]);
        if len <= 0.0 {
            continue;
        }
        let n = [(fb[1] - fa[1]) / len, -(fb[0] - fa[0]) / len];
        let cuts = bin_cuts(a, b);
        for (&r0, &r1) in cuts.iter().zip(&cuts[1..]) {
            // The middle is the first point that the split reads.
            let host = if material != u32::MAX { material } else { end(mid(r0, r1), id).map_or(u32::MAX, |h| h.owner) };
            if host == u32::MAX {
                continue;
            }
            // A bin lists the edge within its host's reach: else the cell has no use for it.
            let reach = f32w(words[words[6] as usize + LINE * host as usize + 6]);
            if seg_box(r0, r1, [0.0, 0.0, cell, cell]) > outline_units(input.key, reach) {
                continue;
            }
            // The cones let go from this many metres.
            let go = reach * (1.0 - rules.plan.heights.fade_share);
            let outward = |p: [f64; 2], m: f64| {
                let u = m / metres_per_unit(origin.y + libm::round(p[1]) as i64);
                terrain([p[0] + n[0] * u, p[1] + n[1] * u])
            };
            let ground = |x: P| outward(fp(x), go);
            // The piece's edges from r0: an edge splits while the paved field, or the ground where its
            // cones let go, leaves the line between its ends.
            let mut pieces = vec![(r0, end(r0, id))];
            let mut ahead = vec![(r1, end(r1, id))];
            while let Some(&(q, hq)) = ahead.last() {
                let (p, hp) = pieces[pieces.len() - 1];
                let m = mid(p, q);
                let off = |x: P, f: f64, end: &mut dyn FnMut(P, u32) -> Option<Height>| match (hp, hq, end(x, id)) {
                    (Some(hp), Some(hq), Some(hx)) => (hx.h - (hp.h + f * (hq.h - hp.h))).abs() > SPLIT_M,
                    (hp, hq, hx) => hx.is_some() != (hp.is_some() && hq.is_some()),
                };
                let (gp, gq) = (ground(p), ground(q));
                let bent = |x: P, f: f64| (ground(x) - (gp + f * (gq - gp))).abs() > SPLIT_GROUND_M;
                let long = metres(p, q);
                let quarters = |end: &mut dyn FnMut(P, u32) -> Option<Height>| {
                    long > QUARTER_M
                        && (off(mid(p, m), 0.25, end)
                            || off(mid(m, q), 0.75, end)
                            || bent(mid(p, m), 0.25)
                            || bent(mid(m, q), 0.75))
                };
                // The cones take one slope per edge from the greater need of its ends (`heights::fit`).
                let need = |h: Option<Height>, g: f64| h.map_or(f64::NAN, |h| h.h - g);
                let varies = (need(hp, gp) - need(hq, gq)).abs() > SPLIT_NEED_M;
                if long > SPLIT_MIN_M && (off(m, 0.5, &mut end) || bent(m, 0.5) || varies || quarters(&mut end)) {
                    ahead.push((m, end(m, id)));
                    continue;
                }
                pieces.push((q, hq));
                ahead.pop();
            }
            let t: Vec<f64> = pieces.iter().map(|&(p, _)| ground(p)).collect();
            for (k, w) in pieces.windows(2).enumerate() {
                let ((p, Some(hp)), (q, Some(hq))) = (w[0], w[1]) else { continue };
                let mid = [(f64::from(p[0]) + f64::from(q[0])) / 2.0, (f64::from(p[1]) + f64::from(q[1])) / 2.0];
                let grade = (outward(mid, go) - outward(mid, go / 2.0)) / (go / 2.0);
                let grade = if grade.is_nan() {
                    i16::MIN
                } else {
                    libm::round(grade * GRADE_STEPS).clamp(-32767.0, 32767.0) as i16
                };
                let d = [q[0] - p[0], q[1] - p[1]].map(|v| u32::from(v as i16 as u16));
                crate::probe::field(crate::probe::Field::Line, host as usize);
                assert!(host < 1 << 16, "a cell's record lines overflow an outline edge's half word");
                out.extend([p[0] as u32, p[1] as u32, d[0] | d[1] << 16, host | u32::from(grade as u16) << 16]);
                out.extend([f(hp.h), f(hq.h), f(t[k]), f(t[k + 1])]);
            }
        }
    }
    out
}

fn f32w(w: u32) -> f64 {
    f64::from(f32::from_bits(w))
}

fn f(v: f64) -> u32 {
    (v as f32).to_bits()
}

/// A candidate's key as its word: rank, class, kind.
fn key_word(key: (u8, u8, u8)) -> u32 {
    u32::from(key.0) << 16 | u32::from(key.1) << 8 | u32::from(key.2)
}

fn kind(c: &Cand) -> u32 {
    if c.relabel.is_some() {
        REGION
    } else if c.fill.is_some() {
        FILL
    } else if c.glob.is_none() {
        WAY
    } else if band(c) {
        BAND
    } else {
        POINT
    }
}

/// Whether a candidate paves: a carriageway's label that is no band layer and relabels nothing.
fn paves(c: &Cand) -> bool {
    c.label.surface == Surface::Carriageway && !matches!(kind(c), REGION | BAND)
}

pub(super) fn label_words(l: &Label) -> [u32; 4] {
    let (kind, id) = match l.owner {
        Owner::Terrain => (TERRAIN, 0),
        Owner::Way(w) => (WAY_OWNER, w),
        Owner::Junction(j) => (JUNCTION, j),
        Owner::Platform(p) => (PLATFORM, p),
    };
    [
        l.surface as u32 | kind << 8 | u32::from(l.rank) << 16 | (l.rule as u32) << 24,
        id,
        l.material.unwrap_or(u32::MAX),
        l.crossing.unwrap_or(u32::MAX),
    ]
}

/// Segment ab clipped to the box [x0, y0, x1, y1], if they meet.
fn clip(a: P, b: P, bx: [f64; 4]) -> Option<([f64; 2], [f64; 2])> {
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let (ax, ay) = (f64::from(a[0]), f64::from(a[1]));
    let (dx, dy) = (f64::from(b[0]) - ax, f64::from(b[1]) - ay);
    for (p, q) in [(-dx, ax - bx[0]), (dx, bx[2] - ax), (-dy, ay - bx[1]), (dy, bx[3] - ay)] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else if p < 0.0 {
            t0 = t0.max(q / p);
        } else {
            t1 = t1.min(q / p);
        }
    }
    (t0 <= t1).then_some(([ax + t0 * dx, ay + t0 * dy], [ax + t1 * dx, ay + t1 * dy]))
}

/// The distance from segment ab to the box [x0, y0, x1, y1]: 0 where they meet.
fn seg_box(a: P, b: P, bx: [f64; 4]) -> f64 {
    if clip(a, b, bx).is_some() {
        return 0.0;
    }
    let (fa, fb) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
    let to_box = |p: [f64; 2]| {
        let dx = (bx[0] - p[0]).max(p[0] - bx[2]).max(0.0);
        let dy = (bx[1] - p[1]).max(p[1] - bx[3]).max(0.0);
        libm::hypot(dx, dy)
    };
    let corners = [[bx[0], bx[1]], [bx[2], bx[1]], [bx[0], bx[3]], [bx[2], bx[3]]];
    corners.into_iter().map(|q| seg_dist(fa, fb, q)).fold(to_box(fa).min(to_box(fb)), f64::min)
}

fn seg_dist(a: [f64; 2], b: [f64; 2], q: [f64; 2]) -> f64 {
    let u = [b[0] - a[0], b[1] - a[1]];
    let t = (((q[0] - a[0]) * u[0] + (q[1] - a[1]) * u[1]) / (u[0] * u[0] + u[1] * u[1]).max(1e-12)).clamp(0.0, 1.0);
    libm::hypot(q[0] - a[0] - t * u[0], q[1] - a[1] - t * u[1])
}

/// A bin's reference point less half a unit: its middle.
pub fn reference(i: i32) -> i32 {
    i * BIN_UNITS + BIN_UNITS / 2
}

/// The bin square [x0, y0, x1, y1] of column c and row r, grown by g units.
fn square(c: i32, r: i32, g: f64) -> [f64; 4] {
    let u = f64::from(BIN_UNITS);
    [f64::from(c) * u - g, f64::from(r) * u - g, f64::from(c + 1) * u + g, f64::from(r + 1) * u + g]
}

/// The bins whose square grown by g meets the box [x0, y0, x1, y1]: columns and rows, or none.
fn bins_of(bx: [f64; 4], g: f64) -> Option<([i32; 2], [i32; 2])> {
    bins_in(bx, g, 0)
}

/// `bins_of` over the cell's bins and `wide` more on each side (columns and rows from −`wide`).
fn bins_in(bx: [f64; 4], g: f64, wide: i32) -> Option<([i32; 2], [i32; 2])> {
    let (lo, hi) = (-f64::from(wide * BIN_UNITS), f64::from((BINS + wide) * BIN_UNITS));
    if bx[2] + g < lo || bx[3] + g < lo || bx[0] - g > hi || bx[1] - g > hi {
        return None;
    }
    let to = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).clamp(-wide, BINS - 1 + wide);
    Some(([to(bx[0] - g), to(bx[2] + g)], [to(bx[1] - g), to(bx[3] + g)]))
}

/// The bins of the build's window: the cell's and `wide` more on each side, row by row. The records keep
/// the cell's; the build reads the paved field at outline ends past the cell square from the others.
struct Window {
    wide: i32,
}

impl Window {
    fn side(&self) -> i32 {
        BINS + 2 * self.wide
    }

    fn len(&self) -> usize {
        (self.side() * self.side()) as usize
    }

    fn at(&self, col: i32, row: i32) -> usize {
        ((row + self.wide) * self.side() + col + self.wide) as usize
    }

    /// The window bin of a point (frame units), clamped to the window.
    fn of(&self, q: [f64; 2]) -> usize {
        let to = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).clamp(-self.wide, BINS - 1 + self.wide);
        self.at(to(q[0]), to(q[1]))
    }
}

/// The windings of rings at the reference point of each bin of columns c and rows r, as `find` reads
/// it: moved by (ε, ε²) in frame axes. Per row a horizontal line through the reference points in
/// doubled units (odd, so it meets no vertex), and the signed crossings east of each point, exact.
fn windings(rings: &[Vec<P>], c: [i32; 2], r: [i32; 2]) -> Vec<i32> {
    let cols = (c[1] - c[0] + 1) as usize;
    let mut out = vec![0; cols * (r[1] - r[0] + 1) as usize];
    for row in r[0]..=r[1] {
        let y = 2 * i64::from(reference(row)) + 1;
        let mut edges: Vec<([i64; 2], [i64; 2], i32)> = Vec::new();
        for ring in rings {
            for (i, &a) in ring.iter().enumerate() {
                let b = ring[(i + 1) % ring.len()];
                let (a, b) = ([2 * i64::from(a[0]), 2 * i64::from(a[1])], [2 * i64::from(b[0]), 2 * i64::from(b[1])]);
                if (a[1] < y) != (b[1] < y) {
                    edges.push((a, b, if b[1] > a[1] { 1 } else { -1 }));
                }
            }
        }
        for col in c[0]..=c[1] {
            let x = 2 * i64::from(reference(col)) + 1;
            // East of the point: (x at y) − x has the sign of num / (b.y − a.y). On the edge, the
            // moved point lies east of it.
            let east = |&(a, b, _): &&([i64; 2], [i64; 2], i32)| {
                let num = (a[0] - x) * (b[1] - a[1]) + (b[0] - a[0]) * (y - a[1]);
                num != 0 && (num > 0) == (b[1] > a[1])
            };
            out[(row - r[0]) as usize * cols + (col - c[0]) as usize] = edges.iter().filter(east).map(|e| e.2).sum();
        }
    }
    out
}

/// Per vertex of a line: the mitre direction (unit) and the health radius on the left and on the right
/// (units, 1e9 for none): the nearest crossing on that side of the mitre lines of the two quads that
/// meet at the vertex. The mitres follow the centre line's direction smoothed to a least radius
/// (`least`, units per vertex): between two vertices they turn by at most the chord over it, so two
/// neighbour mitre lines cross no nearer, and a mean over that length takes out the polyline's kinks.
/// A mitre stays within `tilt_deg` of both its chords (or the bisector's angle at a sharper vertex),
/// so where that does not reach the least radius the frame keeps its fold. The positions, and so the
/// road's outline, do not change.
fn mitres(p: &[P], least: &[f64], tilt_deg: f64) -> Vec<[f64; 4]> {
    let at = |k: usize| [f64::from(p[k][0]), f64::from(p[k][1])];
    let n = p.len();
    if n < 2 {
        return vec![[1.0, 0.0, 1e9, 1e9]; n];
    }
    // The chords' lengths and directions, unwrapped; a chord of no length keeps the one before it.
    let mut len = vec![0.0; n - 1];
    let mut dir = vec![0.0; n - 1];
    for k in 0..n - 1 {
        let d = [at(k + 1)[0] - at(k)[0], at(k + 1)[1] - at(k)[1]];
        len[k] = libm::hypot(d[0], d[1]);
        let raw = if len[k] > 0.0 {
            libm::atan2(d[1], d[0])
        } else if k > 0 {
            dir[k - 1]
        } else {
            0.0
        };
        dir[k] = if k == 0 { raw } else { dir[k - 1] + libm::remainder(raw - dir[k - 1], 2.0 * PI) };
    }
    // Per vertex the bisector's angle; per chord the most turn that the least radius allows.
    let beta: Vec<f64> = (0..n)
        .map(|i| match (i.checked_sub(1).and_then(|k| dir.get(k)), dir.get(i)) {
            (Some(a), Some(b)) => (a + b) / 2.0,
            (Some(a), None) | (None, Some(a)) => *a,
            (None, None) => 0.0,
        })
        .collect();
    let most: Vec<f64> = (0..n - 1).map(|k| len[k] / least[k].min(least[k + 1])).collect();
    // The best fit of the angles that turns by at most `most`: the mean of the greatest such function
    // below them and the least one above them.
    let (mut lo, mut hi) = (beta.clone(), beta.clone());
    for k in 0..n - 1 {
        lo[k + 1] = lo[k + 1].min(lo[k] + most[k]);
        hi[k + 1] = hi[k + 1].max(hi[k] - most[k]);
    }
    for k in (0..n - 1).rev() {
        lo[k] = lo[k].min(lo[k + 1] + most[k]);
        hi[k] = hi[k].max(hi[k + 1] - most[k]);
    }
    // A mitre stays within the tilt of both its chords, or as near to them as the bisector.
    let tilt = |i: usize, a: f64| {
        let (c0, c1) = (dir[i.saturating_sub(1)], dir[i.min(n - 2)]);
        let room = (tilt_deg.to_radians() - (c1 - c0).abs() / 2.0).max(0.0);
        a.clamp(beta[i] - room, beta[i] + room)
    };
    let fit: Vec<f64> = (0..n).map(|i| tilt(i, (lo[i] + hi[i]) / 2.0)).collect();
    // The mean of the piecewise linear angle over the least radius's arc about each vertex, the window
    // shrinking toward the ends, so an end keeps its chord's direction.
    let mut s = vec![0.0; n];
    let mut area = vec![0.0; n];
    for k in 0..n - 1 {
        s[k + 1] = s[k] + len[k];
        area[k + 1] = area[k] + len[k] * (fit[k] + fit[k + 1]) / 2.0;
    }
    let integral = |x: f64| {
        let k = s.partition_point(|&v| v <= x).clamp(1, n - 1) - 1;
        let f = if len[k] > 0.0 { ((x - s[k]) / len[k]).clamp(0.0, 1.0) } else { 0.0 };
        let v = fit[k] + f * (fit[k + 1] - fit[k]);
        area[k] + (x - s[k]).clamp(0.0, len[k]) * (fit[k] + v) / 2.0
    };
    let total = s.last().copied().unwrap_or(0.0);
    let t: Vec<[f64; 2]> = (0..n)
        .map(|i| {
            let half = (least[i] / 2.0).min(s[i]).min(total - s[i]);
            let a = if half > 0.0 {
                tilt(i, (integral(s[i] + half) - integral(s[i] - half)) / (2.0 * half))
            } else {
                beta[i]
            };
            [libm::cos(a), libm::sin(a)]
        })
        .collect();
    // Where the mitre lines of vertices i and i + 1 cross: the signed offset along i's left normal.
    let crossing = |i: usize| {
        let (ma, mb) = ([t[i][1], -t[i][0]], [t[i + 1][1], -t[i + 1][0]]);
        let den = ma[0] * mb[1] - ma[1] * mb[0];
        if den.abs() < 1e-12 {
            return f64::INFINITY;
        }
        let ab = [at(i + 1)[0] - at(i)[0], at(i + 1)[1] - at(i)[1]];
        (ab[0] * mb[1] - ab[1] * mb[0]) / den
    };
    (0..n)
        .map(|i| {
            let rho = |side: f64| {
                let near = [(i > 0).then(|| crossing(i - 1)), (i + 1 < n).then(|| crossing(i))];
                near.into_iter().flatten().filter(|c| c * side > 0.0).fold(1e9, |r: f64, c| r.min(c * side))
            };
            [t[i][0], t[i][1], rho(1.0), rho(-1.0)]
        })
        .collect()
}

/// The finish code of the viewer's looks: `Finish` in order from 1.
fn code(finish: Finish) -> u32 {
    Finish::ALL.iter().position(|&x| x == finish).map_or(0, |i| i as u32 + 1)
}

/// The words of the leg of a line from point `lo` to point `hi` (`legs`), with its first sample 0, and
/// of its samples. A leg is a line of its own: its ends at cuts meet the next legs, and its frame's mitres turn
/// as a line's that ends there. A taper's vertex between two graph samples takes their values at its
/// share of the segment.
fn line_words(
    line: &WayLine,
    (lo, hi): (usize, usize),
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    order: &dyn Fn(&Glob) -> u32,
) -> (Vec<u32>, Vec<u32>) {
    let way = &graph.ways[line.way as usize];
    let sec = &line.sec;
    let flags =
        [(line.rail, RAIL), (sec.marked, MARKED), (sec.motorway, MOTORWAY), (sec.leg_wall, WALL), (sec.path, PATH)]
            .iter()
            .fold(0, |f, &(on, bit)| f | if on { bit } else { 0 });
    let finish = code(sec.finish) | flags | (sec.material as u32) << MATERIAL_SHIFT;
    let mut w = vec![line.way, finish, 0, (hi - lo + 1) as u32];
    w.extend([f(sec.cut), f(sec.fill), f(sec.reach_m), sec.rank as u32]);
    w.extend([f(sec.shoulder_m), f(sec.verge_m), f(sec.return_grade), f(sec.half_m)]);
    // An end meets a junction or a joint whose arms all lie on the way's level; a joint to a bridge or
    // a tunnel is a level change.
    let meets = |dir: i8, st: &crate::graph::Station| {
        let arm = |a: &crate::graph::Arm| a.way == line.way && a.dir == dir && a.at.span == st.span && a.at.u == st.u;
        let level = |arms: &[crate::graph::Arm]| arms.iter().any(arm) && arms.iter().all(|a| a.level == way.level);
        graph.junctions.iter().any(|j| level(&j.arms)) || graph.joints.iter().any(|j| level(&j.arms))
    };
    for (k, (dir, st)) in [(1i8, &way.from), (-1i8, &way.to)].into_iter().enumerate() {
        w.push(if [lo > 0, hi + 1 < line.p.len()][k] {
            MEETS
        } else if line.round[k] {
            FREE
        } else if meets(dir, st) {
            MEETS
        } else {
            LEVEL
        });
    }
    w.extend([f(sec.fill_max), f(sec.cut_max)]);
    let samples = &graph.samples[way.samples.start as usize..way.samples.end as usize];
    // Per vertex: its graph sample and its share of the segment to the next one.
    let mut share = vec![(0usize, 0.0f64); line.p.len()];
    for (k, win) in line.index.windows(2).enumerate() {
        let (a, b) = (line.p[win[0]], line.p[win[1]]);
        let len = libm::hypot(f64::from(b[0] - a[0]), f64::from(b[1] - a[1])).max(1e-9);
        for (j, s) in share.iter_mut().enumerate().take(win[1]).skip(win[0]) {
            let p = line.p[j];
            *s = (k, libm::hypot(f64::from(p[0] - a[0]), f64::from(p[1] - a[1])) / len);
        }
    }
    if let Some(&last) = line.index.last() {
        share[last] = (line.index.len() - 1, 0.0);
    }
    let origin = input.key.origin();
    // The frame's least radius: the half-width and the health's margin and width, so the frame has
    // full health over the whole carriageway.
    let health = rules.plan.heights.health_margin_m + rules.plan.heights.health_width_m;
    let least: Vec<f64> =
        (line.p.iter().zip(&line.h)).map(|(p, h)| h + health / metres_per_unit(origin.y + i64::from(p[1]))).collect();
    let mitres = mitres(&line.p[lo..=hi], &least[lo..=hi], rules.plan.heights.frame_tilt_deg);
    let mut s = Vec::with_capacity(SAMPLE * (hi - lo + 1));
    for (i, &p) in line.p.iter().enumerate().take(hi + 1).skip(lo) {
        let (k, x) = share[i];
        let (s0, s1) = (&samples[k], &samples[(k + 1).min(samples.len() - 1)]);
        let lerp = |a: f64, b: f64| a + x * (b - a);
        s.extend([p[0] as u32, p[1] as u32, f(line.h[i]), f(lerp(s0.along_m, s1.along_m)), order(&line.key[i])]);
        s.extend([f(lerp(s0.z_m, s1.z_m)), f(lerp(s0.bank, s1.bank)), f(lerp(s0.grade, s1.grade))]);
        s.extend(mitres[i - lo].map(f));
    }
    (w, s)
}

/// The words of a disc; `line` gives the record line of a line at a foot.
fn disc_words(d: &Circle, line: &dyn Fn(usize, P) -> u32) -> [u32; DISC] {
    [d.centre[0] as u32, d.centre[1] as u32, f(d.radius), line(d.lines[0], d.feet[0]), line(d.lines[1], d.feet[1])]
}

/// The words of a cut as the slice holds it: its ends snapped as the overlay's `past` reads them.
fn cut_words(cut: &[P; 2]) -> [u32; 4] {
    let (a, b) = (snap(cut[0]), snap(cut[1]));
    [a[0] as u32, a[1] as u32, b[0] as u32, b[1] as u32]
}

#[allow(clippy::too_many_arguments)]
fn encode(
    cell: CellKey,
    graph: &WayGraph,
    input: &CellInput,
    rules: &Rules,
    lines: &[WayLine],
    cands: &[Cand],
    plan: &FacePlan,
    with_outline: bool,
) -> Records {
    let origin = cell.origin();
    let mpu = metres_per_unit(origin.y + CELL_UNITS / 2);
    let units = |m: f64| m / mpu;
    // The window's bins reach past the cell square as far as the ends of the outline edges that a bin
    // lists: an edge reaches a bin's square within `outline_m`, and is a bin long at most (`bin_cuts`).
    let reach_units = outline_units(cell, outline_m(&rules.sections));
    let wide = if with_outline { (reach_units / f64::from(BIN_UNITS)).ceil() as i32 + 1 } else { 0 };
    let window = Window { wide };
    // The paved outline with the circles of its fans and rounds: one union, so the distance to the
    // road's edge has no seam inside the paved area.
    let paved: Vec<Vec<P>> = cands.iter().filter(|c| paves(c)).flat_map(|c| c.rings.iter().cloned()).collect();
    let outline = union(&paved, [0, 0]);
    let outline_discs: Vec<&Circle> = cands.iter().filter(|c| paves(c)).flat_map(|c| &c.discs).collect();
    // The global names in order: a way's at its samples, a point's at its position.
    let mut globs: Vec<Glob> =
        lines.iter().flat_map(|l| l.key.iter().copied()).chain(cands.iter().filter_map(|c| c.glob)).collect();
    globs.sort_unstable();
    globs.dedup();
    let order = |g: &Glob| globs.binary_search(g).unwrap() as u32;
    // One record line per leg of each line (`legs`): its line and its first and last point.
    let mut lines_w = Vec::new();
    let mut samples_w = Vec::new();
    let mut rec: Vec<(usize, usize, usize)> = Vec::new();
    let mut rec_of: Vec<u32> = Vec::with_capacity(lines.len());
    for (l, line) in lines.iter().enumerate() {
        rec_of.push(rec.len() as u32);
        for (leg, (lo, hi)) in legs::record_ranges(&line.spans, &line.cuts, line.p.len()).into_iter().enumerate() {
            let (mut w, s) = line_words(line, (lo, hi), graph, input, rules, &order);
            w[1] |= (leg as u32) << LEG_SHIFT;
            w[2] = (samples_w.len() / SAMPLE) as u32;
            lines_w.extend(w);
            samples_w.extend(s);
            rec.push((l, lo, hi));
        }
    }
    let rec_line = |l: usize, leg: u8| rec_of[l] + u32::from(leg);
    let rec_points = |r: usize| &lines[rec[r].0].p[rec[r].1..=rec[r].2];
    let of_way: BTreeMap<u32, u32> = lines.iter().enumerate().map(|(i, l)| (l.way, i as u32)).collect();
    let line_of =
        |w: Option<u32>, leg: u8| w.and_then(|w| of_way.get(&w)).map_or(u32::MAX, |&l| rec_line(l as usize, leg));
    // The leg of a line nearest a point: a disc's foot on its line's edge.
    let legs_of: Vec<Option<Legs>> = (lines.iter())
        .map(|l| {
            let reach = |o: P| l.leg_reach_m() / metres_per_unit(origin.y + i64::from(o[1]));
            (!l.cuts.is_empty()).then(|| Legs::new(&l.p, &l.cuts, &reach))
        })
        .collect();
    let foot_line = |l: usize, foot: P| {
        if l == usize::MAX {
            return u32::MAX;
        }
        let leg = legs_of[l].as_ref().map_or(0, |g| g.nearest([f64::from(foot[0]), f64::from(foot[1])]));
        rec_line(l, leg)
    };
    // The leg of an arm of a graph point: the one it leaves along.
    let arm_line = |a: &crate::graph::Arm, pos: P| {
        of_way.get(&a.way).map_or(u32::MAX, |&l| {
            let l = l as usize;
            rec_line(l, legs::leg(&lines[l].cuts, sample_at(graph, &lines[l], &a.at, pos), a.dir))
        })
    };
    // A label's words with its material and crossing as record lines.
    let mut labels: Vec<[u32; 4]> = Vec::new();
    let mut label_id = |l: &Label| {
        let mut w = label_words(l);
        w[2] = line_of(l.material, l.legs[0]);
        w[3] = line_of(l.crossing, l.legs[1]);
        labels.iter().position(|x| *x == w).unwrap_or_else(|| {
            labels.push(w);
            labels.len() - 1
        }) as u32
    };
    let mut cands_w = Vec::new();
    let mut extras: Vec<u32> = Vec::new();
    // Per candidate, the labels it can give a point: its own and those past its seams.
    let mut given: Vec<Vec<u32>> = Vec::with_capacity(cands.len());
    for c in cands {
        let k = kind(c);
        let own = label_id(&c.label);
        let discs = extras.len() as u32;
        extras.extend(c.discs.iter().flat_map(|d| disc_words(d, &foot_line)));
        let beyond = extras.len() as u32;
        let mut ids = vec![own];
        for (cut, l) in &c.beyond {
            let id = label_id(l);
            extras.extend(cut_words(cut));
            extras.push(id);
            ids.push(id);
        }
        given.push(ids);
        let parts_at = extras.len() as u32;
        let parts = c.relabel.as_deref().unwrap_or(&[]);
        for part in parts {
            extras.push(lines[part.line].way);
            for cut in &part.cuts {
                extras.push(u32::from(cut.is_some()));
                extras.extend(cut.as_ref().map_or([0; 4], cut_words));
            }
        }
        cands_w.extend([k, key_word(c.key), c.glob.as_ref().map_or(u32::MAX, order), own]);
        cands_w.push(if k == WAY { rec_line(c.line, c.label.legs[0]) } else { u32::MAX });
        cands_w.extend([discs, c.discs.len() as u32, beyond, c.beyond.len() as u32, parts_at, parts.len() as u32]);
    }
    // The blend edges that a bin lists, each side of an edge between two labels whose face blends.
    let n_bins = (BINS * BINS) as usize;
    let margin = units(MARGIN_M);
    let mut blend: Vec<u32> = Vec::new();
    let mut bedges: Vec<Vec<u32>> = vec![Vec::new(); window.len()];
    // The paved outline near the cell (near level only): (a, b, paved label) with the paved face on the
    // left of a to b.
    let mut rims: Vec<(P, P, u32)> = Vec::new();
    for e in plan.edges.iter().filter(|e| e.left != OUTSIDE && e.right != OUTSIDE) {
        let (l, r) = (Label::of(&plan.faces[e.left as usize]), Label::of(&plan.faces[e.right as usize]));
        let (a, b) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
        let eb = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(f64::from);
        let paved = |l: &Label| l.rule as u8 >= HeightRule::Owner as u8;
        if with_outline && paved(&l) != paved(&r) && bins_of(eb, reach_units).is_some() {
            rims.push(if paved(&l) { (a, b, label_id(&l)) } else { (b, a, label_id(&r)) });
        }
        let Some((ec, er)) = bins_in(eb, margin + 1.0, window.wide).filter(|_| l != r) else { continue };
        let listed: Vec<usize> = (er[0]..=er[1])
            .flat_map(|row| (ec[0]..=ec[1]).map(move |col| (col, row)))
            .filter(|&(col, row)| clip(a, b, square(col, row, margin + 1.0)).is_some())
            .map(|(col, row)| window.at(col, row))
            .collect();
        for (region, other, a, b) in [(l, r, a, b), (r, l, b, a)] {
            if region.rule != HeightRule::Blend || listed.is_empty() {
                continue;
            }
            let other = if other.surface == Surface::Terrain { u32::MAX } else { label_id(&other) };
            for &k in &listed {
                bedges[k].push((blend.len() / BLEND) as u32);
            }
            blend.extend([a[0] as u32, a[1] as u32, b[0] as u32, b[1] as u32, label_id(&region), other]);
        }
    }
    let discs = extras.len() as u32;
    extras.extend(outline_discs.iter().flat_map(|d| disc_words(d, &foot_line)));
    cands_w.extend([OUTLINE, u32::MAX, u32::MAX, u32::MAX, u32::MAX, discs, outline_discs.len() as u32, 0, 0, 0, 0]);
    // The shapes in bin order: the paved outline first, then the band layers, then the rest, so a
    // point far from the paved area reads only these first.
    let shapes: Vec<&Vec<Vec<P>>> = cands.iter().map(|c| &c.rings).chain(std::iter::once(&outline)).collect();
    let mut by_order = vec![cands.len()];
    by_order.extend((0..cands.len()).filter(|&c| kind(&cands[c]) == BAND));
    by_order.extend((0..cands.len()).filter(|&c| kind(&cands[c]) != BAND));
    let mut items: Vec<Vec<u32>> = vec![Vec::new(); n_bins];
    let mut n_items = vec![0u32; n_bins];
    let mut stats = RecordStats::default();
    for ci in by_order {
        let rings = shapes[ci];
        let bb = rings.iter().flatten().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| {
            [b[0].min(f64::from(p[0])), b[1].min(f64::from(p[1])), b[2].max(f64::from(p[0])), b[3].max(f64::from(p[1]))]
        });
        let Some((c, r)) = bins_of(bb, margin).filter(|_| bb[0] <= bb[2]) else { continue };
        let wind = windings(rings, c, r);
        let cols = (c[1] - c[0] + 1) as usize;
        // Per bin, the edges (ring, index) that meet its square grown by the margin and a unit.
        let mut edges: Vec<Vec<(usize, usize)>> = vec![Vec::new(); wind.len()];
        for (ri, ring) in rings.iter().enumerate() {
            for (i, &a) in ring.iter().enumerate() {
                let b = ring[(i + 1) % ring.len()];
                let eb = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(f64::from);
                let Some((ec, er)) = bins_of(eb, margin + 1.0) else { continue };
                for row in er[0].max(r[0])..=er[1].min(r[1]) {
                    for col in ec[0].max(c[0])..=ec[1].min(c[1]) {
                        if clip(a, b, square(col, row, margin + 1.0)).is_some() {
                            edges[(row - r[0]) as usize * cols + (col - c[0]) as usize].push((ri, i));
                        }
                    }
                }
            }
        }
        for row in r[0]..=r[1] {
            for col in c[0]..=c[1] {
                let k = (row - r[0]) as usize * cols + (col - c[0]) as usize;
                if edges[k].is_empty() && wind[k] == 0 {
                    continue;
                }
                // The edges as runs of a ring's consecutive vertices.
                let mut runs: Vec<Vec<P>> = Vec::new();
                let mut last: Option<(usize, usize)> = None;
                for &(ri, i) in &edges[k] {
                    let ring = &rings[ri];
                    if last != Some((ri, i.wrapping_sub(1))) {
                        runs.push(vec![ring[i]]);
                    }
                    runs.last_mut().unwrap().push(ring[(i + 1) % ring.len()]);
                    last = Some((ri, i));
                }
                let b = (row * BINS + col) as usize;
                crate::probe::field(crate::probe::Field::Candidate, ci);
                items[b].extend([ci as u32 | ((wind[k].clamp(-127, 127) + 128) as u32) << 16, runs.len() as u32]);
                for run in &runs {
                    items[b].push(run.len() as u32);
                    items[b].extend(run.iter().flat_map(|v| [v[0] as u32, v[1] as u32]));
                }
                n_items[b] += 1;
                stats.bin_items += 1;
                stats.bin_edges += edges[k].len() as u32;
            }
        }
    }
    // A carriageway over a rail bed is a crossing: its label's variant with the rail as `crossing`. A
    // point reads only the candidates of its bin, so a variant is needed only for a label that a
    // candidate can give in a bin that lists a rail bed.
    let rail = |ci: usize| {
        (cands.get(ci).filter(|c| c.label.surface == Surface::RailBed))
            .map(|c| line_of(c.label.material, c.label.legs[0]))
    };
    // The rails already paired with each label.
    let mut paired: Vec<Vec<u32>> = vec![Vec::new(); labels.len()];
    for list in &items {
        let mut listed = Vec::new();
        let mut at = 0;
        while at < list.len() {
            listed.push((list[at] & 0xffff) as usize);
            at = skip_item(list, at);
        }
        let rails: Vec<u32> = listed.iter().filter_map(|&ci| rail(ci)).collect();
        for &l in listed.iter().filter_map(|&ci| given.get(ci)).flatten() {
            let w = labels[l as usize];
            if w[0] & 0xff != Surface::Carriageway as u32 {
                continue;
            }
            for &r in &rails {
                let x = [Surface::Crossing as u32 | (w[0] & !0xff), w[1], w[2], r];
                if !paired[l as usize].contains(&r) && !labels.contains(&x) {
                    paired[l as usize].push(r);
                    labels.push(x);
                }
            }
        }
    }
    let four = |mut v: Vec<u32>| {
        v.retain(|&x| x != u32::MAX);
        v.sort_unstable();
        v.dedup();
        v.resize(4, u32::MAX);
        [v[0], v[1], v[2], v[3]]
    };
    // The lines whose frame the paint of a way's label follows: its leg and the ways it meets at a
    // joint; and its other lines: those partners. Each arm names the leg it leaves along. A junction's
    // record holds its arms.
    let joints_of = |w: u32| graph.joints.iter().filter(move |j| j.arms.iter().any(|a| a.way == w));
    let hosts: Vec<[u32; 4]> = (labels.iter())
        .map(|l| match (l[0] >> 8) & 0xff {
            WAY_OWNER => {
                let partners =
                    joints_of(l[1]).flat_map(|j| j.arms.iter().filter(|a| a.way != l[1]).map(|a| arm_line(a, j.pos)));
                four(std::iter::once(l[2]).chain(partners).collect())
            }
            _ => four(Vec::new()),
        })
        .collect();
    let others: Vec<[u32; 4]> = (labels.iter().zip(&hosts))
        .map(|(l, h)| match (l[0] >> 8) & 0xff {
            WAY_OWNER => four(h.iter().copied().filter(|&x| x != l[2]).collect()),
            _ => four(Vec::new()),
        })
        .collect();
    // A record per junction that a label names, in the order of the labels.
    let mut named: Vec<u32> = Vec::new();
    for l in labels.iter().filter(|l| (l[0] >> 8) & 0xff == JUNCTION) {
        if !named.contains(&l[1]) {
            named.push(l[1]);
        }
    }
    let sec = |w: u32| of_way.get(&w).map(|&l| &lines[l as usize].sec);
    let junction_words = |id: u32| {
        let j = &graph.junctions[id as usize];
        let host = (j.arms.iter().filter(|a| a.host)).filter_map(|a| sec(a.way)).min_by_key(|s| s.rank);
        let parts: Vec<&super::points::Part> = (cands.iter())
            .filter(|c| c.label.owner == Owner::Junction(id))
            .flat_map(|c| c.relabel.iter().flatten())
            .collect();
        let mut w = vec![j.pos[0] as u32, j.pos[1] as u32, 0];
        for a in &j.arms {
            let Some(&l) = of_way.get(&a.way) else { continue };
            let (l, line) = (l as usize, &lines[l as usize]);
            let at = sample_at(graph, line, &a.at, j.pos);
            let r = rec_line(l, legs::leg(&line.cuts, at, a.dir));
            let (lo, hi) = (rec[r as usize].1, rec[r as usize].2);
            // The first sample past the seam of the region's part that holds the node; the node itself
            // where the region has none.
            let x = at as f64;
            let part = parts.iter().find(|p| p.line == l && p.lo <= x + 1e-9 && x <= p.hi + 1e-9);
            let last = match part {
                Some(p) if a.dir > 0 => (p.hi.floor() as usize + 1).min(hi),
                Some(p) => (p.lo.ceil() as usize).saturating_sub(1).max(lo),
                None => at,
            };
            let next = at.checked_add_signed(isize::from(a.dir)).filter(|n| (lo..=hi).contains(n));
            let d = next.map_or([0.0, 0.0], |n| {
                [f64::from(line.p[n][0] - line.p[at][0]), f64::from(line.p[n][1] - line.p[at][1])]
            });
            let len = libm::hypot(d[0], d[1]).max(1e-9);
            let snorm = |v: f64| u32::from(libm::round((v / len).clamp(-1.0, 1.0) * 32767.0) as i16 as u16);
            let stop = !a.host && host.zip(sec(a.way)).is_some_and(|(h, g)| stops(&rules.plan, h, g));
            let flags = if a.host { ARM_HOST } else { 0 } | if stop { ARM_STOP } else { 0 };
            crate::probe::field(crate::probe::Field::Span, (at - lo).max(last - lo));
            w.extend([r | flags, (at - lo) as u32 | ((last - lo) as u32) << 16, snorm(d[0]) | snorm(d[1]) << 16]);
        }
        let n = (w.len() - JR) / JR_ARM;
        crate::probe::field(crate::probe::Field::JunctionArms, n);
        crate::probe::field(crate::probe::Field::JunctionId, id as usize);
        assert!(n < 1 << 8 && id < 1 << 16, "a junction record overflows its words");
        w[2] = n as u32 | id << 16;
        w
    };
    let junctions: Vec<Vec<u32>> = named.iter().map(|&id| junction_words(id)).collect();
    // Centre-line segments within their half-width and LINE_PAD_M of a bin.
    let mut segs: Vec<Vec<u32>> = vec![Vec::new(); n_bins];
    let pad = units(LINE_PAD_M);
    let seg_box_of = |a: P, b: P| [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])].map(f64::from);
    for (li, &(l, lo, hi)) in rec.iter().enumerate() {
        let line = &lines[l];
        for i in 0..hi - lo {
            let (a, b) = (line.p[lo + i], line.p[lo + i + 1]);
            let reach = line.h[lo + i].max(line.h[lo + i + 1]) + pad;
            let Some((c, r)) = bins_of(seg_box_of(a, b), reach) else { continue };
            crate::probe::field(crate::probe::Field::Line, li);
            crate::probe::field(crate::probe::Field::Sample, i);
            for row in r[0]..=r[1] {
                for col in c[0]..=c[1] {
                    if clip(a, b, square(col, row, reach)).is_some() {
                        segs[(row * BINS + col) as usize].push(li as u32 | (i as u32) << 16);
                    }
                }
            }
        }
    }
    // The paint reads a label's host lines, its other lines and its material line at their segment
    // nearest the point: in every bin that a point's, region's or round's label reaches, each set's
    // segments that can be nearest to a point of the bin. A segment farther from the bin than the
    // set's nearest one plus the bin's diagonal is nearer to no point of it.
    let diag = f64::from(BIN_UNITS) * std::f64::consts::SQRT_2;
    for b in 0..n_bins {
        let mut sets: Vec<[u32; 4]> = Vec::new();
        let mut at = 0;
        while at < items[b].len() {
            let ci = (items[b][at] & 0xffff) as usize;
            at = skip_item(&items[b], at);
            if !cands.get(ci).is_some_and(|c| c.label.owner != Owner::Terrain && kind(c) != WAY) {
                continue;
            }
            for &l in &given[ci] {
                let own = [labels[l as usize][2], u32::MAX, u32::MAX, u32::MAX];
                // A junction's frames search the bin for the lines of its record's arms: its hosts and its guests.
                let [h, o] = if (labels[l as usize][0] >> 8) & 0xff == JUNCTION {
                    let j = &graph.junctions[labels[l as usize][1] as usize];
                    let lines = |host: bool| -> Vec<u32> {
                        j.arms.iter().filter(|a| a.host == host).map(|a| arm_line(a, j.pos)).collect()
                    };
                    [four(lines(true)), four(lines(false))]
                } else {
                    [hosts[l as usize], others[l as usize]]
                };
                for h in [h, o, own] {
                    if h[0] != u32::MAX && !sets.contains(&h) {
                        sets.push(h);
                    }
                }
            }
        }
        let (col, row) = (b as i32 % BINS, b as i32 / BINS);
        let bx = square(col, row, 0.0);
        let mut wanted: Vec<u32> = Vec::new();
        for h in sets {
            let found: Vec<(u32, usize, f64)> = (h.iter().filter(|&&l| l != u32::MAX))
                .flat_map(|&l| {
                    let p = rec_points(l as usize);
                    (0..p.len().saturating_sub(1)).map(move |i| (l, i, seg_box(p[i], p[i + 1], bx)))
                })
                .collect();
            let near = found.iter().map(|s| s.2).fold(f64::INFINITY, f64::min);
            for &(l, i, _) in found.iter().filter(|s| s.2 <= near + diag) {
                crate::probe::field(crate::probe::Field::Line, l as usize);
                crate::probe::field(crate::probe::Field::Sample, i);
            }
            wanted.extend(found.iter().filter(|s| s.2 <= near + diag).map(|&(l, i, _)| l | (i as u32) << 16));
        }
        wanted.retain(|w| !segs[b].contains(w));
        wanted.sort_unstable();
        wanted.dedup();
        segs[b].append(&mut wanted);
    }
    // The heights' segments within their reach and HEIGHT_PAD_M of a bin, and the discs whose circle
    // grown by the greatest such reach meets a bin. A segment's reach is the one that the heights read:
    // its half-width, the class's shoulder and verge, and the class's slope reach or the least feather,
    // whatever a taper narrows the bands to.
    let mut hsegs: Vec<Vec<u32>> = vec![Vec::new(); window.len()];
    let mut hdiscs: Vec<Vec<u32>> = vec![Vec::new(); window.len()];
    let hpad = units(HEIGHT_PAD_M);
    let mut reach_max = 0.0f64;
    for (li, &(l, lo, hi)) in rec.iter().enumerate() {
        let line = &lines[l];
        let sec = &line.sec;
        let side = units(sec.shoulder_m + sec.verge_m + sec.reach_m.max(rules.plan.heights.feather_min_m));
        for i in 0..hi - lo {
            let (a, b) = (line.p[lo + i], line.p[lo + i + 1]);
            let reach = line.h[lo + i].max(line.h[lo + i + 1]) + side + hpad;
            reach_max = reach_max.max(reach);
            let Some((c, r)) = bins_in(seg_box_of(a, b), reach, window.wide) else { continue };
            crate::probe::field(crate::probe::Field::Line, li);
            crate::probe::field(crate::probe::Field::Sample, i);
            for row in r[0]..=r[1] {
                for col in c[0]..=c[1] {
                    if clip(a, b, square(col, row, reach)).is_some() {
                        hsegs[window.at(col, row)].push(li as u32 | (i as u32) << 16);
                    }
                }
            }
        }
    }
    for (ci, cand) in cands.iter().enumerate() {
        for (di, d) in cand.discs.iter().enumerate() {
            let (x, y, reach) = (f64::from(d.centre[0]), f64::from(d.centre[1]), d.radius + reach_max);
            let Some((c, r)) = bins_in([x, y, x, y], reach, window.wide) else { continue };
            crate::probe::field(crate::probe::Field::Candidate, ci);
            crate::probe::field(crate::probe::Field::Disc, di);
            for row in r[0]..=r[1] {
                for col in c[0]..=c[1] {
                    let bx = square(col, row, 0.0);
                    let (dx, dy) = ((bx[0] - x).max(x - bx[2]).max(0.0), (bx[1] - y).max(y - bx[3]).max(0.0));
                    if libm::hypot(dx, dy) <= reach {
                        hdiscs[window.at(col, row)].push(ci as u32 | (di as u32) << 16);
                    }
                }
            }
        }
    }
    let mut words = vec![0u32; HEAD];
    words[0] = MAGIC;
    words[1] = shapes.len() as u32;
    words[2] = labels.len() as u32;
    words[3] = rec.len() as u32;
    words[10] = (origin.y / CELL_UNITS) as u32;
    words.extend(rules.plan.heights.words());
    words[4] = words.len() as u32;
    words.extend(&cands_w);
    words[5] = words.len() as u32;
    for ((l, h), o) in labels.iter().zip(&hosts).zip(&others) {
        let mut l = *l;
        if (l[0] >> 8) & 0xff == JUNCTION {
            l[1] = named.iter().position(|&j| j == l[1]).unwrap() as u32;
        }
        words.extend(l.iter().chain(h).chain(o));
    }
    words[6] = words.len() as u32;
    words.extend(&lines_w);
    words[7] = words.len() as u32;
    words.extend(&samples_w);
    words[8] = words.len() as u32;
    words.extend(&extras);
    words[12] = words.len() as u32;
    words[13] = junctions.len() as u32;
    let mut at = words.len() + junctions.len();
    for j in &junctions {
        words.push(at as u32);
        at += j.len();
    }
    words.extend(junctions.iter().flatten());
    // The blend edges that a bin of the cell lists first: the records keep only those, and the build's
    // window bins read the rest.
    let n_blend = blend.len() / BLEND;
    let mut listed = vec![false; n_blend];
    for b in 0..n_bins {
        for &k in &bedges[window.at(b as i32 % BINS, b as i32 / BINS)] {
            listed[k as usize] = true;
        }
    }
    let mut order: Vec<usize> = (0..n_blend).collect();
    order.sort_by_key(|&k| !listed[k]);
    let mut renumber = vec![0u32; n_blend];
    for (to, &k) in order.iter().enumerate() {
        renumber[k] = to as u32;
    }
    for list in &mut bedges {
        list.iter_mut().for_each(|k| *k = renumber[*k as usize]);
    }
    words[11] = words.len() as u32;
    words.extend(order.iter().flat_map(|&k| &blend[BLEND * k..BLEND * (k + 1)]));
    // The outline edges: the near level's from its plan and the block so far, which the paved field
    // reads.
    let edges = if with_outline {
        outline_edges(&words, input, rules, &rims, &|q| {
            let k = window.of(q);
            Items { segs: &hsegs[k], discs: &hdiscs[k], blend: &bedges[k] }
        })
    } else {
        Vec::new()
    };
    words.truncate(words[11] as usize + BLEND * listed.iter().filter(|&&l| l).count());
    words[14] = words.len() as u32;
    words.extend(&edges);
    // Per bin the outline edges within their host's reach of its square, by host line. A block with no
    // outline edges lists none.
    let oedges = outline_bins(cell, &edges, &|l| lines[rec[l as usize].0].sec.reach_m);
    words[9] = words.len() as u32;
    let heads = words.len();
    words.resize(heads + HEADER * n_bins, 0);
    for b in 0..n_bins {
        let w = window.at(b as i32 % BINS, b as i32 / BINS);
        if items[b].is_empty()
            && segs[b].is_empty()
            && hsegs[w].is_empty()
            && hdiscs[w].is_empty()
            && bedges[w].is_empty()
            && oedges[b].is_empty()
        {
            continue;
        }
        let before = words.len();
        {
            use crate::probe::{field, Field};
            field(Field::BinCandidates, n_items[b] as usize);
            field(Field::BinSegments, segs[b].len());
            field(Field::HeightSegments, hsegs[w].len());
            field(Field::HeightDiscs, hdiscs[w].len());
            field(Field::BinItems, items[b].len());
            field(Field::BlendEdges, bedges[w].len());
        }
        words[heads + HEADER * b] = before as u32;
        words[heads + HEADER * b + 1] = n_items[b] << 16 | segs[b].len() as u32;
        words[heads + HEADER * b + 2] = (hsegs[w].len() as u32) << 16 | hdiscs[w].len() as u32;
        // The words of the items let a reader find the segments without walking the items.
        assert!(
            items[b].len() < 1 << 16 && bedges[w].len() < 1 << 16,
            "a bin's items or blend edges overflow its header"
        );
        words[heads + HEADER * b + 3] = (items[b].len() as u32) << 16 | bedges[w].len() as u32;
        words.extend(items[b].iter().chain(&segs[b]).chain(&hsegs[w]).chain(&hdiscs[w]).chain(&bedges[w]));
        // The count of its outline edges, then the edges: half words, the first in the low half.
        if !edges.is_empty() {
            assert!(oedges[b].len() < 1 << 16, "a bin's outline edges overflow a half word");
            let half: Vec<u32> = std::iter::once(oedges[b].len() as u32).chain(oedges[b].iter().copied()).collect();
            words.extend(half.chunks(2).map(|k| k[0] | k.get(1).map_or(0, |&x| x << 16)));
        }
        stats.bins_used += 1;
        stats.max_bin_words = stats.max_bin_words.max((words.len() - before) as u32);
    }
    stats.cands = shapes.len() as u32;
    stats.labels = labels.len() as u32;
    stats.lines = rec.len() as u32;
    stats.samples = (samples_w.len() / SAMPLE) as u32;
    Records { cell, words, stats }
}

/// An edge of a blend face: its ends in frame units from the cell's origin, with the blend face on
/// its left, the blend face's label and the label of the face beyond it (None for terrain).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlendEdge {
    pub a: [i32; 2],
    pub b: [i32; 2],
    pub region: u32,
    pub other: Option<u32>,
}

/// The blend edges that q's bin lists (q in the closed cell square, as `find`): every one within
/// `MARGIN_M` of the bin.
pub fn blend_edges(words: &[u32], q: [f64; 2]) -> impl Iterator<Item = BlendEdge> + '_ {
    let cell = f64::from(CELL_UNITS as i32);
    let inside = (0.0..=cell).contains(&q[0]) && (0.0..=cell).contains(&q[1]);
    let bin = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).clamp(0, BINS - 1);
    let head = words[9] as usize + HEADER * (bin(q[1]) * BINS + bin(q[0])) as usize;
    let (start, n) = if inside { blend_items(words, head) } else { (0, 0) };
    words[start..start + n].iter().map(move |&k| blend_edge(words, k))
}

/// Blend edge k of a block.
pub fn blend_edge(words: &[u32], k: u32) -> BlendEdge {
    let e = &words[words[11] as usize + BLEND * k as usize..][..BLEND];
    BlendEdge {
        a: [e[0] as i32, e[1] as i32],
        b: [e[2] as i32, e[3] as i32],
        region: e[4],
        other: (e[5] != u32::MAX).then_some(e[5]),
    }
}

/// The start and the count of the blend edge items of the bin whose header is at `head`.
fn blend_items(words: &[u32], head: usize) -> (usize, usize) {
    let h = words[head + 2];
    let at = words[head] as usize + (words[head + 3] >> 16) as usize;
    let at = at + (words[head + 1] & 0xffff) as usize + (h >> 16) as usize + (h & 0xffff) as usize;
    (at, (words[head + 3] & 0xffff) as usize)
}

/// The words of label `id` of a block.
pub fn label(words: &[u32], id: u32) -> &[u32] {
    &words[words[5] as usize + LABEL * id as usize..][..LABEL]
}

/// The junction record of label `id` (`JR`), none for a label that no junction owns.
pub fn junction(words: &[u32], id: u32) -> Option<&[u32]> {
    let l = label(words, id);
    if (l[0] >> 8) & 0xff != JUNCTION {
        return None;
    }
    let at = words[words[12] as usize + l[1] as usize] as usize;
    Some(&words[at..at + JR + JR_ARM * (words[at + 2] & 0xff) as usize])
}

/// The graph's id of the owner of label `id`: a junction's from its record.
pub fn owner(words: &[u32], id: u32) -> u32 {
    junction(words, id).map_or(label(words, id)[1], |j| j[2] >> 16)
}

fn cand(words: &[u32], ci: u32) -> &[u32] {
    &words[words[4] as usize + CAND * ci as usize..][..CAND]
}

/// The word after the candidate item at `at`.
pub(crate) fn skip_item(words: &[u32], mut at: usize) -> usize {
    let runs = words[at + 1];
    at += 2;
    for _ in 0..runs {
        at += 1 + 2 * words[at] as usize;
    }
    at
}

fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}

/// Whether point p moved by (ε, ε²) lies left of the line through a along e.
fn left(e: [f64; 2], a: [f64; 2], p: [f64; 2]) -> bool {
    let s = cross(e, [p[0] - a[0], p[1] - a[1]]);
    if s != 0.0 {
        return s > 0.0;
    }
    e[1] < 0.0 || (e[1] == 0.0 && e[0] > 0.0)
}

/// The change of the winding number from c to q, both moved by (ε, ε²), across edge ab: +1 where q
/// enters the side on the edge's left. In doubled units.
fn crossing(a: [f64; 2], b: [f64; 2], c: [f64; 2], q: [f64; 2]) -> i32 {
    let e = [b[0] - a[0], b[1] - a[1]];
    let (sc, sq) = (left(e, a, c), left(e, a, q));
    if sc == sq {
        return 0;
    }
    // The ends of the edge against the moved segment: v − (c + (ε, ε²)) across d.
    let d = [q[0] - c[0], q[1] - c[1]];
    let side = |v: [f64; 2]| {
        let s = cross(d, [v[0] - c[0], v[1] - c[1]]);
        if s != 0.0 {
            s > 0.0
        } else {
            d[1] > 0.0 || (d[1] == 0.0 && d[0] < 0.0)
        }
    };
    if side(a) == side(b) {
        0
    } else if sq {
        1
    } else {
        -1
    }
}

/// Whether q lies past a cut: on its right, within its length, as the overlay's `past`.
fn past(cut: &[u32], q: [f64; 2]) -> bool {
    let [a0, a1, b0, b1] = [0, 1, 2, 3].map(|k| f64::from(cut[k] as i32));
    let (u, v) = ([b0 - a0, b1 - a1], [q[0] - a0, q[1] - a1]);
    let along = u[0] * v[0] + u[1] * v[1];
    cross(u, v) < 0.0 && (0.0..=u[0] * u[0] + u[1] * u[1]).contains(&along)
}

/// The label of a block at point q of the closed cell square (frame units from the cell's origin),
/// none for terrain: the overlay's decision over the candidates that cover q. A point on an edge
/// reads as moved by (ε, ε²) in frame axes, so every cell breaks a tie alike; the tests are exact
/// for q on a grid of 1/1024 unit. A fan or round leaves out the disc of its circle: its arc is the
/// true circle where the overlay's is its chords.
pub fn find(words: &[u32], q: [f64; 2]) -> Option<u32> {
    let cell = f64::from(CELL_UNITS as i32);
    if !(0.0..=cell).contains(&q[0]) || !(0.0..=cell).contains(&q[1]) {
        return None;
    }
    let bin = |v: f64| ((v / f64::from(BIN_UNITS)).floor() as i32).min(BINS - 1);
    let (col, row) = (bin(q[0]), bin(q[1]));
    let head = words[9] as usize + HEADER * (row * BINS + col) as usize;
    let (mut at, n_cands, n_segs) = (words[head] as usize, (words[head + 1] >> 16) as usize, words[head + 1] & 0xffff);
    let c = [2.0 * f64::from(reference(col)) + 1.0, 2.0 * f64::from(reference(row)) + 1.0];
    let q2 = [2.0 * q[0], 2.0 * q[1]];
    let extras = &words[words[8] as usize..];
    let mut covered = Vec::new();
    for _ in 0..n_cands {
        let ci = words[at] & 0xffff;
        let mut w = ((words[at] >> 16) & 0xff) as i32 - 128;
        let runs = words[at + 1];
        at += 2;
        for _ in 0..runs {
            let n = words[at] as usize;
            let v = |k: usize| {
                [2.0 * f64::from(words[at + 1 + 2 * k] as i32), 2.0 * f64::from(words[at + 2 + 2 * k] as i32)]
            };
            for k in 0..n.saturating_sub(1) {
                w += crossing(v(k), v(k + 1), c, q2);
            }
            at += 1 + 2 * n;
        }
        let k = cand(words, ci);
        let in_disc = (0..k[6] as usize).any(|d| {
            let e = &extras[k[5] as usize + DISC * d..];
            libm::hypot(q[0] - f64::from(e[0] as i32), q[1] - f64::from(e[1] as i32)) < f64::from(f32::from_bits(e[2]))
        });
        if w != 0 && !in_disc {
            covered.push(ci);
        }
    }
    decide(words, &covered, q, &words[at..at + n_segs as usize])
}

/// The glob order of a line at q: of its sample nearest q among the bin's segments.
fn way_glob(words: &[u32], line: u32, q: [f64; 2], segs: &[u32]) -> u32 {
    let first = words[words[6] as usize + LINE * line as usize + 2] as usize;
    let mut best = (f64::INFINITY, u32::MAX);
    for &s in segs.iter().filter(|&&s| s & 0xffff == line) {
        for i in [(s >> 16) as usize, (s >> 16) as usize + 1] {
            let w = &words[words[7] as usize + SAMPLE * (first + i)..];
            let (dx, dy) = (q[0] - f64::from(w[0] as i32), q[1] - f64::from(w[1] as i32));
            if dx * dx + dy * dy < best.0 {
                best = (dx * dx + dy * dy, w[4]);
            }
        }
    }
    best.1
}

fn decide(words: &[u32], covered: &[u32], q: [f64; 2], segs: &[u32]) -> Option<u32> {
    let extras = &words[words[8] as usize..];
    let glob = |ci: u32| {
        let k = cand(words, ci);
        if k[0] == WAY {
            way_glob(words, k[4], q, segs)
        } else {
            k[2]
        }
    };
    let rest: Vec<u32> = covered.iter().copied().filter(|&c| !matches!(cand(words, c)[0], REGION | OUTLINE)).collect();
    let best = rest.iter().map(|&c| cand(words, c)[1]).min()?;
    let win = rest.iter().copied().filter(|&c| cand(words, c)[1] == best).min_by_key(|&c| glob(c))?;
    let k = cand(words, win);
    if k[0] == FILL {
        return Some(k[3]);
    }
    let across = (0..k[8] as usize).map(|b| &extras[k[7] as usize + BEYOND * b..][..BEYOND]).find(|e| past(e, q));
    let mut id = across.map_or(k[3], |e| e[4]);
    let l = label(words, id);
    if (l[0] >> 8) & 0xff == WAY_OWNER && across.is_none() {
        let w = l[1];
        let outside = |p: &[u32]| !(p[1] == 1 && past(&p[2..6], q)) && !(p[6] == 1 && past(&p[7..11], q));
        let region = (covered.iter().copied())
            .filter(|&c| cand(words, c)[0] == REGION)
            .filter(|&c| {
                let r = cand(words, c);
                let parts = (0..r[10] as usize).map(|p| &extras[r[9] as usize + PART * p..][..PART]);
                let mine: Vec<&[u32]> = parts.filter(|p| p[0] == w).collect();
                !mine.is_empty() && mine.iter().all(|p| outside(p))
            })
            .min_by_key(|&c| (cand(words, c)[1], cand(words, c)[2]));
        if let Some(r) = region {
            id = cand(words, r)[3];
        }
    }
    let l = label(words, id);
    let rail = rest.iter().find(|&&c| label(words, cand(words, c)[3])[0] & 0xff == Surface::RailBed as u32);
    if let (true, Some(&r)) = (l[0] & 0xff == Surface::Carriageway as u32, rail) {
        let want = [Surface::Crossing as u32 | (l[0] & !0xff), l[1], l[2], label(words, cand(words, r)[3])[2]];
        let found = (0..words[2]).find(|&x| label(words, x)[..4] == want);
        debug_assert!(found.is_some(), "no crossing variant of label {id} over a rail of the bin");
        if let Some(x) = found {
            id = x;
        }
    }
    Some(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_segments_of_one_line_are_cut_at_the_same_points_whatever_their_far_ends() {
        // From a point on a cell line, along one exact line, to two far ends; north-south and oblique.
        for (a, step, far) in [([1000, 0], [0, 1], [32222, 32768]), ([5, 40], [3, 7], [4600, 4681])] {
            let end = |n: i32| [a[0] + n * step[0], a[1] + n * step[1]];
            let (p, q) = (bin_cuts(a, end(far[0])), bin_cuts(a, end(far[1])));
            let shared = p.len().min(q.len()) - 1;
            assert!(shared >= 3, "{p:?}");
            assert_eq!(p[..shared], q[..shared]);
            // Each piece spans at most a bin.
            assert!(q.windows(2).all(|w| (0..2).all(|i| (w[1][i] - w[0][i]).abs() <= BIN_UNITS)), "{q:?}");
        }
    }

    #[test]
    fn a_guest_stops_where_its_look_is_not_the_hosts_or_its_finish_keeps_its_tread() {
        let sections = crate::sections::Sections::embedded();
        let plan = &Rules::embedded().plan;
        let way = |kind: &str, tags: &[(&str, &str)]| {
            sections.way(kind, &tags.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), false)
        };
        let road = way("road", &[("highway", "residential")]);
        let service = way("road", &[("highway", "service")]);
        let dirt = way("path", &[("highway", "path"), ("surface", "dirt")]);
        let gravel = way("road", &[("highway", "track"), ("surface", "gravel")]);
        let pavers = way("path", &[("highway", "path"), ("surface", "paving_stones")]);
        let cycleway = way("path", &[("highway", "cycleway"), ("surface", "asphalt")]);
        assert!(stops(plan, &road, &dirt) && stops(plan, &road, &gravel));
        // Paved guests of another look keep it too: pavers at a service road, asphalt at a gravel track.
        assert!(stops(plan, &service, &pavers) && stops(plan, &gravel, &cycleway));
        // A reach pair keeps the junction's look: asphalt at a gravel footway.
        let gravel_footway = way("path", &[("highway", "footway"), ("surface", "gravel")]);
        assert_eq!(plan.pair(gravel_footway.group, cycleway.group).mouth, Mouth::Reach);
        assert!(!stops(plan, &gravel_footway, &cycleway));
        // A paved guest of the host's look does not stop; a soil one keeps its tread (`tread_finishes`).
        let paved = way("road", &[("highway", "track"), ("surface", "asphalt")]);
        let gravel_path = way("path", &[("highway", "path"), ("surface", "gravel")]);
        assert!(!stops(plan, &road, &paved) && !stops(plan, &road, &road) && stops(plan, &gravel, &gravel_path));
        // Steps of another look stop at a road too, but their corners have no radius: no mouth.
        let steps = way("path", &[("highway", "steps"), ("surface", "dirt")]);
        assert_eq!(plan.pair(road.group, steps.group).mouth, Mouth::Stop);
        assert_eq!(steps.corner_radius_m.max(steps.corner_radius_walk_m), 0.0);
        assert!(!stops(plan, &road, &steps));
    }

    /// The angle of a unit vector, and the turn between two.
    fn angle(v: [f64; 2]) -> f64 {
        libm::atan2(v[1], v[0])
    }

    #[test]
    fn the_frame_turns_no_tighter_than_its_least_radius_and_keeps_ordinary_bends_square() {
        let least = 4000.0;
        // A U of radius 4,500 units between two legs, sampled at uneven angles, so that the polyline's
        // own mitre lines cross well within the least radius.
        let mut p: Vec<P> = (0..6).map(|k| [0, -20_000 + 4_000 * k]).collect();
        let mut a = 0.0f64;
        for step in [0.3, 0.05, 0.4, 0.1, 0.35, 0.05, 0.45, 0.2, 0.3, 0.1, 0.4, 0.2, 0.12] {
            a += step;
            p.push([(4_500.0 - 4_500.0 * libm::cos(a)) as i32, (4_500.0 * libm::sin(a)) as i32]);
        }
        p.extend((1..6).map(|k| [9_000, -4_000 * k]));
        let raw = mitres(&p, &vec![1.0; p.len()], 30.0);
        assert!(raw.iter().any(|m| m[2].min(m[3]) < 0.5 * least), "the polyline itself folds");
        let m = mitres(&p, &vec![least; p.len()], 30.0);
        for (i, w) in m.iter().enumerate() {
            assert!(w[2].min(w[3]) >= 0.99 * least, "vertex {i}: health radius {:?}", &w[2..]);
        }
        // The ends keep their chords, and a straight leg away from the bend stays square.
        let n = p.len();
        let chord = |i: usize| angle([f64::from(p[i + 1][0] - p[i][0]), f64::from(p[i + 1][1] - p[i][1])]);
        assert_eq!(angle([m[0][0], m[0][1]]), chord(0));
        assert!((angle([m[n - 1][0], m[n - 1][1]]) - chord(n - 2)).abs() < 1e-12);
        assert!((angle([m[1][0], m[1][1]]) - chord(0)).abs() < 1e-12);
        // An evenly sampled arc wider than the least radius stays square to the arc away from its ends.
        let arc: Vec<P> = (0..40)
            .map(|k| f64::from(k) * 0.05)
            .map(|a| [libm::round(9_000.0 * libm::cos(a)) as i32, libm::round(9_000.0 * libm::sin(a)) as i32])
            .collect();
        let m = mitres(&arc, &vec![least; arc.len()], 30.0);
        for (i, w) in m.iter().enumerate().take(arc.len() - 6).skip(6) {
            let square = f64::from(i as u32) * 0.05 + PI / 2.0;
            assert!(libm::remainder(angle([w[0], w[1]]) - square, 2.0 * PI).abs() < 1e-3, "vertex {i}");
        }
    }
}
