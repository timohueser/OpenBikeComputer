//! The owned region R over the cell's window of level-17 cells, its strip S, the interior domain
//! D = (R ∖ S) ∩ cell and D's border.
//!
//! R is every cell whose open square meets the inside of a level-0 face with a rule other than
//! terrain, grown by `FACE_RING` rings, then grown by the strip rounds. The plans hold every face of
//! the 3 × 3 cells, so R is exact up to the window's edge before the strip rounds; each round reads
//! and changes R within one cell, so the cell and two rings stay exact (`REGION_RING`).

use super::params::{CELLS17, FACE_RING, L17, REGION_RING, STRIP_ROUNDS};
use super::{owned, MeshError, Window};

/// Level-17 cells along a side of the window.
const N: i32 = CELLS17 + 2 * REGION_RING;
/// The 8 neighbours in the order of a pattern's bits.
pub const NEIGHBOURS: [[i32; 2]; 8] = [[-1, -1], [0, -1], [1, -1], [1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0]];

pub struct Region {
    r: Vec<bool>,
    faces: Vec<bool>,
    /// Strip cells of the cell whose hole had no fill, so their neighbours joined R.
    pub strip_growths: u32,
}

/// A segment of D's border, D on its left (the side where the shoelace sum of (a, b, point) is
/// positive). `border`: a span on the cell line with R ∖ S on both sides; else the inner border.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bound {
    pub a: [i32; 2],
    pub b: [i32; 2],
    pub border: bool,
}

fn index(i: i32, j: i32) -> Option<usize> {
    let (x, y) = (i + REGION_RING, j + REGION_RING);
    ((0..N).contains(&x) && (0..N).contains(&y)).then_some((y * N + x) as usize)
}

impl Region {
    /// R of the window. `solvable(pattern)`: whether the strip fills the hole of a strip cell whose
    /// neighbours in R are the pattern's bits (`NEIGHBOURS`).
    pub fn new(window: &Window, solvable: impl Fn(u8) -> bool) -> Result<Region, MeshError> {
        let mut faces = vec![false; (N * N) as usize];
        for (shift, plan) in window.plans() {
            for face in plan.faces.iter().filter(|f| f.level == 0 && owned(f.rule)) {
                let rings: Vec<Vec<[i64; 2]>> = (face.rings.iter())
                    .map(|r| {
                        r.iter()
                            .map(|&v| {
                                let p = plan.vertices[v as usize];
                                [i64::from(p[0] + shift[0]), i64::from(p[1] + shift[1])]
                            })
                            .collect()
                    })
                    .collect();
                mark(&rings, &mut faces);
            }
        }
        let r = dilate(&faces, FACE_RING);
        let mut region = Region { r, faces, strip_growths: 0 };
        for _ in 0..STRIP_ROUNDS {
            let mut add = Vec::new();
            for j in -REGION_RING..CELLS17 + REGION_RING {
                for i in -REGION_RING..CELLS17 + REGION_RING {
                    if region.strip(i, j) && !solvable(region.pattern(i, j)) {
                        region.strip_growths += u32::from((0..CELLS17).contains(&i) && (0..CELLS17).contains(&j));
                        add.extend(NEIGHBOURS.iter().filter_map(|d| index(i + d[0], j + d[1])));
                    }
                }
            }
            if add.is_empty() {
                break;
            }
            add.into_iter().for_each(|k| region.r[k] = true);
        }
        for j in 0..CELLS17 {
            for i in 0..CELLS17 {
                if region.strip(i, j) && !solvable(region.pattern(i, j)) {
                    return Err(MeshError::StripUnsolved { at: [i * L17, j * L17] });
                }
            }
        }
        Ok(region)
    }

    /// Whether level-17 cell (i, j) of the cell's frame lies in R; false past the window.
    pub fn r(&self, i: i32, j: i32) -> bool {
        index(i, j).is_some_and(|k| self.r[k])
    }

    /// Whether a face other than terrain meets the cell.
    pub fn face(&self, i: i32, j: i32) -> bool {
        index(i, j).is_some_and(|k| self.faces[k])
    }

    /// A strip cell: in R with an 8-neighbour outside R.
    pub fn strip(&self, i: i32, j: i32) -> bool {
        self.r(i, j) && NEIGHBOURS.iter().any(|d| !self.r(i + d[0], j + d[1]))
    }

    /// R ∖ S.
    pub fn interior(&self, i: i32, j: i32) -> bool {
        self.r(i, j) && !self.strip(i, j)
    }

    /// D: R ∖ S in the cell.
    pub fn d(&self, i: i32, j: i32) -> bool {
        (0..CELLS17).contains(&i) && (0..CELLS17).contains(&j) && self.interior(i, j)
    }

    /// Whether a point of the cell's frame lies in a cell of D (a point on a cell line in the one east
    /// or south of it).
    pub fn in_d(&self, p: [i64; 2]) -> bool {
        let c = p.map(|v| v.div_euclid(i64::from(L17)) as i32);
        self.d(c[0], c[1])
    }

    /// The neighbours of a cell in R, as bits of `NEIGHBOURS`.
    pub fn pattern(&self, i: i32, j: i32) -> u8 {
        NEIGHBOURS.iter().enumerate().fold(0, |bits, (k, d)| bits | u8::from(self.r(i + d[0], j + d[1])) << k)
    }

    /// The faces' cells of the cell that lie outside D: none by construction (the faces' growth).
    pub fn faces_outside(&self) -> u32 {
        let mut n = 0;
        for j in 0..CELLS17 {
            for i in 0..CELLS17 {
                n += u32::from(self.face(i, j) && !self.d(i, j));
            }
        }
        n
    }

    /// D's border, cell by cell in rows, each cell's sides north, east, south, west.
    pub fn bounds(&self) -> Vec<Bound> {
        let mut out = Vec::new();
        for j in 0..CELLS17 {
            for i in 0..CELLS17 {
                if !self.d(i, j) {
                    continue;
                }
                let (x0, y0, x1, y1) = (i * L17, j * L17, (i + 1) * L17, (j + 1) * L17);
                for (n, a, b) in [
                    ([i, j - 1], [x0, y0], [x1, y0]),
                    ([i + 1, j], [x1, y0], [x1, y1]),
                    ([i, j + 1], [x1, y1], [x0, y1]),
                    ([i - 1, j], [x0, y1], [x0, y0]),
                ] {
                    if self.d(n[0], n[1]) {
                        continue;
                    }
                    out.push(Bound { a, b, border: self.interior(n[0], n[1]) });
                }
            }
        }
        out
    }
}

fn cross(a: [i64; 2], b: [i64; 2], p: [i64; 2]) -> i128 {
    i128::from(b[0] - a[0]) * i128::from(p[1] - a[1]) - i128::from(b[1] - a[1]) * i128::from(p[0] - a[0])
}

/// Whether the closed segment ab meets the open square of level-17 cell (i, j): the separating axes
/// of the square's sides and of the segment's normal, exact.
fn meets_open(a: [i64; 2], b: [i64; 2], i: i32, j: i32) -> bool {
    let s = i64::from(L17);
    let (x0, y0) = (i64::from(i) * s, i64::from(j) * s);
    let (x1, y1) = (x0 + s, y0 + s);
    if a[0].max(b[0]) <= x0 || a[0].min(b[0]) >= x1 || a[1].max(b[1]) <= y0 || a[1].min(b[1]) >= y1 {
        return false;
    }
    let c = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| cross(a, b, p));
    c.iter().any(|&v| v < 0) && c.iter().any(|&v| v > 0)
}

/// Marks the cells of the window whose open square meets the inside of a polygon (rings of frame
/// units in the cell's frame): the cells that an edge enters, and the cells whose centre lies inside
/// (even-odd over all rings), which an edge does not enter.
fn mark(rings: &[Vec<[i64; 2]>], out: &mut [bool]) {
    let s = i64::from(L17);
    let (lo, hi) = (-REGION_RING, CELLS17 + REGION_RING - 1);
    let cell = |v: i64| (v.div_euclid(s)).clamp(i64::from(lo) - 1, i64::from(hi) + 1) as i32;
    let edges = || rings.iter().flat_map(|r| (0..r.len()).map(move |k| (r[k], r[(k + 1) % r.len()])));
    let mut bbox = [i64::MAX, i64::MAX, i64::MIN, i64::MIN];
    for (a, b) in edges() {
        bbox = [bbox[0].min(a[0]), bbox[1].min(a[1]), bbox[2].max(a[0]), bbox[3].max(a[1])];
        for j in cell(a[1].min(b[1])).max(lo)..=cell(a[1].max(b[1])).min(hi) {
            for i in cell(a[0].min(b[0])).max(lo)..=cell(a[0].max(b[0])).min(hi) {
                if meets_open(a, b, i, j) {
                    out[index(i, j).unwrap()] = true;
                }
            }
        }
    }
    if bbox[0] > bbox[2] {
        return;
    }
    let half = s as f64 / 2.0;
    for j in cell(bbox[1]).max(lo)..=cell(bbox[3]).min(hi) {
        let y = (i64::from(j) * s) as f64 + half;
        let mut xs: Vec<f64> = edges()
            .filter(|(a, b)| (a[1] as f64 <= y) != (b[1] as f64 <= y))
            .map(|(a, b)| a[0] as f64 + (y - a[1] as f64) * (b[0] - a[0]) as f64 / (b[1] - a[1]) as f64)
            .collect();
        xs.sort_by(f64::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            let first = ((pair[0] - half) / s as f64).ceil() as i64;
            let last = ((pair[1] - half) / s as f64).floor() as i64;
            for i in first.max(i64::from(lo))..=last.min(i64::from(hi)) {
                out[index(i as i32, j).unwrap()] = true;
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

    #[test]
    fn a_square_marks_the_cells_its_inside_meets_and_not_those_it_touches() {
        let mut out = vec![false; (N * N) as usize];
        // From (1, 1) to (3, 2) level-17 cells, on the cell lines: cells (1, 1) and (2, 1) only.
        let s = i64::from(L17);
        mark(&[vec![[s, s], [3 * s, s], [3 * s, 2 * s], [s, 2 * s]]], &mut out);
        let marked: Vec<(i32, i32)> =
            (0..N * N).filter(|&k| out[k as usize]).map(|k| (k % N - REGION_RING, k / N - REGION_RING)).collect();
        assert_eq!(marked, [(1, 1), (2, 1)]);
        // A thin sliver inside one cell marks it; a large square marks its inside without an edge.
        let mut out = vec![false; (N * N) as usize];
        mark(&[vec![[10, 10], [20, 10], [20, 11]]], &mut out);
        assert!(out[index(0, 0).unwrap()]);
        let mut out = vec![false; (N * N) as usize];
        mark(&[vec![[-s / 2, -s / 2], [9 * s + 1, -s / 2], [9 * s + 1, 9 * s + 1], [-s / 2, 9 * s + 1]]], &mut out);
        assert_eq!(out.iter().filter(|&&m| m).count(), 11 * 11);
    }

    #[test]
    fn dilation_grows_by_chebyshev_rings() {
        let mut cells = vec![false; (N * N) as usize];
        cells[index(5, 5).unwrap()] = true;
        let grown = dilate(&cells, 2);
        assert_eq!(grown.iter().filter(|&&m| m).count(), 25);
        assert!(grown[index(7, 3).unwrap()] && !grown[index(8, 5).unwrap()]);
    }
}
