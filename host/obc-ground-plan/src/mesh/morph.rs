//! The Rust twin of the terrain's `vertex_place` and `place_height` (`terrain/drawn-mesh.wgsl`) at
//! level 18 with the snap off, with heights from `Terrain`: for the strip's morph test, the floors and
//! GP9's tests.

use super::params::L18;
use crate::frame::{CellKey, GlobalPoint};
use crate::terrain::Terrain;

/// The point that a level-18 grid point slides to: each odd coordinate goes to the even one before
/// it. The cell's origin is a multiple of 2^18, so a local coordinate has the global one's parity.
pub fn even(g: [i32; 2]) -> [i32; 2] {
    g.map(|v| v - (v & L18))
}

/// Where a level-18 grid point (frame units from the cell's origin) lies at morph m, and its height:
/// the fine height at the point mixed toward the coarse height at its even point, as `place_height`
/// (with one unknown height, the known one; the coarse one first).
pub fn place(cell: CellKey, terrain: &Terrain, g: [i32; 2], m: f64) -> ([f64; 2], Option<f64>) {
    let h = |p: [i32; 2]| {
        let o = cell.origin();
        terrain.height(GlobalPoint { x: o.x + i64::from(p[0]), y: o.y + i64::from(p[1]) })
    };
    let f = |v: i32| f64::from(v);
    if m <= 0.0 {
        return ([f(g[0]), f(g[1])], h(g));
    }
    let e = even(g);
    let at = [f(g[0]) + (f(e[0]) - f(g[0])) * m, f(g[1]) + (f(e[1]) - f(g[1])) * m];
    let coarse = h(e);
    if m >= 1.0 {
        return (at, coarse);
    }
    let height = match (h(g), coarse) {
        (Some(fine), Some(coarse)) => Some(fine + (coarse - fine) * m),
        (fine, None) => fine,
        (None, coarse) => coarse,
    };
    (at, height)
}

/// A strip vertex for the morph test: its grid point and whether it slides (a terrain vertex) or
/// stays (a fixed vertex).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MorphVertex {
    pub at: [i32; 2],
    pub slides: bool,
}

impl MorphVertex {
    fn at_morph(self, m: bool) -> [i64; 2] {
        let p = if m && self.slides { even(self.at) } else { self.at };
        p.map(i64::from)
    }
}

/// Twice the signed area (x east, y south) of a triangle at the 8 corners of its morph cube, corner
/// k with vertex i at morph `k >> i & 1`. The area is multilinear in the three morphs, so these bound
/// it at every morph.
pub fn orientation_corners(tri: [MorphVertex; 3]) -> [i128; 8] {
    std::array::from_fn(|k| {
        let [a, b, c] = std::array::from_fn(|i| tri[i].at_morph(k >> i & 1 == 1));
        i128::from(b[0] - a[0]) * i128::from(c[1] - a[1]) - i128::from(b[1] - a[1]) * i128::from(c[0] - a[0])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::from_lon_lat;
    use crate::terrain::{StoredPage, Terrain, PAGE};

    #[test]
    fn odd_points_slide_to_the_even_point_before_them_and_heights_mix_by_the_morph() {
        assert_eq!(even([0, 1024]), [0, 0]);
        assert_eq!(even([3 * 1024, 4 * 1024]), [2 * 1024, 4 * 1024]);
        assert_eq!(even([-1024, -2048]), [-2048, -2048]);
        let cell = CellKey::of(from_lon_lat(8.4, 46.8));
        let wanted = Terrain::wanted(cell);
        // A plane rising 1 m per level-18 texel row: the twin reads the same plane at both levels.
        let pages = wanted
            .iter()
            .map(|k| StoredPage {
                level: k.level,
                x: k.x,
                y: k.y,
                offset: 100.0,
                scale: 0.0,
                codes: vec![0; PAGE * PAGE],
            })
            .collect();
        let terrain = Terrain::new(cell, pages);
        let g = [5 * 1024, 2 * 1024];
        assert_eq!(place(cell, &terrain, g, 0.0), ([5120.0, 2048.0], Some(100.0)));
        assert_eq!(place(cell, &terrain, g, 1.0).0, [4096.0, 2048.0]);
        assert_eq!(place(cell, &terrain, g, 0.5), ([4608.0, 2048.0], Some(100.0)));
        assert_eq!(place(cell, &terrain, [4096, 2048], 0.5).0, [4096.0, 2048.0], "an even point stays");
    }

    #[test]
    fn a_triangle_that_turns_over_in_the_morph_shows_it_at_a_corner() {
        let v = |x: i32, y: i32, slides: bool| MorphVertex { at: [x * 1024, y * 1024], slides };
        // The terrain's own triangle (a, a+n, a+1) of the cell at (1, 0): negative at every corner.
        let own = orientation_corners([v(1, 0, true), v(1, 1, true), v(2, 0, false)]);
        assert!(own.iter().all(|&a| a <= 0) && own[0] < 0, "{own:?}");
        // A sliver whose odd vertex slides across the opposite edge.
        let over = orientation_corners([v(1, 1, true), v(1, 0, false), v(0, 1, false)]);
        assert!(over.iter().any(|&a| a > 0) && over.iter().any(|&a| a < 0), "{over:?}");
    }
}
