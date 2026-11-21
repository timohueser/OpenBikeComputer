//! Snap rounding (Hobby; Guibas and Marimont): segments with whole endpoints, some of which cross, become polylines
//! through whole points that cross no other. A hot pixel is the half-open unit square [x − ½, x + ½) × [y − ½, y + ½)
//! about each endpoint and about each crossing of two segments; each segment runs through the centre of every hot pixel
//! that it meets, in order along it. Two polylines then meet only at such centres, or run along each other.

pub type P = [i32; 2];

/// The side of a bucket of hot pixels, in units.
const BUCKET: i64 = 64;

fn orient(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i128 {
    i128::from(b[0] - a[0]) * i128::from(p[1] - a[1]) - i128::from(b[1] - a[1]) * i128::from(p[0] - a[0])
}

fn wide(p: P) -> [i64; 2] {
    p.map(i64::from)
}

/// The hot pixel of the crossing of segments ab and cd, where they cross at a point inside both.
fn crossing(a: P, b: P, c: P, d: P) -> Option<P> {
    let (a, b, c, d) = (wide(a), wide(b), wide(c), wide(d));
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    if o1.signum() * o2.signum() >= 0 || o3.signum() * o4.signum() >= 0 {
        return None;
    }
    // a + t (b − a) with t = o3 / (o3 − o4); the pixel's centre is ⌊x + ½⌋.
    let den = o3 - o4;
    let (den, o3) = if den < 0 { (-den, -o3) } else { (den, o3) };
    Some([0, 1].map(|k| {
        let num = i128::from(a[k]) * den + o3 * i128::from(b[k] - a[k]);
        (2 * num + den).div_euclid(2 * den) as i32
    }))
}

/// A bound of a parameter t: n / d with d > 0, and whether it holds its own value.
#[derive(Clone, Copy)]
struct Bound {
    n: i128,
    d: i128,
    closed: bool,
}

impl Bound {
    fn of(n: i128, d: i128, closed: bool) -> Bound {
        if d < 0 {
            Bound { n: -n, d: -d, closed }
        } else {
            Bound { n, d, closed }
        }
    }

    fn cmp(&self, o: &Bound) -> std::cmp::Ordering {
        (self.n * o.d).cmp(&(o.n * self.d))
    }
}

/// Whether segment ab meets the half-open hot pixel about h: exact, in doubled units.
fn meets(a: P, b: P, h: P) -> bool {
    let (a, b, h) = (wide(a).map(|v| 2 * v), wide(b).map(|v| 2 * v), wide(h).map(|v| 2 * v));
    let mut lo = Bound { n: 0, d: 1, closed: true };
    let mut hi = Bound { n: 1, d: 1, closed: true };
    for k in 0..2 {
        let (from, to) = (i128::from(h[k] - 1), i128::from(h[k] + 1));
        let (p, d) = (i128::from(a[k]), i128::from(b[k] - a[k]));
        if d == 0 {
            if p < from || p >= to {
                return false;
            }
            continue;
        }
        // from ≤ p + t d < to.
        let (first, second) = (Bound::of(from - p, d, true), Bound::of(to - p, d, false));
        let (low, high) = if d > 0 { (first, second) } else { (second, first) };
        let tighter = |x: Bound, y: Bound, raise: bool| match x.cmp(&y) {
            std::cmp::Ordering::Equal => Bound { closed: x.closed && y.closed, ..x },
            o if (o == std::cmp::Ordering::Greater) == raise => x,
            _ => y,
        };
        lo = tighter(lo, low, true);
        hi = tighter(hi, high, false);
    }
    match lo.cmp(&hi) {
        std::cmp::Ordering::Less => true,
        std::cmp::Ordering::Equal => lo.closed && hi.closed,
        std::cmp::Ordering::Greater => false,
    }
}

/// The polylines of `segments`, each from its first point to its last, with hot pixels about `points` too. Only the
/// pairs with a segment of `movers` may cross: the others meet at most at their ends.
pub fn snap(segments: &[[P; 2]], movers: &[usize], points: &[P]) -> Vec<Vec<P>> {
    let mut hot: Vec<P> = segments.iter().flatten().chain(points).copied().collect();
    let boxes: Vec<[i32; 4]> =
        (segments.iter()).map(|&[a, b]| [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])]).collect();
    let overlap = |i: usize, j: usize| {
        let (p, q) = (boxes[i], boxes[j]);
        p[0] <= q[2] && q[0] <= p[2] && p[1] <= q[3] && q[1] <= p[3]
    };
    let mut moving = vec![false; segments.len()];
    for &i in movers {
        moving[i] = true;
    }
    for &i in movers {
        for j in (0..segments.len()).filter(|&j| j != i && !(moving[j] && j < i) && overlap(i, j)) {
            let ([a, b], [c, d]) = (segments[i], segments[j]);
            hot.extend(crossing(a, b, c, d));
        }
    }
    // The hot pixels by bucket, each once.
    let bucket = |v: i64| v.div_euclid(BUCKET);
    let key = |h: &P| wide(*h).map(bucket);
    hot.sort_unstable_by_key(|h| (key(h), *h));
    hot.dedup();
    let in_bucket = |k: [i64; 2]| &hot[hot.partition_point(|h| key(h) < k)..hot.partition_point(|h| key(h) <= k)];
    (segments.iter())
        .map(|&[a, b]| {
            let (aw, bw) = (wide(a), wide(b));
            let mut on: Vec<P> = Vec::new();
            let (x0, x1) = (aw[0].min(bw[0]) - 1, aw[0].max(bw[0]) + 1);
            for bx in bucket(x0)..=bucket(x1) {
                // The rows of the segment over this column, padded: a superset of the pixels that it meets.
                let (cx0, cx1) = ((bx * BUCKET).max(x0) as f64, ((bx + 1) * BUCKET).min(x1) as f64);
                let (ya, yb) = if aw[0] == bw[0] {
                    (aw[1] as f64, bw[1] as f64)
                } else {
                    let y =
                        |x: f64| aw[1] as f64 + (x - aw[0] as f64) * (bw[1] - aw[1]) as f64 / (bw[0] - aw[0]) as f64;
                    (y(cx0), y(cx1))
                };
                let (lo, hi) = (ya.min(yb).floor() as i64 - 2, ya.max(yb).ceil() as i64 + 2);
                let (lo, hi) = (lo.max(aw[1].min(bw[1]) - 1), hi.min(aw[1].max(bw[1]) + 1));
                for by in bucket(lo)..=bucket(hi) {
                    for &h in in_bucket([bx, by]) {
                        if h != a && h != b && meets(a, b, h) {
                            on.push(h);
                        }
                    }
                }
            }
            let along = |p: &P| {
                i128::from(wide(*p)[0] - aw[0]) * i128::from(bw[0] - aw[0])
                    + i128::from(wide(*p)[1] - aw[1]) * i128::from(bw[1] - aw[1])
            };
            on.sort_unstable_by(|p, q| along(p).cmp(&along(q)).then(p.cmp(q)));
            on.dedup();
            [vec![a], on, vec![b]].concat()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Whether the open parts of two segments cross or one's end lies inside the other.
    fn touch(p: [P; 2], q: [P; 2]) -> bool {
        let (a, b, c, d) = (wide(p[0]), wide(p[1]), wide(q[0]), wide(q[1]));
        let (o1, o2, o3, o4) = (orient(a, b, c), orient(a, b, d), orient(c, d, a), orient(c, d, b));
        o1.signum() * o2.signum() < 0 && o3.signum() * o4.signum() < 0
    }

    #[test]
    fn crossing_segments_become_polylines_that_meet_only_at_whole_points() {
        // A fan of segments from one point and a ring piece across them, near a vertex and near-parallel to one.
        let segments = [
            [[0, 0], [1000, 3]],
            [[0, 0], [997, 400]],
            [[0, 0], [10, 1000]],
            [[1000, 4], [1001, 900]],
            [[5, 1], [990, 2]],
            [[3, -50], [7, 60]],
        ];
        let out = snap(&segments, &[4, 5], &[]);
        let pieces: Vec<[P; 2]> = out.iter().flat_map(|l| l.windows(2).map(|w| [w[0], w[1]])).collect();
        for (i, p) in pieces.iter().enumerate() {
            for q in &pieces[i + 1..] {
                assert!(!touch(*p, *q), "{p:?} crosses {q:?}");
            }
        }
        // The ring piece and the first segment cross near (5, 1): both run through the same whole point there.
        assert!(out[4].len() > 2 && out[0].len() > 2);
        assert_eq!(out[3], segments[3].to_vec(), "a segment that meets no other's pixel stays");
    }

    #[test]
    fn a_pixel_holds_its_west_and_south_sides_only() {
        // The segment touches the pixel about (0, 0) only at its north-east corner (½, ½), which the pixel about (1, 1)
        // holds.
        let (a, b) = ([-1, 2], [1, 0]);
        assert!(!meets(a, b, [0, 0]));
        assert!(meets(a, b, [1, 1]) && meets(a, b, [0, 1]) && meets(a, b, [1, 0]));
    }
}
