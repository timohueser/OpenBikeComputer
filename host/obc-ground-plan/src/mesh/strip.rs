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

    /// Twice the area (x east, y south, positive for `gridIndices`' turn) of a triangle with each of its
    /// terrain vertices at morph `m` (0 or 1).
    fn area(tri: [[i32; 2]; 3], terrain: [bool; 3], m: bool) -> i64 {
        let [a, b, c] = std::array::from_fn(|i| if m && terrain[i] { even(tri[i]) } else { tri[i] });
        -(i64::from(b[0] - a[0]) * i64::from(c[1] - a[1]) - i64::from(b[1] - a[1]) * i64::from(c[0] - a[0]))
    }

    #[test]
    fn each_strip_cell_is_drawn_once_by_the_strip_or_the_terrain_at_level_18_and_17() {
        use super::super::params::{CELLS, FACE_RING};
        use super::super::region::{Region, MARK_WORDS};
        // R grown from a few sharp cells, in many patterns, so that it has every corner shape.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % n) as i32
        };
        let (mut halves_seen, mut pinches) = ([0u32; 2], 0);
        for _ in 0..400 {
            let mut bits = vec![0u64; MARK_WORDS];
            for _ in 0..1 + next(8) {
                let (i, j) = (20 + next(24), 20 + next(24));
                let k = (j * CELLS + i) as usize;
                bits[k / 64] |= 1 << (k % 64);
            }
            let region = Region::new(&[([0, 0], bits.as_slice())]);
            let boundary = |g: [i32; 2]| region.on_boundary(g[0], g[1]);
            for j in 20 - FACE_RING - 1..44 + FACE_RING + 1 {
                for i in 20 - FACE_RING - 1..44 + FACE_RING + 1 {
                    if !region.strip(i, j) {
                        continue;
                    }
                    let tris = triangles(i, j);
                    // Level 18, morph 0: the drawn strip and the terrain's triangles without a sunk corner (a
                    // point of R off ∂R is sunk) tile the cell, and the latter are the left-out ears.
                    let drawn18 = |k: usize| !(tris[k].map(boundary) == [true; 3] && TERRAIN_EARS.contains(&k));
                    let strip18: i64 =
                        (0..6).filter(|&k| drawn18(k)).map(|k| area(tris[k], tris[k].map(boundary), false)).sum();
                    let ground18: Vec<_> = terrain(i, j).into_iter().filter(|t| t.map(boundary) == [true; 3]).collect();
                    for t in &ground18 {
                        assert!(
                            (0..6).any(|k| !drawn18(k) && tris[k] == *t),
                            "({i}, {j}): terrain {t:?} under the strip"
                        );
                    }
                    assert_eq!(strip18 + ground18.len() as i64, 8, "({i}, {j}) at level 18");
                    // Level 17: each half is the terrain's (all its corners on ∂R) or the strip's, whole.
                    let terrain17 = halves(i, j).map(|h| h.map(boundary) == [true; 3]);
                    for h in 0..2 {
                        let strip17: i64 = (3 * h..3 * h + 3)
                            .filter(|&k| drawn18(k) && !terrain17[h])
                            .map(|k| area(tris[k], tris[k].map(boundary), true))
                            .sum();
                        assert_eq!(strip17, if terrain17[h] { 0 } else { 4 }, "({i}, {j}) half {h} at level 17");
                        halves_seen[h] += u32::from(terrain17[h]);
                    }
                    // No T-junction: a side middle of a terrain half that is a fixed point (off ∂R) is a corner of
                    // no strip triangle that draws at level 17.
                    for h in (0..2).filter(|&h| terrain17[h]) {
                        let [a, b, c] = halves(i, j)[h];
                        let mid = |p: [i32; 2], q: [i32; 2]| [(p[0] + q[0]) / 2, (p[1] + q[1]) / 2];
                        for m in if h == 0 { [mid(a, b), mid(a, c)] } else { [mid(a, c), mid(b, c)] } {
                            if boundary(m) {
                                continue;
                            }
                            pinches += 1;
                            for (ni, nj) in [(i - 1, j), (i + 1, j), (i, j - 1), (i, j + 1), (i, j)] {
                                if !region.r(ni, nj) {
                                    continue;
                                }
                                let (nt, nh) =
                                    (triangles(ni, nj), halves(ni, nj).map(|h| h.map(boundary) == [true; 3]));
                                for k in (0..6).filter(|&k| !nh[k / 3] && area(nt[k], nt[k].map(boundary), true) != 0) {
                                    assert!(
                                        !nt[k].contains(&m),
                                        "({i}, {j}) half {h}: strip ({ni}, {nj}) uses {m:?} at level 17"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        // Both kinds of terrain half occur, and halves where two parts of R touch at a corner.
        assert!(halves_seen[0] > 0 && halves_seen[1] > 0 && pinches > 0, "{halves_seen:?} {pinches}");
    }
}
