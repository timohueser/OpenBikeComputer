//! The strip: the terrain's own triangles over each strip cell, so the mesh meets the terrain grid on
//! ∂R at every morph.
//!
//! A strip cell holds 3 × 3 level-18 grid points and the terrain's 8 triangles over them (the diagonal
//! of `gridIndices`). A point on ∂R is a terrain vertex: it draws with the terrain's own vertex code,
//! so it slides, morphs and takes the page's height as the terrain's vertex there does. Every other
//! point is a fixed ground corner. A terrain vertex slides along ∂R to its even point; no strip
//! triangle turns over for any morph of its terrain vertices (the test below, for every neighbour
//! pattern). The terrain's triangles inside R whose corners all lie on ∂R are these same triangles;
//! every other terrain triangle inside R has a sunk corner, so it lies far under the mesh.

/// The 8 triangles of the strip cell at level-17 cell (i, j), as level-18 grid points, turning as
/// `gridIndices`: (a, a + n, a + 1) and (a + 1, a + n, a + n + 1) per level-18 cell a.
pub fn triangles(i: i32, j: i32) -> [[[i32; 2]; 3]; 8] {
    let mut out = [[[0; 2]; 3]; 8];
    for (k, (u, v)) in [(0, 0), (1, 0), (0, 1), (1, 1)].into_iter().enumerate() {
        let a = [2 * i + u, 2 * j + v];
        let (n, r, nr) = ([a[0], a[1] + 1], [a[0] + 1, a[1]], [a[0] + 1, a[1] + 1]);
        out[2 * k] = [a, n, r];
        out[2 * k + 1] = [r, n, nr];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The point that a level-18 grid point slides to: each odd coordinate to the even one before it.
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
