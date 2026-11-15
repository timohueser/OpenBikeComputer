//! One constrained Delaunay triangulation of D: a single bulk load of every point and constraint, no
//! refinement, then a flood fill that gives each triangle the face that holds it.
//!
//! Every point is an integer in frame units, exact in f64, and spade's predicates are exact, so
//! native and WASM builds give the same triangles.

use spade::handles::FixedVertexHandle;
use spade::{ConstrainedDelaunayTriangulation, Point2, Triangulation};

use super::points::P;
use super::MeshError;

type Cdt = ConstrainedDelaunayTriangulation<Point2<f64>>;

/// The side of a constraint that lies in D with no face named: D's border.
pub const INSIDE: u32 = u32::MAX - 1;
/// The side outside D, and a triangle outside D.
pub const OUTSIDE: u32 = crate::faces::OUTSIDE;
/// A triangle that no seed reached yet.
const UNSET: u32 = u32::MAX - 2;

/// D's triangles with their classes, and the count of face-edge pieces split at a point on them.
pub type Triangles = (Vec<([u32; 3], u32)>, u32);

/// A constraint from point a to point b, with the class of the triangles on each side: a face of the
/// plan, `INSIDE` or `OUTSIDE`. `left` is the side where the shoelace sum of (a, b, point) is
/// positive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub a: u32,
    pub b: u32,
    pub left: u32,
    pub right: u32,
}

/// The triangles of D: point indices turning as the terrain's `gridIndices` (negative signed area in
/// x east, y south), and the class of each: the face of a face edge that bounds its region, else
/// `INSIDE`. A face edge names the faces; D's border, given after them, only parts D from the rest.
/// Also the count of face-edge pieces that the triangulation split at a point on them.
pub fn triangulate(points: &[P], segments: &[Segment], edges: usize) -> Result<Triangles, MeshError> {
    let vertices: Vec<Point2<f64>> = points.iter().map(|p| Point2::new(f64::from(p[0]), f64::from(p[1]))).collect();
    let pairs: Vec<[usize; 2]> = segments.iter().filter(|s| s.a != s.b).map(|s| [s.a as usize, s.b as usize]).collect();
    let mut conflict = None;
    let cdt = Cdt::try_bulk_load_cdt(vertices, pairs, |e| {
        conflict.get_or_insert(e);
    })
    .map_err(|e| MeshError::InvalidPlan { at: points.first().copied().unwrap_or_default(), why: format!("{e:?}") })?;
    if let Some([a, b]) = conflict {
        return Err(MeshError::InvalidPlan {
            at: points[a],
            why: format!("constraint {:?}-{:?} crosses another", points[a], points[b]),
        });
    }
    let mut class = vec![UNSET; cdt.num_all_faces()];
    let mut stack = Vec::new();
    let mut split = 0;
    for (k, range) in [0..edges, edges..segments.len()].into_iter().enumerate() {
        for s in &segments[range] {
            let Some(e) = cdt.get_edge_from_neighbors(
                FixedVertexHandle::from_index(s.a as usize),
                FixedVertexHandle::from_index(s.b as usize),
            ) else {
                split += u32::from(k == 0 && s.a != s.b);
                continue;
            };
            for (face, c) in [(e.face(), s.left), (e.rev().face(), s.right)] {
                if let Some(f) = face.as_inner() {
                    if class[f.index()] == UNSET {
                        class[f.index()] = c;
                        stack.push(f.fix());
                    }
                }
            }
        }
        while let Some(f) = stack.pop() {
            let c = class[f.index()];
            for e in cdt.face(f).adjacent_edges() {
                if e.is_constraint_edge() {
                    continue;
                }
                if let Some(g) = e.rev().face().as_inner() {
                    if class[g.index()] == UNSET {
                        class[g.index()] = c;
                        stack.push(g.fix());
                    }
                }
            }
        }
    }
    let tris = (cdt.inner_faces())
        .filter(|f| !matches!(class[f.index()], OUTSIDE | UNSET))
        .map(|f| {
            let v = f.vertices().map(|v| v.fix().index() as u32);
            ([v[0], v[2], v[1]], class[f.index()])
        })
        .collect();
    Ok((tris, split))
}
