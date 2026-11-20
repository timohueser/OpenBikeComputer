//! Distances in metres between points, lines and areas in microdegrees, and a grid that finds the
//! objects near a box.

use planner_router::geometry::METRES_PER_UDEG;
use planner_router::shape::P;

type Xy = [f64; 2];

/// Metres east and north of the origin, with the east scale of one latitude. Over a few
/// kilometres, the error of one scale is far below a metre.
#[derive(Clone, Copy)]
pub struct Plane {
    east: f64,
}

impl Plane {
    pub fn at(lat: i32) -> Self {
        Plane { east: (lat as f64 * 1e-6).to_radians().cos() * METRES_PER_UDEG }
    }

    pub fn xy(&self, p: P) -> Xy {
        [p[0] as f64 * self.east, p[1] as f64 * METRES_PER_UDEG]
    }

    pub fn distance(&self, a: P, b: P) -> f64 {
        let (a, b) = (self.xy(a), self.xy(b));
        (a[0] - b[0]).hypot(a[1] - b[1])
    }
}

fn to_segment(p: Xy, a: Xy, b: Xy) -> f64 {
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = ((p[0] - a[0]) * d[0] + (p[1] - a[1]) * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(f64::MIN_POSITIVE);
    let t = t.clamp(0.0, 1.0);
    (a[0] + d[0] * t - p[0]).hypot(a[1] + d[1] * t - p[1])
}

/// The distance from `p` to the segment `a`–`b`.
pub fn to_segment_m(p: P, [a, b]: [P; 2]) -> f64 {
    let plane = Plane::at(p[1]);
    to_segment(plane.xy(p), plane.xy(a), plane.xy(b))
}

/// The distance between two segments: 0 when they cross, else the nearest end to the other one.
fn between(a: [Xy; 2], b: [Xy; 2]) -> f64 {
    let side = |p: Xy, q: Xy, r: Xy| (q[0] - p[0]) * (r[1] - p[1]) - (q[1] - p[1]) * (r[0] - p[0]);
    let crosses =
        side(a[0], a[1], b[0]) * side(a[0], a[1], b[1]) < 0.0 && side(b[0], b[1], a[0]) * side(b[0], b[1], a[1]) < 0.0;
    if crosses {
        return 0.0;
    }
    let ends = [to_segment(a[0], b[0], b[1]), to_segment(a[1], b[0], b[1])];
    ends.into_iter().chain([to_segment(b[0], a[0], a[1]), to_segment(b[1], a[0], a[1])]).fold(f64::INFINITY, f64::min)
}

/// Whether `p` is inside the area of `rings` by the even-odd rule, so an inner ring is a hole.
pub fn inside(p: P, rings: &[Vec<P>]) -> bool {
    let (x, y) = (p[0] as f64, p[1] as f64);
    let mut inside = false;
    for ring in rings {
        for w in ring.windows(2) {
            let ([ax, ay], [bx, by]) = (w[0].map(f64::from), w[1].map(f64::from));
            if (ay > y) != (by > y) && x < ax + (y - ay) * (bx - ax) / (by - ay) {
                inside = !inside;
            }
        }
    }
    inside
}

/// The area of a closed ring in square metres.
pub fn ring_m2(ring: &[P]) -> f64 {
    let plane = Plane::at(ring[0][1]);
    let origin = plane.xy(ring[0]);
    let xy = |p: P| {
        let q = plane.xy(p);
        [q[0] - origin[0], q[1] - origin[1]]
    };
    (ring.windows(2).map(|w| xy(w[0])[0] * xy(w[1])[1] - xy(w[1])[0] * xy(w[0])[1]).sum::<f64>() / 2.0).abs()
}

/// A point of the area of the largest outer ring: its centroid when that lies inside the ring, else
/// the vertex of the ring nearest to the centroid.
pub fn inner_point(outer: &[Vec<P>]) -> P {
    let ring = outer.iter().max_by(|a, b| ring_m2(a).total_cmp(&ring_m2(b))).expect("an area has an outer ring");
    let origin = ring[0].map(f64::from);
    let (mut area, mut cx, mut cy) = (0.0, 0.0, 0.0);
    for w in ring.windows(2) {
        let (a, b) = (w[0].map(f64::from), w[1].map(f64::from));
        let (ax, ay, bx, by) = (a[0] - origin[0], a[1] - origin[1], b[0] - origin[0], b[1] - origin[1]);
        let cross = ax * by - bx * ay;
        (area, cx, cy) = (area + cross, cx + (ax + bx) * cross, cy + (ay + by) * cross);
    }
    let centroid = if area == 0.0 {
        ring[0]
    } else {
        [(origin[0] + cx / (3.0 * area)).round() as i32, (origin[1] + cy / (3.0 * area)).round() as i32]
    };
    if inside(centroid, std::slice::from_ref(ring)) {
        return centroid;
    }
    let plane = Plane::at(centroid[1]);
    *ring.iter().min_by(|a, b| plane.distance(**a, centroid).total_cmp(&plane.distance(**b, centroid))).unwrap()
}

/// The box of `points`: the smallest and the largest longitude and latitude.
pub fn bounds(points: &[P]) -> [P; 2] {
    let (mut min, mut max) = ([i32::MAX; 2], [i32::MIN; 2]);
    for p in points {
        min = [min[0].min(p[0]), min[1].min(p[1])];
        max = [max[0].max(p[0]), max[1].max(p[1])];
    }
    [min, max]
}

/// A box widened by `reach_m` on each side, east and west at its latitude farthest from the equator.
pub fn widen([min, max]: [P; 2], reach_m: f64) -> [P; 2] {
    let lat = (min[1].unsigned_abs().max(max[1].unsigned_abs()) as f64 * 1e-6).to_radians();
    let north = (reach_m / METRES_PER_UDEG).ceil();
    let east = (north / lat.cos().max(0.01)).ceil();
    let by = |value: i32, by: f64| (value as f64 + by).clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    [[by(min[0], -east), by(min[1], -north)], [by(max[0], east), by(max[1], north)]]
}

/// The box of `points` widened by `reach_m`.
pub fn around(points: &[P], reach_m: f64) -> [P; 2] {
    widen(bounds(points), reach_m)
}

pub fn meets(a: [P; 2], b: [P; 2]) -> bool {
    a[0][0] <= b[1][0] && b[0][0] <= a[1][0] && a[0][1] <= b[1][1] && b[0][1] <= a[1][1]
}

/// A line in the plane of its first point, to measure the distances of points and areas to it.
pub struct Line {
    plane: Plane,
    first: P,
    bounds: [P; 2],
    xy: Vec<Xy>,
}

impl Line {
    /// A line of one or more points.
    pub fn new(points: &[P]) -> Self {
        let plane = Plane::at(points[0][1]);
        let xy = points.iter().map(|&p| plane.xy(p)).collect();
        Line { plane, first: points[0], bounds: bounds(points), xy }
    }

    fn segments(&self) -> impl Iterator<Item = [Xy; 2]> + '_ {
        let last = self.xy.len() - 1;
        (0..self.xy.len().max(2) - 1).map(move |k| [self.xy[k], self.xy[(k + 1).min(last)]])
    }

    pub fn to_point(&self, p: P) -> f64 {
        let p = self.plane.xy(p);
        self.segments().map(|[a, b]| to_segment(p, a, b)).fold(f64::INFINITY, f64::min)
    }

    /// The distance to the edge of the area of `rings`, 0 when the line enters it. An edge farther
    /// than `reach_m` from the box of the line does not count, so a far area is at infinity.
    pub fn to_area(&self, rings: &[Vec<P>], reach_m: f64) -> f64 {
        if inside(self.first, rings) {
            return 0.0;
        }
        let near = widen(self.bounds, reach_m);
        let mut best = f64::INFINITY;
        for ring in rings {
            for w in ring.windows(2) {
                if !meets(near, bounds(w)) {
                    continue;
                }
                let edge = [self.plane.xy(w[0]), self.plane.xy(w[1])];
                best = self.segments().map(|segment| between(segment, edge)).fold(best, f64::min);
            }
        }
        best
    }
}

/// The items of a grid of square cells of microdegrees, by the cells that their boxes meet.
pub struct Grid {
    cell: f64,
    /// (cell, item), sorted.
    entries: Vec<(u64, u32)>,
}

impl Grid {
    /// Cells of `cell_m` metres north to south. The size changes the speed of a search, never its
    /// result.
    pub fn new(cell_m: f64, boxes: impl IntoIterator<Item = ([P; 2], u32)>) -> Self {
        let cell = cell_m / METRES_PER_UDEG;
        let mut grid = Grid { cell, entries: Vec::new() };
        for (bounds, item) in boxes {
            let entries: Vec<(u64, u32)> = grid.cells(bounds).map(|key| (key, item)).collect();
            grid.entries.extend(entries);
        }
        grid.entries.sort_unstable();
        grid.entries.dedup();
        grid
    }

    fn cells(&self, [min, max]: [P; 2]) -> impl Iterator<Item = u64> {
        let index = |value: i32| (value as f64 / self.cell).floor() as i32;
        let (x0, x1, y0, y1) = (index(min[0]), index(max[0]), index(min[1]), index(max[1]));
        (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (u64::from(x as u32) << 32) | u64::from(y as u32)))
    }

    /// The items whose boxes meet a cell that `bounds` meets, each once.
    pub fn items(&self, bounds: impl IntoIterator<Item = [P; 2]>) -> Vec<u32> {
        let mut keys: Vec<u64> = bounds.into_iter().flat_map(|bounds| self.cells(bounds).collect::<Vec<_>>()).collect();
        keys.sort_unstable();
        keys.dedup();
        let mut items = Vec::new();
        for key in keys {
            let start = self.entries.partition_point(|entry| entry.0 < key);
            items.extend(self.entries[start..].iter().take_while(|entry| entry.0 == key).map(|entry| entry.1));
        }
        items.sort_unstable();
        items.dedup();
        items
    }
}

/// The box of each segment of `line`, for a grid of the line.
pub fn segment_boxes(line: &[P]) -> impl Iterator<Item = [P; 2]> + '_ {
    line.windows(2).map(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_area_is_at_its_nearest_edge_and_zero_when_crossed() {
        // A square of about 222 m at 47° N, and lines that pass 140 m and 160 m east of it.
        let (lon, lat) = (8_000_000, 47_000_000);
        let side = 2_000;
        let square = vec![[lon, lat], [lon, lat + side], [lon + side, lat + side], [lon + side, lat], [lon, lat]];
        let east = |metres: f64| lon + side + (metres / Plane::at(lat).xy([1, 0])[0]).round() as i32;
        let line = |x: i32| Line::new(&[[x, lat - 5_000], [x, lat + 5_000]]);
        let rings = [square.clone()];
        assert!((line(east(140.0)).to_area(&rings, 150.0) - 140.0).abs() < 0.5);
        assert!((line(east(160.0)).to_area(&rings, 3_000.0) - 160.0).abs() < 0.5);
        assert_eq!(line(east(160.0)).to_area(&rings, 150.0), f64::INFINITY);
        assert_eq!(line(lon + side / 2).to_area(&rings, 150.0), 0.0);
        // A line wholly inside, and one inside a hole.
        let inner = Line::new(&[[lon + 500, lat + 500], [lon + 600, lat + 600]]);
        assert_eq!(inner.to_area(&rings, 150.0), 0.0);
        let hole = vec![
            [lon + 400, lat + 400],
            [lon + 400, lat + 700],
            [lon + 700, lat + 700],
            [lon + 700, lat + 400],
            [lon + 400, lat + 400],
        ];
        assert!(inner.to_area(&[square, hole], 150.0) > 5.0);
    }
}
