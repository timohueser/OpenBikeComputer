//! The mesh of a cell's faces: a constrained Delaunay triangulation of the owned region's interior in
//! two ranges, with locked cell borders, heights from the one road model's point function, a strip
//! that meets the terrain at every morph, and floors that keep the terrain under the mesh.

pub mod cdt;
pub mod lines;
pub mod morph;
pub mod params;
pub mod region;
pub mod vertices;

use crate::faces::{FacePlan, HeightRule};
use crate::frame::{metres_per_unit_at, CellKey, CELL_UNITS};

/// The two ranges of a cell's mesh: near the eye over level-18 nodes, and far over level-17 nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Range {
    Near,
    Far,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MeshError {
    /// The constraints cross, overlap or hold a point inside a segment: a fault in the faces or the
    /// two cells' plans at a border. The place in frame units from the cell's origin.
    InvalidPlan { at: [i32; 2], why: String },
    /// A strip hole that no fill and no growth solves.
    StripUnsolved { at: [i32; 2] },
}

/// The plans of a cell and its eight neighbours. A plan holds its own cell's faces; the mesh reads the
/// neighbours' for the owned region and the border points.
pub struct Window<'a> {
    pub cell: CellKey,
    /// Row by row from the north-west: index (dy + 1) × 3 + dx + 1.
    plans: [Option<&'a FacePlan>; 9],
}

impl<'a> Window<'a> {
    /// The window of the cell from any plans: each plan of the cell or a neighbour takes its place.
    pub fn new(cell: CellKey, plans: impl IntoIterator<Item = &'a FacePlan>) -> Window<'a> {
        let mut out = [None; 9];
        for plan in plans {
            let (dx, dy) = (i64::from(plan.cell.x) - i64::from(cell.x), i64::from(plan.cell.y) - i64::from(cell.y));
            if dx.abs() <= 1 && dy.abs() <= 1 {
                out[((dy + 1) * 3 + dx + 1) as usize] = Some(plan);
            }
        }
        Window { cell, plans: out }
    }

    /// The cell's own plan.
    pub fn centre(&self) -> Option<&'a FacePlan> {
        self.plans[4]
    }

    /// Each plan with the shift of its vertices into the cell's frame.
    pub fn plans(&self) -> impl Iterator<Item = ([i32; 2], &'a FacePlan)> + '_ {
        let c = CELL_UNITS as i32;
        (0..9).filter_map(move |k| self.plans[k].map(|p| ([(k as i32 % 3 - 1) * c, (k as i32 / 3 - 1) * c], p)))
    }

    /// Metres per frame unit at a row of the cell's frame: global, so two cells agree.
    pub fn mpu(&self, y: f64) -> f64 {
        metres_per_unit_at(self.cell.origin().y as f64 + y)
    }
}

/// Whether a face's height rule makes it owned: every rule but terrain.
pub fn owned(rule: HeightRule) -> bool {
    rule != HeightRule::Terrain
}

use crate::faces::heights::{height_at_sides, RULE_FIELDS};
use crate::faces::records::HEAD;
use crate::faces::OUTSIDE;
use crate::terrain::Terrain;
use cdt::{Constraint, FaceInfo, Triangles};
use region::Region;
use vertices::{face_labels, Heights};

/// What a cell's mesh build gives: `near: false` (the finest level 17) builds the far range only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeshOptions {
    pub near: bool,
    /// The least angle of refinement: `params::MIN_ANGLE_DEG`. Measurements lower it.
    pub min_angle_deg: f64,
}

impl Default for MeshOptions {
    fn default() -> Self {
        MeshOptions { near: true, min_angle_deg: params::MIN_ANGLE_DEG }
    }
}

/// One range's triangulation and its heights.
pub struct RangeMesh {
    pub range: Range,
    pub tris: Triangles,
    pub heights: Heights,
}

/// The stages of `build` in their order: `Built::times` holds one time per stage.
pub const STAGES: [&str; 5] = ["region", "far", "far heights", "near", "near heights"];

pub struct Built {
    pub region: Region,
    pub ranges: Vec<RangeMesh>,
    pub labels: Vec<Option<u32>>,
    /// Face edges of the plan outside D: none by construction.
    pub outside_d: u32,
    pub times: Vec<f64>,
}

/// The join of two sides' heights: GP6's `step_m`, from the block's rules.
pub fn join_m(words: &[u32]) -> f64 {
    let k = RULE_FIELDS.iter().position(|f| *f == "step_m").expect("heights.step_m");
    f64::from(f32::from_bits(words[HEAD + k]))
}

/// Even-odd: whether a point lies inside a face's rings.
fn inside(plan: &FacePlan, face: usize, q: [f64; 2]) -> bool {
    let mut odd = false;
    for ring in &plan.faces[face].rings {
        for k in 0..ring.len() {
            let (a, b) = (plan.vertices[ring[k] as usize], plan.vertices[ring[(k + 1) % ring.len()] as usize]);
            let (a, b) = ([f64::from(a[0]), f64::from(a[1])], [f64::from(b[0]), f64::from(b[1])]);
            if (a[1] <= q[1]) != (b[1] <= q[1]) && q[0] < a[0] + (q[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]) {
                odd = !odd;
            }
        }
    }
    odd
}

/// The constraints of the cell's D: every face edge between two faces, and D's border with the
/// border spans split by `lines`. Also the count of face edges outside D.
fn constraints(window: &Window, region: &Region) -> (Vec<Constraint>, u32) {
    let plan = window.centre().expect("the cell's plan");
    let mut out = Vec::new();
    let mut outside_d = 0;
    for e in plan.edges.iter().filter(|e| e.left != OUTSIDE && e.right != OUTSIDE) {
        let (a, b) = (plan.vertices[e.a as usize], plan.vertices[e.b as usize]);
        let both_terrain = [e.left, e.right].iter().all(|&f| !owned(plan.faces[f as usize].rule));
        if both_terrain {
            continue;
        }
        let mid = [i64::from(a[0]) + i64::from(b[0]), i64::from(a[1]) + i64::from(b[1])].map(|v| v.div_euclid(2));
        if !region.in_d(mid) {
            outside_d += 1;
            continue;
        }
        out.push(Constraint { a, b, left: e.left, right: e.right, locked: false });
    }
    // The faces along the cell lines: the plan's border edges.
    let rims: Vec<([i32; 2], [i32; 2], u32)> = (plan.edges.iter())
        .filter(|e| (e.left == OUTSIDE) != (e.right == OUTSIDE))
        .map(|e| {
            (plan.vertices[e.a as usize], plan.vertices[e.b as usize], if e.left == OUTSIDE { e.right } else { e.left })
        })
        .collect();
    let border = lines::Border::new(window);
    let terrain: Vec<usize> = (0..plan.faces.len()).filter(|&f| !owned(plan.faces[f].rule)).collect();
    for bound in region.bounds() {
        let mid = [f64::from(bound.a[0] + bound.b[0]) / 2.0, f64::from(bound.a[1] + bound.b[1]) / 2.0];
        let points = if bound.border { border.split(bound.a, bound.b) } else { vec![bound.a, bound.b] };
        for w in points.windows(2) {
            let m = [f64::from(w[0][0] + w[1][0]) / 2.0, f64::from(w[0][1] + w[1][1]) / 2.0];
            let face = if bound.border {
                rims.iter()
                    .find(|(a, b, _)| {
                        let on = |p: [i32; 2]| {
                            let (lo, hi) = ([a[0].min(b[0]), a[1].min(b[1])], [a[0].max(b[0]), a[1].max(b[1])]);
                            (lo[0]..=hi[0]).contains(&p[0]) && (lo[1]..=hi[1]).contains(&p[1])
                        };
                        (a[0] == b[0] || a[1] == b[1]) && on(w[0]) && on(w[1])
                    })
                    .map(|r| r.2)
            } else {
                // The inner border lies in terrain: nudge toward D (the left: shoelace positive).
                let d = [bound.b[0] - bound.a[0], bound.b[1] - bound.a[1]].map(|v| f64::from(v.signum()));
                let q = [m[0] - d[1] * 0.25, m[1] + d[0] * 0.25];
                terrain.iter().copied().find(|&f| inside(plan, f, q)).map(|f| f as u32)
            };
            let face = face
                .unwrap_or_else(|| (0..plan.faces.len()).find(|&f| inside(plan, f, mid)).map_or(OUTSIDE, |f| f as u32));
            out.push(Constraint { a: w[0], b: w[1], left: face, right: OUTSIDE, locked: true });
        }
    }
    (out, outside_d)
}

/// The triangulations and heights of a cell: what the mesh stages build so far. `now` gives
/// milliseconds; no output depends on it.
pub fn build(
    window: &Window,
    words: &[u32],
    terrain: &Terrain,
    options: MeshOptions,
    now: &mut dyn FnMut() -> f64,
) -> Result<Built, MeshError> {
    let mut times = Vec::with_capacity(STAGES.len());
    let mut last = now();
    let mut lap = |times: &mut Vec<f64>| {
        let t = now();
        times.push(t - last);
        last = t;
    };
    let plan = window.centre().expect("the cell's plan");
    let region = Region::new(window, |_| true)?;
    let (constraints, outside_d) = constraints(window, &region);
    let labels = face_labels(&plan.faces, words);
    let faces: Vec<FaceInfo> = plan.faces.iter().map(|f| FaceInfo { surface: f.surface, rule: f.rule }).collect();
    let rank: Vec<u8> = plan.faces.iter().map(|f| f.rank).collect();
    let join = join_m(words);
    let cell = window.cell;
    let mpu = |y: f64| window.mpu(y);
    lap(&mut times);
    let mut ranges = Vec::new();
    for range in [Range::Far, Range::Near] {
        if range == Range::Near && !options.near {
            times.extend([0.0, 0.0]);
            continue;
        }
        let input = cdt::Input {
            faces: &faces,
            constraints: constraints.clone(),
            region: &region,
            mpu: &mpu,
            join_m: join,
            min_angle_deg: options.min_angle_deg,
        };
        let mut steps = |p: [i32; 2], l: u32, r: u32| {
            let t = vertices_terrain(terrain, cell, p)?;
            let h =
                height_at_sides(words, [f64::from(p[0]), f64::from(p[1])], [labels[l as usize], labels[r as usize]], t);
            Some(h[0].h - h[1].h)
        };
        let tris = cdt::triangulate(&input, range, &mut steps)?;
        lap(&mut times);
        let heights = vertices::heights(&tris, &labels, &rank, words, terrain, cell, range, join, &mpu);
        lap(&mut times);
        ranges.push(RangeMesh { range, tris, heights });
    }
    Ok(Built { region, ranges, labels, outside_d, times })
}

fn vertices_terrain(terrain: &Terrain, cell: CellKey, p: [i32; 2]) -> Option<f64> {
    let o = cell.origin();
    terrain.height(crate::frame::GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) })
}
