//! Which candidates cover a point: a grid of candidate boxes, and per candidate its segments in
//! horizontal strips, so a winding number reads only the segments of the point's strip.

use super::geom::P;

/// The strip height and the grid cell: 1,024 frame units, about 1.6 m.
const SHIFT: u32 = 10;

struct Region {
    bbox: [i32; 4],
    /// Per strip from `bbox[1] >> SHIFT`, the segments whose y range meets it.
    strips: Vec<Vec<[P; 2]>>,
}

pub struct Cover {
    regions: Vec<Region>,
    origin: [i32; 2],
    cols: usize,
    rows: usize,
    grid: Vec<Vec<u32>>,
}

fn strip(y: i32) -> i32 {
    y >> SHIFT
}

impl Cover {
    /// The index of candidates' rings over the window [x0, y0, x1, y1].
    pub fn new(window: [i32; 4], candidates: &[&[Vec<P>]]) -> Cover {
        let origin = [window[0] >> SHIFT, window[1] >> SHIFT];
        let cols = ((window[2] >> SHIFT) - origin[0] + 1) as usize;
        let rows = ((window[3] >> SHIFT) - origin[1] + 1) as usize;
        let mut cover = Cover { regions: Vec::new(), origin, cols, rows, grid: vec![Vec::new(); cols * rows] };
        for (id, rings) in candidates.iter().enumerate() {
            let points = rings.iter().flatten();
            let bbox = points.fold([i32::MAX, i32::MAX, i32::MIN, i32::MIN], |b, p| {
                [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]
            });
            let mut strips = Vec::new();
            if bbox[0] <= bbox[2] {
                strips = vec![Vec::new(); (strip(bbox[3]) - strip(bbox[1]) + 1) as usize];
                for ring in rings.iter() {
                    for i in 0..ring.len() {
                        let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
                        for s in strip(a[1].min(b[1]))..=strip(a[1].max(b[1])) {
                            strips[(s - strip(bbox[1])) as usize].push([a, b]);
                        }
                    }
                }
                let cx = |x: i32| ((x >> SHIFT) - origin[0]).clamp(0, cols as i32 - 1) as usize;
                let cy = |y: i32| ((y >> SHIFT) - origin[1]).clamp(0, rows as i32 - 1) as usize;
                for y in cy(bbox[1])..=cy(bbox[3]) {
                    for x in cx(bbox[0])..=cx(bbox[2]) {
                        cover.grid[y * cols + x].push(id as u32);
                    }
                }
            }
            cover.regions.push(Region { bbox, strips });
        }
        cover
    }

    /// The winding number of candidate `id` about a point in doubled coordinates.
    fn winding(&self, id: usize, q: [i64; 2]) -> i32 {
        let r = &self.regions[id];
        let (x, y) = (q[0].div_euclid(2) as i32, q[1].div_euclid(2) as i32);
        if x < r.bbox[0] || x > r.bbox[2] || y < r.bbox[1] || y > r.bbox[3] {
            return 0;
        }
        let mut w = 0;
        for &[a, b] in &r.strips[(strip(y) - strip(r.bbox[1])) as usize] {
            let (a2, b2) = ([2 * i64::from(a[0]), 2 * i64::from(a[1])], [2 * i64::from(b[0]), 2 * i64::from(b[1])]);
            let side = || {
                i128::from(b2[0] - a2[0]) * i128::from(q[1] - a2[1]) - i128::from(q[0] - a2[0]) * i128::from(b2[1] - a2[1])
            };
            if a2[1] <= q[1] {
                if b2[1] > q[1] && side() > 0 {
                    w += 1;
                }
            } else if b2[1] <= q[1] && side() < 0 {
                w -= 1;
            }
        }
        w
    }

    /// The candidates that cover a point in doubled coordinates, in index order.
    pub fn at(&self, q: [i64; 2]) -> Vec<u32> {
        let cx = ((q[0].div_euclid(2) as i32 >> SHIFT) - self.origin[0]).clamp(0, self.cols as i32 - 1) as usize;
        let cy = ((q[1].div_euclid(2) as i32 >> SHIFT) - self.origin[1]).clamp(0, self.rows as i32 - 1) as usize;
        let mut out: Vec<u32> =
            self.grid[cy * self.cols + cx].iter().copied().filter(|&id| self.winding(id as usize, q) != 0).collect();
        out.sort_unstable();
        out
    }
}
