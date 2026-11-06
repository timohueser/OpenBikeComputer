//! The owned region R about a cell, its strip S and the interior domain D = (R ∖ S) ∩ cell.
//!
//! R is every region cell whose open square meets the inside of a sharp face, grown by `FACE_RING`
//! rings. S is R's outer ring: the cells of R with an 8-neighbour outside R. R is a union of 5 × 5
//! squares, so S is one cell wide and every cell of D lies a cell or more from ∂R.
//!
//! Each cell marks its own sharp cells (`sharp_cells`); R about a cell reads its neighbours' marks
//! within `RING` cells, so two cells agree on R wherever both read it.

use super::params::{sharp, CELLS, COARSE, FACE_RING, GRID};
use crate::faces::FacePlan;

/// The window of sharp marks about the cell: R is exact on the cell and two rings, which S and the
/// spans of the cell's lines read.
const RING: i32 = FACE_RING + 2;
/// Level-17 cells along a side of the window.
const N: i32 = CELLS + 2 * RING;
/// The 8 neighbours.
const NEIGHBOURS: [[i32; 2]; 8] = [[-1, -1], [0, -1], [1, -1], [1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0]];
/// Words of a cell's sharp marks: one bit per region cell, row by row.
pub const MARK_WORDS: usize = (CELLS * CELLS / 64) as usize;
/// Words of R on the cell and one ring (`Region::bits`).
pub const REGION_WORDS: usize = (((CELLS + 2) * (CELLS + 2)) as usize).div_ceil(32);

#[derive(PartialEq)]
pub struct Region {
    r: Vec<bool>,
}

/// A side of a cell of D that faces a cell outside D, D on its left (the side where the shoelace sum
/// of (a, b, point) is positive). `span`: on the cell line with R ∖ S of the next cell beyond it;
/// else the inner border, with S beyond it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bound {
    pub a: [i32; 2],
    pub b: [i32; 2],
    pub span: bool,
}

fn index(i: i32, j: i32) -> Option<usize> {
    let (x, y) = (i + RING, j + RING);
    ((0..N).contains(&x) && (0..N).contains(&y)).then_some((y * N + x) as usize)
}

impl Region {
    /// R from the sharp marks of the cell and its neighbours, by (dx, dy) of the neighbour: a missing
    /// neighbour has none.
    pub fn new(marks: &[([i32; 2], &[u64])]) -> Region {
        let mut faces = vec![false; (N * N) as usize];
        for &(d, bits) in marks {
            for j in 0..CELLS {
                for i in 0..CELLS {
                    let k = (j * CELLS + i) as usize;
                    if bits[k / 64] >> (k % 64) & 1 == 1 {
                        if let Some(at) = index(i + d[0] * CELLS, j + d[1] * CELLS) {
                            faces[at] = true;
                        }
                    }
                }
            }
        }
        Region { r: dilate(&faces, FACE_RING) }
    }

    /// Whether region cell (i, j) of the cell's frame lies in R.
    pub fn r(&self, i: i32, j: i32) -> bool {
        index(i, j).is_some_and(|k| self.r[k])
    }

    /// A strip cell: in R with an 8-neighbour outside R.
    pub fn strip(&self, i: i32, j: i32) -> bool {
        self.r(i, j) && NEIGHBOURS.iter().any(|d| !self.r(i + d[0], j + d[1]))
    }

    fn interior(&self, i: i32, j: i32) -> bool {
        self.r(i, j) && !self.strip(i, j)
    }

    /// D: R ∖ S in the cell.
    pub fn d(&self, i: i32, j: i32) -> bool {
        (0..CELLS).contains(&i) && (0..CELLS).contains(&j) && self.interior(i, j)
    }

    /// The region cells that touch grid point (i, j) of the cell: one, two or four.
    fn touching(i: i32, j: i32) -> impl Iterator<Item = [i32; 2]> {
        let span = |v: i32| if v % 2 == 0 { (v / 2 - 1)..=(v / 2) } else { (v / 2)..=(v / 2) };
        span(j).flat_map(move |cy| span(i).map(move |cx| [cx, cy]))
    }

    /// Whether grid point (i, j) lies on ∂R: it touches a cell of R and one outside.
    pub fn on_boundary(&self, i: i32, j: i32) -> bool {
        let (mut inside, mut outside) = (false, false);
        for c in Self::touching(i, j) {
            if self.r(c[0], c[1]) {
                inside = true;
            } else {
                outside = true;
            }
        }
        inside && outside
    }

    /// R on the cell and one ring: bit (j + 1) × (`CELLS` + 2) + i + 1 for region cell (i, j),
    /// −1 ≤ i, j ≤ `CELLS`.
    pub fn bits(&self) -> Vec<u32> {
        let n = CELLS + 2;
        let mut out = vec![0u32; REGION_WORDS];
        for j in -1..=CELLS {
            for i in -1..=CELLS {
                if self.r(i, j) {
                    let k = ((j + 1) * n + i + 1) as usize;
                    out[k / 32] |= 1 << (k % 32);
                }
            }
        }
        out
    }

    /// The cells of R, S and D in the cell.
    pub fn counts(&self) -> [u32; 3] {
        let mut c = [0; 3];
        for j in 0..CELLS {
            for i in 0..CELLS {
                c[0] += u32::from(self.r(i, j));
                c[1] += u32::from(self.strip(i, j));
                c[2] += u32::from(self.d(i, j));
            }
        }
        c
    }

    /// D's border, cell by cell in rows, each cell's sides north, east, south, west.
    pub fn bounds(&self) -> Vec<Bound> {
        let mut out = Vec::new();
        for j in 0..CELLS {
            for i in 0..CELLS {
                if !self.d(i, j) {
                    continue;
                }
                let (x0, y0, x1, y1) = (i * COARSE, j * COARSE, (i + 1) * COARSE, (j + 1) * COARSE);
                for (n, a, b) in [
                    ([i, j - 1], [x0, y0], [x1, y0]),
                    ([i + 1, j], [x1, y0], [x1, y1]),
                    ([i, j + 1], [x1, y1], [x0, y1]),
                    ([i - 1, j], [x0, y1], [x0, y0]),
                ] {
                    if !self.d(n[0], n[1]) {
                        out.push(Bound { a, b, span: self.interior(n[0], n[1]) });
                    }
                }
            }
        }
        out
    }

    /// Whether grid point (i, j) lies inside D, off D's border: every cell that it touches
    /// is in D.
    pub fn inside_d(&self, i: i32, j: i32) -> bool {
        Self::touching(i, j).all(|c| self.d(c[0], c[1]))
    }
}

/// The grid point of a frame point on the grid.
pub fn grid_of(p: [i32; 2]) -> Option<[i32; 2]> {
    (p[0] % GRID == 0 && p[1] % GRID == 0).then(|| [p[0] / GRID, p[1] / GRID])
}

/// The cell's sharp marks: bit i + `CELLS` · j where region cell (i, j) of the cell's open square
/// meets the inside of a sharp face.
pub fn sharp_cells(plan: &FacePlan) -> Vec<u64> {
    let mut out = vec![0u64; MARK_WORDS];
    for face in plan.faces.iter().filter(|f| f.level == 0 && sharp(f.rule)) {
        let rings: Vec<Vec<[i64; 2]>> = (face.rings.iter())
            .map(|r| r.iter().map(|&v| plan.vertices[v as usize].map(i64::from)).collect())
            .collect();
        mark(&rings, &mut |i, j| {
            let k = (j * CELLS + i) as usize;
            out[k / 64] |= 1 << (k % 64);
        });
    }
    out
}

fn cross(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i128 {
    i128::from(b[0] - a[0]) * i128::from(p[1] - a[1]) - i128::from(b[1] - a[1]) * i128::from(p[0] - a[0])
}

/// Whether the closed segment ab meets the open square of region cell (i, j): the separating axes
/// of the square's sides and of the segment's normal, exact.
fn meets_open(a: [i64; 2], b: [i64; 2], i: i32, j: i32) -> bool {
    let s = i64::from(COARSE);
    let (x0, y0) = (i64::from(i) * s, i64::from(j) * s);
    let (x1, y1) = (x0 + s, y0 + s);
    if a[0].max(b[0]) <= x0 || a[0].min(b[0]) >= x1 || a[1].max(b[1]) <= y0 || a[1].min(b[1]) >= y1 {
        return false;
    }
    let c = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| cross(a, b, p));
    c.iter().any(|&v| v < 0) && c.iter().any(|&v| v > 0)
}

/// Marks the cells of the cell whose open square meets the inside of a polygon (rings in the cell's
/// frame): the cells that an edge enters, and the cells whose centre lies inside (even-odd over all
/// rings), which no edge enters.
fn mark(rings: &[Vec<[i64; 2]>], set: &mut dyn FnMut(i32, i32)) {
    let s = i64::from(COARSE);
    let cell = |v: i64| v.div_euclid(s).clamp(-1, i64::from(CELLS)) as i32;
    let edges = || rings.iter().flat_map(|r| (0..r.len()).map(move |k| (r[k], r[(k + 1) % r.len()])));
    let mut bbox = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for (a, b) in edges() {
        bbox = [bbox[0].min(a[0]), bbox[1].min(a[1]), bbox[2].max(a[0]), bbox[3].max(a[1])];
        for j in cell(a[1].min(b[1])).max(0)..=cell(a[1].max(b[1])).min(CELLS - 1) {
            for i in cell(a[0].min(b[0])).max(0)..=cell(a[0].max(b[0])).min(CELLS - 1) {
                if meets_open(a, b, i, j) {
                    set(i, j);
                }
            }
        }
    }
    if bbox[0] > bbox[2] {
        return;
    }
    let half = s as f64 / 2.0;
    for j in cell(bbox[1]).max(0)..=cell(bbox[3]).min(CELLS - 1) {
        let y = (i64::from(j) * s) as f64 + half;
        let mut xs: Vec<f64> = edges()
            .filter(|(a, b)| (a[1] as f64 <= y) != (b[1] as f64 <= y))
            .map(|(a, b)| a[0] as f64 + (y - a[1] as f64) * (b[0] - a[0]) as f64 / (b[1] - a[1]) as f64)
            .collect();
        xs.sort_by(f64::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            let first = ((pair[0] - half) / s as f64).ceil() as i64;
            let last = ((pair[1] - half) / s as f64).floor() as i64;
            for i in first.max(0)..=last.min(i64::from(CELLS - 1)) {
                set(i as i32, j);
            }
        }
    }
}

/// The cells within `ring` cells (Chebyshev) of a marked cell.
fn dilate(cells: &[bool], ring: i32) -> Vec<bool> {
    let n = N as usize;
    let pass = |src: &[bool], along_rows: bool| {
        (0..src.len())
            .map(|k| {
                let (x, y) = ((k % n) as i32, (k / n) as i32);
                (-ring..=ring).any(|d| {
                    let (xx, yy) = if along_rows { (x + d, y) } else { (x, y + d) };
                    (0..N).contains(&xx) && (0..N).contains(&yy) && src[(yy * N + xx) as usize]
                })
            })
            .collect::<Vec<bool>>()
    };
    pass(&pass(cells, true), false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marked(rings: &[Vec<[i64; 2]>]) -> Vec<(i32, i32)> {
        let mut out = Vec::new();
        mark(rings, &mut |i, j| out.push((i, j)));
        out.sort_unstable();
        out.dedup();
        out
    }

    #[test]
    fn a_square_marks_the_cells_its_inside_meets_and_not_those_it_touches() {
        // From (1, 1) to (3, 2) region cells, on the cell lines: cells (1, 1) and (2, 1) only.
        let s = i64::from(COARSE);
        assert_eq!(marked(&[vec![[s, s], [3 * s, s], [3 * s, 2 * s], [s, 2 * s]]]), [(1, 1), (2, 1)]);
        // A thin sliver inside one cell marks it; a large square marks its inside without an edge.
        assert_eq!(marked(&[vec![[10, 10], [20, 10], [20, 11]]]), [(0, 0)]);
        let big = marked(&[vec![[-s / 2, -s / 2], [9 * s + 1, -s / 2], [9 * s + 1, 9 * s + 1], [-s / 2, 9 * s + 1]]]);
        assert_eq!(big.len(), 10 * 10);
    }

    #[test]
    fn a_neighbours_mark_reaches_the_cell_and_the_strip_rings_it() {
        // One sharp cell in the east neighbour's first column: R reaches two columns into the cell,
        // the outer one is the strip, and the points of ∂R and inside R follow.
        let mut bits = vec![0u64; MARK_WORDS];
        let k = (10 * CELLS) as usize;
        bits[k / 64] |= 1 << (k % 64);
        let region = Region::new(&[([1, 0], &bits)]);
        let last = CELLS - 1;
        assert!(region.r(last - 1, 10) && !region.r(last - 2, 10));
        assert!(region.strip(last - 1, 10) && region.d(last, 10) && !region.d(last, 8) && region.strip(last, 8));
        assert!(region.on_boundary(2 * (last - 1), 21) && !region.on_boundary(2 * last + 1, 21));
        assert_eq!(region.counts(), [10, 7, 3]);
    }
}
