//! The strip: each strip cell as a fixed fan of 6 triangles over its border, so the mesh meets the
//! terrain grid on ∂R at every morph.
//!
//! A strip cell holds the 8 grid points on its border: its corners and the middle of each side. Its
//! middle is never on ∂R and always sunk, so the strip leaves it out: four ears at the corners and two
//! triangles in the diamond of the side middles. The ears at the north-west and south-east corners are
//! the terrain's own triangles there (the diagonal of `gridIndices`); they are the only terrain
//! triangles of the cell without its middle.
//!
//! A point on ∂R is a terrain vertex: it draws with the terrain's own vertex code, so it slides,
//! morphs and takes the page's height as the terrain's vertex there does. Every other point is a fixed
//! ground corner. A terrain vertex slides along its side to the cell's corner; no strip triangle turns
//! over for any morph of its terrain vertices (the test below, for every neighbour pattern). An ear
//! with all three corners on ∂R is the terrain's own triangle: the terrain draws it and the mesh
//! leaves it out.
//!
//! The claim holds where the terrain draws nodes of `MESH_LEVEL` without the snap of its vertices to
//! road edges (`vertex_place` with the snap off). On a coarser node, the terrain's triangle over the
//! three corners of a convex corner of R has no sunk corner and lies under the strip, so the mesh
//! draws only over nodes of `MESH_LEVEL`. The snap moves a terrain vertex across the strip's triangles.

/// The 6 triangles of the strip cell at region cell (i, j), as grid points, turning as
/// `gridIndices` (negative signed area in x east, y south).
pub fn triangles(i: i32, j: i32) -> [[[i32; 2]; 3]; 6] {
    let at = |u: i32, v: i32| [2 * i + u, 2 * j + v];
    let [c00, c20, c02, c22] = [at(0, 0), at(2, 0), at(0, 2), at(2, 2)];
    let [m10, m01, m21, m12] = [at(1, 0), at(0, 1), at(2, 1), at(1, 2)];
    [[c00, m01, m10], [m10, m21, c20], [m01, c02, m12], [m21, m12, c22], [m10, m01, m21], [m01, m12, m21]]
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
            for tri in triangles(1, 1) {
                let corners = orientation_corners(tri, tri.map(boundary));
                assert!(
                    corners[0] < 0 && corners.iter().all(|&a| a <= 0),
                    "pattern {pattern:08b}: {tri:?} {corners:?}"
                );
            }
        }
    }
}
