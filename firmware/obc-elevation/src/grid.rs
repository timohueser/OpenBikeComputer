//! The sample lattice: integer arithmetic from a µdeg coordinate to a lattice index, a cell index,
//! and a tile-local sample (`OBCT_Spec.md` §1, §3).
//!
//! Deliberately **integer-only and float-free**, for the same reason OBCA's grid is: the whole
//! contract rests on two implementations landing on the same sample from the same coordinate, and a
//! float would make that a property of the FPU. Everything here is a shift or a mask over an `i64`
//! widened from the `int32` µdeg coordinate — no division, no rounding, no `libm`.

use obc_formats::obct::{GRID_ORIGIN, WORLD_SIDE};

/// Where a query coordinate lands on the lattice: the index of the sample at or below it on each
/// axis, plus the sub-posting remainder that becomes the bilinear weight (`OBCT_Spec.md` §5.1).
///
/// `frac_*` is in µdeg, `0..P`, **not** a normalized fraction — keeping it in the coordinate's own
/// unit is what lets §5.2's interpolation stay in integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lattice {
    /// Latitude sample index: `floor((lat − GRID_ORIGIN) / P)`.
    pub i: u32,
    /// Longitude sample index: `floor((lon − GRID_ORIGIN) / P)`.
    pub j: u32,
    /// `lat − lattice_lat(i)`, µdeg, in `0..P`.
    pub frac_lat: u32,
    /// `lon − lattice_lon(j)`, µdeg, in `0..P`.
    pub frac_lon: u32,
}

/// Locate `(lat, lon)` on the lattice of posting `2^posting_log2`, or `None` when the coordinate is
/// outside the world box (`OBCT_Spec.md` §1.1). The world box is bigger than the geographic domain,
/// so this rejects only genuinely impossible coordinates — a `0` from an unfixed GPS is *inside* it
/// and is the sampler's business to answer, not this function's.
#[inline]
pub fn locate(lat_udeg: i32, lon_udeg: i32, posting_log2: u8) -> Option<Lattice> {
    let (i, frac_lat) = axis(lat_udeg, posting_log2)?;
    let (j, frac_lon) = axis(lon_udeg, posting_log2)?;
    Some(Lattice { i, j, frac_lat, frac_lon })
}

/// One axis of [`locate`]. Widened to `i64` before the subtraction: `lat − GRID_ORIGIN` adds `2^28`
/// to a value that may legally be any `int32`, which overflows `i32` near its top end.
#[inline]
fn axis(v_udeg: i32, posting_log2: u8) -> Option<(u32, u32)> {
    let offset = v_udeg as i64 - GRID_ORIGIN as i64;
    if offset < 0 || offset >= WORLD_SIDE as i64 {
        return None;
    }
    let mask = (1i64 << posting_log2) - 1;
    Some(((offset >> posting_log2) as u32, (offset & mask) as u32))
}

/// The µdeg coordinate of lattice index `i` on either axis: `GRID_ORIGIN + i·P`.
#[inline]
pub fn lattice_coord(i: u32, posting_log2: u8) -> i32 {
    (GRID_ORIGIN as i64 + ((i as i64) << posting_log2)) as i32
}

/// The cell index owning lattice index `i`, for a cell of `2^cell_log2` µdeg at this posting.
/// A pure shift, which is exactly why the cell side is a power of two on the same origin: cell
/// membership is derivable from the lattice index with no table and no search.
#[inline]
pub fn cell_of(i: u32, posting_log2: u8, cell_log2: u8) -> u32 {
    i >> (cell_log2 - posting_log2)
}

/// The lattice index of a cell's **min** sample on either axis — the sample the cell owns under the
/// half-open rule (`OBCT_Spec.md` §3.1).
#[inline]
pub fn cell_base_sample(cell: u32, posting_log2: u8, cell_log2: u8) -> u32 {
    cell << (cell_log2 - posting_log2)
}

/// Number of cells along one axis of the world box at `2^cell_log2` µdeg (OBCA §1.1).
#[inline]
pub fn axis_cells(cell_log2: u8) -> u32 {
    WORLD_SIDE >> cell_log2
}

/// Zero-padding width of a cell index in a canonical cell id (`OBCA_Spec.md` §1.3):
/// `max(4, digits(axis_cells − 1))`. Four digits at `2^16` and above, wider below.
///
/// The rule is here, in the one crate both sides of the terrain pipeline already depend on,
/// because it is **content addressing**, not formatting: `18/1204/52` and `18/01204/1052` are two
/// strings for one cell, and a store keyed by the string would then hold the same square twice
/// under two names. `obc-pack`'s `grid::id_width` and `obc-dem`'s `bake::cell_file_name` are the
/// two producers of those strings, they cannot see each other (a host tool must not depend on the
/// packer), and a second transcription of `max(4, …)` is exactly how the two would drift apart.
///
/// Integer-only and allocation-free, like everything else in this crate — the digit count is a
/// loop, not a `to_string().len()`.
pub fn id_width(cell_log2: u8) -> usize {
    let mut v = axis_cells(cell_log2) - 1;
    let mut digits = 1;
    while v >= 10 {
        v /= 10;
        digits += 1;
    }
    if digits < 4 {
        4
    } else {
        digits
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lattice is anchored on the OBCA origin, not on the query — the same coordinate must land
    /// on the same index whatever file asks.
    #[test]
    fn the_origin_is_lattice_index_zero_on_both_axes() {
        let at_origin = locate(GRID_ORIGIN, GRID_ORIGIN, 9).unwrap();
        assert_eq!(at_origin, Lattice { i: 0, j: 0, frac_lat: 0, frac_lon: 0 });
        assert_eq!(lattice_coord(0, 9), GRID_ORIGIN);
        // One posting in, on latitude only.
        let one = locate(GRID_ORIGIN + 512, GRID_ORIGIN, 9).unwrap();
        assert_eq!((one.i, one.j, one.frac_lat), (1, 0, 0));
        assert_eq!(lattice_coord(1, 9), GRID_ORIGIN + 512);
    }

    #[test]
    fn a_coordinate_splits_into_index_and_sub_posting_remainder() {
        // 47°N / 8°E at the v1 posting.
        let l = locate(47_000_000, 8_000_000, 9).unwrap();
        assert_eq!(l.i, ((47_000_000i64 + 268_435_456) >> 9) as u32);
        assert_eq!(lattice_coord(l.i, 9) as i64 + l.frac_lat as i64, 47_000_000);
        assert_eq!(lattice_coord(l.j, 9) as i64 + l.frac_lon as i64, 8_000_000);
        assert!(l.frac_lat < 512 && l.frac_lon < 512);
    }

    #[test]
    fn the_world_box_is_half_open_and_int32_safe() {
        assert!(locate(GRID_ORIGIN - 1, 0, 9).is_none());
        assert!(locate(0, GRID_ORIGIN - 1, 9).is_none());
        let last = GRID_ORIGIN + WORLD_SIDE as i32 - 1;
        assert!(locate(last, last, 9).is_some());
        assert!(locate(i32::MAX, 0, 9).is_none(), "the widening keeps this a rejection, not a wrap");
        assert!(locate(i32::MIN, 0, 9).is_none());
    }

    /// Cell membership and the lattice must agree in both directions: the base sample of the cell a
    /// sample belongs to is never above that sample, and never a whole cell below it.
    #[test]
    fn cell_membership_agrees_with_the_lattice_at_every_v1_pairing() {
        let (posting_log2, cell_log2) = (9u8, 19u8);
        let span = 1u32 << (cell_log2 - posting_log2);
        for i in [0u32, 1, span - 1, span, span + 1, 12_345, u32::MAX >> cell_log2] {
            let c = cell_of(i, posting_log2, cell_log2);
            let base = cell_base_sample(c, posting_log2, cell_log2);
            assert!(base <= i && i - base < span, "sample {i} sits in cell {c} at base {base}");
            assert_eq!(cell_of(base, posting_log2, cell_log2), c, "a cell's base sample is its own");
        }
        assert_eq!(axis_cells(19), 1 << 10);
        assert_eq!(axis_cells(28), 2, "the largest legal cell tiles the world box twice per axis");
    }
}
