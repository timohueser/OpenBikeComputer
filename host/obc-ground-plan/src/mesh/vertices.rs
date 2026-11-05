//! The heights and normals of a triangulation's vertices: each face at a vertex takes its height from
//! the point function with its own label (`height_at`, or `height_at_sides` across a face edge), the
//! faces whose heights meet within the join form a group of one height, and each face's normal comes
//! from differences of the function with its label, one-sided into the face at an edge.

use std::collections::BTreeMap;

use super::cdt::Triangles;
use super::params::normal_step_m;
use super::Range;
use crate::faces::heights::{height_at, height_at_sides, Height, Step};
use crate::faces::{Face, OUTSIDE};
use crate::frame::{CellKey, GlobalPoint};
use crate::terrain::Terrain;

/// A face's vertex at a triangulation point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corner {
    pub point: u32,
    pub face: u32,
    /// The face's own height at the point, and its step's rule.
    pub own: Height,
    /// The height of the face's group: the height of its face of the highest overlay priority.
    pub h: f64,
    pub group: u32,
    /// Octahedral normal (east, north, up) as snorm16.
    pub normal: [i16; 2],
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct HeightStats {
    pub inside_calls: u64,
    pub sides_calls: u64,
    pub normal_calls: u64,
    /// Face vertices whose face is thinner than the normal's step on both sides of an axis.
    pub thin_normals: u32,
    /// Points with no known terrain.
    pub unknown: u32,
}

pub struct Heights {
    pub corners: Vec<Corner>,
    /// Per (point, face): the index of its corner.
    pub at: BTreeMap<(u32, u32), u32>,
    pub stats: HeightStats,
}

/// The labels of a plan's faces in a block: the first label whose words match the face's (surface,
/// owner, rank, rule, material and crossing lines); none for terrain.
pub fn face_labels(faces: &[Face], words: &[u32]) -> Vec<Option<u32>> {
    use crate::faces::records::{JUNCTION, LABEL, LINE, PLATFORM, TERRAIN, WAY_OWNER};
    use crate::faces::{Owner, Surface};
    let (labels, lines) = (words[2] as usize, words[3] as usize);
    let line_of = |way: Option<u32>| {
        way.map_or(u32::MAX, |w| {
            (0..lines).find(|&l| words[words[6] as usize + LINE * l] == w).map_or(u32::MAX, |l| l as u32)
        })
    };
    faces
        .iter()
        .map(|f| {
            if f.surface == Surface::Terrain {
                return None;
            }
            let (kind, id) = match f.owner {
                Owner::Terrain => (TERRAIN, 0),
                Owner::Way(w) => (WAY_OWNER, w),
                Owner::Junction(j) => (JUNCTION, j),
                Owner::Platform(p) => (PLATFORM, p),
            };
            let w = [
                f.surface as u32 | kind << 8 | u32::from(f.rank) << 16 | (f.rule as u32) << 24,
                id,
                line_of(f.material),
                line_of(f.crossing),
            ];
            (0..labels as u32).find(|&l| words[words[5] as usize + LABEL * l as usize..][..4] == w)
        })
        .collect()
}

/// The plain terrain at a point of the cell's frame.
fn terrain_at(terrain: &Terrain, cell: CellKey, p: [f64; 2]) -> Option<f64> {
    let o = cell.origin();
    terrain.height(GlobalPoint { x: o.x + p[0].round() as i64, y: o.y + p[1].round() as i64 })
}

/// The triangles about each point and each edge's triangles.
struct Mesh<'a> {
    t: &'a Triangles,
    about: Vec<Vec<u32>>,
    edge: BTreeMap<(u32, u32), Vec<u32>>,
    constraint: BTreeMap<(u32, u32), (u32, u32)>,
}

impl<'a> Mesh<'a> {
    fn new(t: &'a Triangles) -> Mesh<'a> {
        let mut about = vec![Vec::new(); t.points.len()];
        let mut edge: BTreeMap<(u32, u32), Vec<u32>> = BTreeMap::new();
        for (k, (v, _)) in t.tris.iter().enumerate() {
            for i in 0..3 {
                about[v[i] as usize].push(k as u32);
                let (a, b) = (v[i], v[(i + 1) % 3]);
                edge.entry((a.min(b), a.max(b))).or_default().push(k as u32);
            }
        }
        let constraint = t.segments.iter().map(|s| ((s.a.min(s.b), s.a.max(s.b)), (s.left, s.right))).collect();
        Mesh { t, about, edge, constraint }
    }

    fn pt(&self, v: u32) -> [f64; 2] {
        let p = self.t.points[v as usize];
        [f64::from(p[0]), f64::from(p[1])]
    }

    /// Whether the point at distance s from vertex v along d lies in face f (closed): a walk through
    /// the triangles from v's triangle of f that holds the direction, stopping at a constraint.
    fn holds(&self, v: u32, f: u32, d: [f64; 2], s: f64) -> bool {
        let o = self.pt(v);
        let q = [o[0] + d[0] * s, o[1] + d[1] * s];
        let cr = |a: [f64; 2], b: [f64; 2], p: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
        // The triangle of f at v whose sector holds d (the triangles turn negative: x east, y south).
        let start = self.about[v as usize].iter().copied().find(|&k| {
            let (tri, face) = self.t.tris[k as usize];
            if face != f {
                return false;
            }
            let i = tri.iter().position(|&x| x == v).unwrap();
            let (a, b) = (self.pt(tri[(i + 1) % 3]), self.pt(tri[(i + 2) % 3]));
            cr(o, a, q) <= 0.0 && cr(o, b, q) >= 0.0
        });
        let Some(mut k) = start else { return false };
        let mut from = v;
        for _ in 0..64 {
            let (tri, face) = self.t.tris[k as usize];
            if face != f {
                return false;
            }
            let p = tri.map(|x| self.pt(x));
            if (0..3).all(|i| cr(p[i], p[(i + 1) % 3], q) <= 0.0) {
                return true;
            }
            // The edge that the ray leaves through: one that does not hold `from`, with q beyond it.
            let exit = (0..3).find(|&i| {
                let (a, b) = (tri[i], tri[(i + 1) % 3]);
                a != from && b != from && cr(p[i], p[(i + 1) % 3], q) > 0.0
            });
            let Some(i) = exit.or_else(|| (0..3).find(|&i| cr(p[i], p[(i + 1) % 3], q) > 0.0)) else { return true };
            let (a, b) = (tri[i], tri[(i + 1) % 3]);
            if self.constraint.contains_key(&(a.min(b), a.max(b))) {
                return false;
            }
            let Some(&next) = self.edge[&(a.min(b), a.max(b))].iter().find(|&&x| x != k) else { return false };
            from = tri[(i + 2) % 3];
            k = next;
        }
        false
    }
}

/// Packs a unit normal (east, north, up) octahedrally into two snorm16.
fn octahedral(n: [f64; 3]) -> [i16; 2] {
    let s = n[0].abs() + n[1].abs() + n[2].abs();
    let (mut x, mut y) = (n[0] / s, n[1] / s);
    if n[2] < 0.0 {
        (x, y) = ((1.0 - y.abs()) * x.signum(), (1.0 - x.abs()) * y.signum());
    }
    [x, y].map(|v| (v.clamp(-1.0, 1.0) * 32767.0).round() as i16)
}

/// The heights of every face's vertex of a triangulation. `labels` gives each face's label in the
/// block; `rank` its overlay priority (0 wins); `mpu` the metres per unit at a row.
#[allow(clippy::too_many_arguments)]
pub fn heights(
    t: &Triangles,
    labels: &[Option<u32>],
    rank: &[u8],
    words: &[u32],
    terrain: &Terrain,
    cell: CellKey,
    range: Range,
    join_m: f64,
    mpu: &dyn Fn(f64) -> f64,
) -> Heights {
    let mesh = Mesh::new(t);
    let mut stats = HeightStats::default();
    let mut corners = Vec::new();
    let mut at = BTreeMap::new();
    let mut pairs_of: Vec<Vec<(u32, u32)>> = vec![Vec::new(); t.points.len()];
    for s in &t.segments {
        if s.left != OUTSIDE && s.right != OUTSIDE {
            pairs_of[s.a as usize].push((s.left, s.right));
            pairs_of[s.b as usize].push((s.left, s.right));
        }
    }
    for v in 0..t.points.len() as u32 {
        let mut faces: Vec<u32> = mesh.about[v as usize].iter().map(|&k| t.tris[k as usize].1).collect();
        faces.sort_unstable();
        faces.dedup();
        if faces.is_empty() {
            continue;
        }
        let p = mesh.pt(v);
        let Some(tp) = terrain_at(terrain, cell, p) else {
            stats.unknown += 1;
            continue;
        };
        let label = |f: u32| labels[f as usize];
        let mut own: Vec<(u32, Height)> = Vec::new();
        let mut pairs = std::mem::take(&mut pairs_of[v as usize]);
        pairs.sort_unstable();
        pairs.dedup();
        for &(a, b) in &pairs {
            if own.iter().any(|x| x.0 == a) && own.iter().any(|x| x.0 == b) {
                continue;
            }
            let h = height_at_sides(words, p, [label(a), label(b)], tp);
            stats.sides_calls += 1;
            for (f, h) in [(a, h[0]), (b, h[1])] {
                if !own.iter().any(|x| x.0 == f) {
                    own.push((f, h));
                }
            }
        }
        for &f in &faces {
            if !own.iter().any(|x| x.0 == f) {
                own.push((f, height_at(words, p, label(f), tp)));
                stats.inside_calls += 1;
            }
        }
        own.sort_unstable_by_key(|x| x.0);
        // Groups: faces joined across an edge whose heights meet within the join.
        let mut group: Vec<usize> = (0..own.len()).collect();
        let find = |g: &Vec<usize>, mut i: usize| {
            while g[i] != i {
                i = g[i];
            }
            i
        };
        for &(a, b) in &pairs {
            let (Some(i), Some(j)) = (own.iter().position(|x| x.0 == a), own.iter().position(|x| x.0 == b)) else {
                continue;
            };
            if (own[i].1.h - own[j].1.h).abs() <= join_m {
                let (ri, rj) = (find(&group, i), find(&group, j));
                group[ri.max(rj)] = ri.min(rj);
            }
        }
        let roots: Vec<usize> = (0..own.len()).map(|i| find(&group, i)).collect();
        let step = normal_step_m(range) / mpu(p[1]);
        let m = mpu(p[1]);
        for (i, &(f, h)) in own.iter().enumerate() {
            let lead = (0..own.len())
                .filter(|&j| roots[j] == roots[i])
                .min_by_key(|&j| (rank[own[j].0 as usize], own[j].0))
                .unwrap();
            // The normal: per axis, central differences where both offsets lie in the face, else the
            // one-sided one into it, halving the step until one does.
            let mut g = [0.0; 2];
            for (axis, gk) in g.iter_mut().enumerate() {
                let d = if axis == 0 { [1.0, 0.0] } else { [0.0, 1.0] };
                let mut s = step;
                let mut thin = false;
                let value = |q: [f64; 2], stats: &mut HeightStats| {
                    stats.normal_calls += 1;
                    terrain_at(terrain, cell, q).map(|tq| height_at(words, q, label(f), tq).h)
                };
                loop {
                    let (fw, bw) = (mesh.holds(v, f, d, s), mesh.holds(v, f, [-d[0], -d[1]], s));
                    let q = |sign: f64| [p[0] + sign * d[0] * s, p[1] + sign * d[1] * s];
                    let slope = match (fw, bw) {
                        (true, true) => {
                            value(q(1.0), &mut stats).zip(value(q(-1.0), &mut stats)).map(|(a, b)| (a - b) / (2.0 * s))
                        }
                        (true, false) => value(q(1.0), &mut stats).map(|a| (a - h.h) / s),
                        (false, true) => value(q(-1.0), &mut stats).map(|b| (h.h - b) / s),
                        (false, false) if s > 1.0 => {
                            thin = true;
                            s /= 2.0;
                            continue;
                        }
                        _ => Some(0.0),
                    };
                    *gk = slope.unwrap_or(0.0) / m;
                    break;
                }
                stats.thin_normals += u32::from(thin);
            }
            let n = [-g[0], g[1], 1.0];
            let l = libm::sqrt(n[0] * n[0] + n[1] * n[1] + 1.0);
            at.insert((v, f), corners.len() as u32);
            corners.push(Corner {
                point: v,
                face: f,
                own: h,
                h: own[lead].1.h,
                group: roots[i] as u32,
                normal: octahedral(n.map(|x| x / l)),
            });
        }
    }
    Heights { corners, at, stats }
}

/// Whether a step between two faces names a rule: a vertical face needs one on a side.
pub fn named(a: &Height, b: &Height) -> bool {
    a.step != Step::None || b.step != Step::None
}
