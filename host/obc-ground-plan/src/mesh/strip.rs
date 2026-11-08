//! The strip: each strip cell as a fixed fan of 6 triangles over its border, so the mesh meets the
//! terrain grid on ∂R at every morph, over level-18 and level-17 nodes.
//!
//! A strip cell holds the 8 grid points on its border: its corners and the middle of each side. Its
//! middle is never on ∂R and always sunk, so the strip leaves it out. The fan follows the terrain's
//! diagonal from the north-east to the south-west corner (`gridIndices`): three triangles tile the
//! north-west half, three the south-east half, and each half's side middles lie on the half's border.
//! A middle slides along its side to the cell's corner, so each half stays tiled at every morph, and at
//! morph 1 (a level-17 node) the halves are the terrain's two level-17 triangles of the cell.
//!
//! The ears at the north-west and south-east corners (`TERRAIN_EARS`) are the terrain's own level-18
//! triangles there: with all three corners on ∂R, they have no sunk corner, the terrain draws them and
//! the strip leaves them out. In the same way a half whose three corners lie on ∂R is the terrain's own
//! level-17 triangle: over a level-17 node the strip leaves out the triangles of that half. Such a half
//! lies at a north-west or south-east convex corner of R, or where two parts of R touch at a corner.
//!
//! A point on ∂R is a terrain vertex: it draws with the terrain's own vertex code, so it slides,
//! morphs and takes the page's height as the terrain's vertex there does. Every other point is a fixed
//! ground corner. No strip triangle turns over for any morph of its terrain vertices (the test below,
//! for every neighbour pattern). Over a level-17 node the mesh range keeps the level-17 vertices out
//! of their morph, so the strip needs no level-17 morph.
//!
//! At morph 1 a strip triangle can lose its plan area and still be needed. At a straight west border
//! of R with the north cell in R, [m10 m01 c20] becomes (m10, c00, c20): a vertical triangle between
//! the fixed m10, which the north cell shares, and the half's edge c00–c20. It closes the surface
//! there, so no step may drop a strip triangle for a zero plan area.
//!
//! The claim holds where the terrain draws its vertices there without the snap to road edges: the snap
//! moves a terrain vertex across the strip's triangles.

/// The 6 triangles of the strip cell at region cell (i, j), as grid points, turning as
/// `gridIndices` (negative signed area in x east, y south): the north-west half, then the south-east.
pub fn triangles(i: i32, j: i32) -> [[[i32; 2]; 3]; 6] {
    let at = |u: i32, v: i32| [2 * i + u, 2 * j + v];
    let [c00, c20, c02, c22] = [at(0, 0), at(2, 0), at(0, 2), at(2, 2)];
    let [m10, m01, m21, m12] = [at(1, 0), at(0, 1), at(2, 1), at(1, 2)];
    [[c00, m01, m10], [m10, m01, c20], [m01, c02, c20], [c20, c02, m21], [m21, c02, m12], [m21, m12, c22]]
}

/// The ears of `triangles` that are the terrain's own level-18 triangles: north-west and south-east.
pub const TERRAIN_EARS: [usize; 2] = [0, 5];

/// The corners of the cell's two halves, the terrain's level-17 triangles: triangles 0 to 2 of
/// `triangles` tile the first, 3 to 5 the second.
pub fn halves(i: i32, j: i32) -> [[[i32; 2]; 3]; 2] {
    let at = |u: i32, v: i32| [2 * i + u, 2 * j + v];
    [[at(0, 0), at(0, 2), at(2, 0)], [at(2, 0), at(0, 2), at(2, 2)]]
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::super::params::GRID;
    use super::super::region::Region;
    use super::super::CellMesh;
    use super::*;

    /// The point that a grid point slides to: each odd coordinate to the even one before it.
    fn even(g: [i32; 2]) -> [i32; 2] {
        g.map(|v| v - (v & 1))
    }

    /// Twice the signed area (x east, y south) of a triangle at the 8 corners of its morph cube: corner
    /// k with vertex i at morph `k >> i & 1`, where only terrain vertices move. The area is
    /// multilinear in the three morphs, so these bound it at every morph.
    fn orientation_corners(tri: [[i32; 2]; 3], slides: [bool; 3]) -> [i64; 8] {
        std::array::from_fn(|k| {
            let [a, b, c] = std::array::from_fn(|i| if k >> i & 1 == 1 && slides[i] { even(tri[i]) } else { tri[i] });
            i64::from(b[0] - a[0]) * i64::from(c[1] - a[1]) - i64::from(b[1] - a[1]) * i64::from(c[0] - a[0])
        })
    }

    /// The terrain's 8 triangles over region cell (i, j), as `gridIndices`: (a, a + n, a + 1) and
    /// (a + 1, a + n, a + n + 1) per grid cell a.
    fn terrain(i: i32, j: i32) -> Vec<[[i32; 2]; 3]> {
        [(0, 0), (1, 0), (0, 1), (1, 1)]
            .into_iter()
            .flat_map(|(u, v)| {
                let a = [2 * i + u, 2 * j + v];
                let (n, r, nr) = ([a[0], a[1] + 1], [a[0] + 1, a[1]], [a[0] + 1, a[1] + 1]);
                [[a, n, r], [r, n, nr]]
            })
            .collect()
    }

    #[test]
    fn no_strip_triangle_turns_over_for_any_neighbour_pattern_or_morph() {
        // The strip cell (1, 1) of a 3 × 3 window; bit k of the pattern puts neighbour k in R.
        let neighbours = [[0, 0], [1, 0], [2, 0], [2, 1], [2, 2], [1, 2], [0, 2], [0, 1]];
        for pattern in 0..255u32 {
            let r = |c: [i32; 2]| {
                c == [1, 1] || neighbours.iter().enumerate().any(|(k, &n)| n == c && pattern >> k & 1 == 1)
            };
            // A grid point lies on ∂R when it touches a cell of R and one outside.
            let boundary = |g: [i32; 2]| {
                let span = |v: i32| if v % 2 == 0 { vec![v / 2 - 1, v / 2] } else { vec![v / 2] };
                let cells: Vec<[i32; 2]> =
                    span(g[1]).into_iter().flat_map(|y| span(g[0]).into_iter().map(move |x| [x, y])).collect();
                cells.iter().any(|&c| r(c)) && cells.iter().any(|&c| !r(c))
            };
            for (k, tri) in triangles(1, 1).into_iter().enumerate() {
                // A left-out ear is a triangle that the terrain draws.
                if tri.map(boundary) == [true; 3] && TERRAIN_EARS.contains(&k) {
                    assert!(terrain(1, 1).contains(&tri), "pattern {pattern:08b}: ear {tri:?}");
                }
                let corners = orientation_corners(tri, tri.map(boundary));
                assert!(
                    corners[0] < 0 && corners.iter().all(|&a| a <= 0),
                    "pattern {pattern:08b}: {tri:?} {corners:?}"
                );
            }
        }
    }

    /// The region cells that touch grid point g: one, two or four.
    fn touching(g: [i32; 2]) -> Vec<[i32; 2]> {
        let span = |v: i32| if v % 2 == 0 { vec![v / 2 - 1, v / 2] } else { vec![v / 2] };
        span(g[1]).into_iter().flat_map(|y| span(g[0]).into_iter().map(move |x| [x, y])).collect()
    }

    /// A drawn corner: its grid point and whether it is a terrain vertex; a drawn triangle.
    type Corner = ([i32; 2], bool);
    type Drawn = [Corner; 3];

    /// What draws over region cells lo..hi with terrain nodes of `level` (18 or 17): the cell's mesh, at
    /// 17 without its level-18 runs and with its terrain vertices at their even point, and the terrain's
    /// triangles without a sunk corner (a point whose region cells all lie in R).
    fn drawn(mesh: &CellMesh, region: &Region, level: u32, lo: i32, hi: i32) -> Vec<Drawn> {
        let level18: u32 =
            mesh.triangle_runs[mesh.triangle_runs.len() - mesh.level18_runs as usize..].iter().map(|r| r[0]).sum();
        let shown = mesh.triangles.len() - if level < 18 { level18 as usize } else { 0 };
        let mut out: Vec<Drawn> = mesh.triangles[..shown]
            .iter()
            .map(|t| {
                t.map(|c| {
                    let (p, _, terrain) = mesh.corner(c as usize);
                    let g = p.map(|v| v / GRID);
                    (if level < 18 && terrain { even(g) } else { g }, terrain)
                })
            })
            .collect();
        let sunk = |g: [i32; 2]| touching(g).iter().all(|c| region.r(c[0], c[1]));
        for j in lo..hi {
            for i in lo..hi {
                let ground = if level < 18 { halves(i, j).to_vec() } else { terrain(i, j) };
                out.extend(ground.into_iter().filter(|t| !t.iter().any(|&g| sunk(g))).map(|t| t.map(|g| (g, true))));
            }
        }
        out
    }

    /// Twice the area of a triangle in grid points, positive for `gridIndices`' turn (x east, y south).
    fn turn(t: [[f64; 2]; 3]) -> f64 {
        -((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[1][1] - t[0][1]) * (t[2][0] - t[0][0]))
    }

    /// The faults of what draws inside region cells lo..hi less two: a sample point that is not covered
    /// exactly once, and an edge without its reverse (vertices by point and kind, so a crack shows). A
    /// zero-area triangle covers no point but closes edges.
    fn faults(tris: &[Drawn], lo: i32, hi: i32) -> Vec<String> {
        let (a, b) = (2 * lo + 4, 2 * hi - 4);
        let mut cells: BTreeMap<[i32; 2], Vec<usize>> = BTreeMap::new();
        for (k, t) in tris.iter().enumerate() {
            let (x0, x1) = (t.iter().map(|c| c.0[0]).min().unwrap(), t.iter().map(|c| c.0[0]).max().unwrap());
            let (y0, y1) = (t.iter().map(|c| c.0[1]).min().unwrap(), t.iter().map(|c| c.0[1]).max().unwrap());
            for y in y0..y1.max(y0 + 1) {
                for x in x0..x1.max(x0 + 1) {
                    cells.entry([x, y]).or_default().push(k);
                }
            }
        }
        let mut out = Vec::new();
        for y in a..b {
            for x in a..b {
                for o in [[0.1234, 0.3141], [0.6123, 0.1732], [0.2718, 0.866], [0.4142, 0.539]] {
                    let p = [f64::from(x) + o[0], f64::from(y) + o[1]];
                    let n = (cells.get(&[x, y]).into_iter().flatten())
                        .filter(|&&k| {
                            let t = tris[k].map(|c| c.0.map(f64::from));
                            (0..3).all(|e| turn([t[e], t[(e + 1) % 3], p]) > 0.0)
                        })
                        .count();
                    if n != 1 {
                        out.push(format!("point {p:?} drawn {n} times"));
                    }
                }
            }
        }
        let mut edges: BTreeMap<(Corner, Corner), i32> = BTreeMap::new();
        for t in tris {
            for e in 0..3 {
                let (p, q) = (t[e], t[(e + 1) % 3]);
                if p.0 != q.0 {
                    *edges.entry((p, q)).or_default() += 1;
                    *edges.entry((q, p)).or_default() -= 1;
                }
            }
        }
        let inside = |g: [i32; 2]| g.iter().all(|v| (a..=b).contains(v));
        out.extend(
            (edges.iter())
                .filter(|(e, &n)| n != 0 && inside(e.0 .0) && inside(e.1 .0))
                .map(|(e, n)| format!("edge {e:?} open {n}")),
        );
        out
    }

    #[test]
    fn mesh_cell_and_the_terrain_draw_each_point_once_and_close_at_level_18_and_17() {
        use super::super::params::{CELLS, FACE_RING};
        use super::super::region::MARK_WORDS;
        use super::super::{mesh_cell, Reads};
        use crate::faces::{records, Rules};
        use crate::frame::CellKey;
        use crate::terrain::Terrain;
        // An empty cell: one terrain face, so every corner of the mesh is ground.
        let key = CellKey { x: 34_307, y: 23_107 };
        let input = crate::CellInput { key, features: Vec::new(), terrain: Terrain::new(key, Vec::new()) };
        let rules = Rules::embedded();
        let (plan, near) =
            records::near(&crate::graph::build(key, &input, &rules.sections), &input, rules, key, 0, true);
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as i32
        };
        // A sharp cell grows to a square of this side: two at this offset across and down touch at a corner.
        let side = 2 * FACE_RING + 1;
        let (lo, hi) = (20 - FACE_RING - 3, 44 + side + FACE_RING + 3);
        let (mut contacts, mut halves18) = (0, [0u32; 2]);
        for shape in 0..50 {
            let mut bits = vec![0u64; MARK_WORDS];
            let mut mark = |i: i32, j: i32| {
                let k = (j * CELLS + i) as usize;
                bits[k / 64] |= 1 << (k % 64);
            };
            // Every other shape forces two parts of R to touch at a corner; random cells add every other shape.
            if shape % 2 == 0 {
                let (i, j, down) = (20 + next(20), 24 + next(16), if next(2) == 0 { 1 } else { -1 });
                mark(i, j);
                mark(i + side, j + down * side);
            }
            for _ in 0..1 + next(6) {
                mark(20 + next(24), 20 + next(24));
            }
            let region = Region::new(&[([0, 0], bits.as_slice())]);
            let reads = Reads { region, lines: Default::default() };
            let mesh = mesh_cell(&plan, &near.words, &reads, &mut || 0.0).expect("a mesh");
            let region = &reads.region;
            let boundary = |g: [i32; 2]| region.on_boundary(g[0], g[1]);
            for level in [18, 17] {
                let found = faults(&drawn(&mesh, region, level, lo, hi), lo, hi);
                assert!(found.is_empty(), "shape {shape} level {level}: {:?}", &found[..found.len().min(8)]);
            }
            // The level-18 runs hold exactly the strip triangles of halves with all three corners on ∂R.
            let n = mesh.triangles.len();
            let level18: u32 =
                mesh.triangle_runs[mesh.triangle_runs.len() - mesh.level18_runs as usize..].iter().map(|r| r[0]).sum();
            for (k, t) in mesh.triangles.iter().enumerate().skip(n - mesh.strip_triangles as usize) {
                let c = t.iter().fold([0, 0], |s, &v| {
                    let p = mesh.corner(v as usize).0;
                    [s[0] + p[0] / GRID, s[1] + p[1] / GRID]
                });
                let (i, j) = (c[0].div_euclid(6), c[1].div_euclid(6));
                let h = usize::from(c[0] - 6 * i + c[1] - 6 * j > 6);
                let terrain_half = halves(i, j)[h].iter().all(|&g| boundary(g));
                assert_eq!(
                    k >= n - level18 as usize,
                    terrain_half,
                    "shape {shape}: strip triangle {k} of ({i}, {j}) half {h}"
                );
                halves18[h] += u32::from(terrain_half);
            }
            for j in lo..hi {
                for i in lo..hi {
                    let r = [[0, 0], [1, 0], [0, 1], [1, 1]].map(|d| region.r(i - 1 + d[0], j - 1 + d[1]));
                    contacts += u32::from(r == [true, false, false, true] || r == [false, true, true, false]);
                }
            }
        }
        // Both halves go to the level-18 runs, and parts of R touch at a corner.
        assert!(halves18[0] > 0 && halves18[1] > 0 && contacts > 0, "{halves18:?} {contacts}");
    }
}
