//! The OBCA cell grid, as the **assembler** needs it: cell identity, the alignment arithmetic
//! ([`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §1–§2), and the assembly-bbox snap of §4.2.
//!
//! # Why this is not `obc_pack::grid`
//!
//! The cutter's copy of this arithmetic lives in `host/obc-pack/src/grid.rs`, and that crate carries
//! libGEOS — a native dependency the engine must not have (#1024: the core is GEOS-free and compiles
//! for `wasm32-unknown-unknown`). So the few dozen lines of integer arithmetic are restated here,
//! deliberately, rather than pulling a C++ geometry library into a browser tab.
//!
//! Restating a normative contract is a drift risk, so the drift is **tested**: the oracle suite
//! (`tests/oracle.rs`, which may dev-depend on obc-pack) asserts cell-for-cell that both copies
//! compute the same squares, the same containment, and the same boundary predicate. A divergence
//! fails a test rather than mis-grafting a map.
//!
//! Everything here is integer-only. The grid exists so an OBCM quadtree's floor-midpoint
//! subdivision lands *exactly* on cell boundaries (§2), and one rounding step in the wrong direction
//! would break that.

use core::fmt;

/// Origin of the fixed global cell grid, µdeg, on **both** axes (OBCA §1.1).
pub const GRID_ORIGIN: i64 = -(1 << 28);

/// Side of the world box, µdeg (OBCA §1.1): `2^29`. The grid does **not** wrap and cells may legally
/// overhang ±90 / ±180 (§1.4).
pub const WORLD_SIDE: i64 = 1 << 29;

/// Smallest permitted cell size as `log2(µdeg)` (OBCA §1.1).
pub const MIN_CELL_LOG2: u32 = 10;

/// Largest permitted cell size as `log2(µdeg)` (OBCA §1.1).
pub const MAX_CELL_LOG2: u32 = 28;

/// Largest permitted assembly-bbox span as `log2(µdeg)` (OBCA §2.1: `n ≤ 29`).
pub const MAX_SPAN_LOG2: u32 = 29;

/// A bbox in the serializer's order: `(min_lon, min_lat, max_lon, max_lat)`, µdeg — the order
/// `obc-pack`'s writer and `obc-reader`'s `BBox` both use, so nothing here has to swap axes.
pub type UBox = (i64, i64, i64, i64);

/// One cell of the grid: a size (`log2(µdeg)`) plus its **latitude** index `i` and **longitude**
/// index `j` (OBCA §1.1/§1.3 — the canonical id is `<log2>/<i>/<j>`, latitude first).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CellId {
    pub log2: u32,
    pub i: i64,
    pub j: i64,
}

/// Cells per axis at size `2^log2`.
#[inline]
pub fn axis_cells(log2: u32) -> i64 {
    WORLD_SIDE >> log2
}

fn decimal_width(mut v: i64) -> usize {
    let mut w = 1;
    while v >= 10 {
        v /= 10;
        w += 1;
    }
    w
}

/// Zero-padding width of a cell id's indices (OBCA §1.3): `max(4, digits(cells_per_axis − 1))`.
pub fn id_width(log2: u32) -> usize {
    decimal_width(axis_cells(log2) - 1).max(4)
}

impl CellId {
    /// A cell by size + indices, validated against the world box.
    pub fn new(log2: u32, i: i64, j: i64) -> Result<Self, String> {
        if !(MIN_CELL_LOG2..=MAX_CELL_LOG2).contains(&log2) {
            return Err(format!("cell size 2^{log2} µdeg is outside the grid's {MIN_CELL_LOG2}..={MAX_CELL_LOG2}"));
        }
        let n = axis_cells(log2);
        if !(0..n).contains(&i) || !(0..n).contains(&j) {
            return Err(format!("cell 2^{log2}/{i}/{j} is outside the world box (indices must be 0..{n})"));
        }
        Ok(CellId { log2, i, j })
    }

    /// Cell size in µdeg.
    #[inline]
    pub fn size(self) -> i64 {
        1 << self.log2
    }

    /// The cell's square, half-open on both axes (OBCA §1.1), in [`UBox`] order.
    #[inline]
    pub fn square(self) -> UBox {
        let s = self.size();
        let min_lat = GRID_ORIGIN + self.i * s;
        let min_lon = GRID_ORIGIN + self.j * s;
        (min_lon, min_lat, min_lon + s, min_lat + s)
    }

    /// The cell of size `2^log2` whose half-open square contains `(lat, lon)`.
    #[inline]
    pub fn containing(log2: u32, lat: i64, lon: i64) -> Self {
        CellId { log2, i: (lat - GRID_ORIGIN).div_euclid(1 << log2), j: (lon - GRID_ORIGIN).div_euclid(1 << log2) }
    }

    /// Parse a canonical id `<log2>/<i>/<j>` (OBCA §1.3). Lenient about zero padding in, canonical
    /// out.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut parts = s.split('/');
        let bad = || format!("cell id {s:?} is not <log2>/<i>/<j>");
        let (a, b, c) = match (parts.next(), parts.next(), parts.next(), parts.next()) {
            (Some(a), Some(b), Some(c), None) => (a, b, c),
            _ => return Err(bad()),
        };
        let log2: u32 = a.parse().map_err(|_| bad())?;
        let i: i64 = b.parse().map_err(|_| bad())?;
        let j: i64 = c.parse().map_err(|_| bad())?;
        Self::new(log2, i, j)
    }
}

impl fmt::Display for CellId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let w = id_width(self.log2);
        write!(f, "{}/{:0w$}/{:0w$}", self.log2, self.i, self.j, w = w)
    }
}

/// Whether `v` lies exactly on a grid line of size `2^log2` — the seam predicate OBCA §4.6 admits to
/// unification, and nothing weaker.
#[inline]
pub fn on_grid_line(v: i64, log2: u32) -> bool {
    (v - GRID_ORIGIN) & ((1 << log2) - 1) == 0
}

/// Whether `(lat, lon)` lies on any boundary line of the `2^log2` grid.
#[inline]
pub fn on_grid_boundary(lat: i64, lon: i64, log2: u32) -> bool {
    on_grid_line(lat, log2) || on_grid_line(lon, log2)
}

/// The floor-division midpoint the OBCM quadtree splits at (`OBCM_Spec.md` §4).
#[inline]
pub fn quad_mid(min: i64, max: i64) -> i64 {
    (min + max).div_euclid(2)
}

/// The four children of `b`, in the format's **NW, NE, SW, SE** order (`OBCM_Spec.md` §4).
#[inline]
pub fn quad_children(b: UBox) -> [UBox; 4] {
    let (min_lon, min_lat, max_lon, max_lat) = b;
    let mid_lon = quad_mid(min_lon, max_lon);
    let mid_lat = quad_mid(min_lat, max_lat);
    [
        (min_lon, mid_lat, mid_lon, max_lat), // NW
        (mid_lon, mid_lat, max_lon, max_lat), // NE
        (min_lon, min_lat, mid_lon, mid_lat), // SW
        (mid_lon, min_lat, max_lon, mid_lat), // SE
    ]
}

/// A grid-aligned power-of-two assembly bbox (OBCA §2.1) — the box the theorem holds over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AlignedBox {
    /// Minimum corner, µdeg. Congruent to [`GRID_ORIGIN`] modulo `S_MAX`.
    pub min_lat: i64,
    pub min_lon: i64,
    /// Side as `log2(µdeg)`; the box is square by construction.
    pub span_log2: u32,
}

impl AlignedBox {
    /// This box in [`UBox`] order.
    #[inline]
    pub fn ubox(self) -> UBox {
        let side = 1i64 << self.span_log2;
        (self.min_lon, self.min_lat, self.min_lon + side, self.min_lat + side)
    }

    /// Depth at which this box's quadtree nodes **are** the cells of size `2^cell_log2` (OBCA §2.1).
    #[inline]
    pub fn cell_depth(self, cell_log2: u32) -> u32 {
        self.span_log2 - cell_log2
    }

    /// Whether `cell`'s square lies inside this box.
    pub fn contains_cell(self, cell: CellId) -> bool {
        let (min_lon, min_lat, max_lon, max_lat) = self.ubox();
        let (c_min_lon, c_min_lat, c_max_lon, c_max_lat) = cell.square();
        c_min_lat >= min_lat && c_max_lat <= max_lat && c_min_lon >= min_lon && c_max_lon <= max_lon
    }

    /// The four children of this box as aligned boxes (NW, NE, SW, SE).
    pub fn children(self) -> [AlignedBox; 4] {
        let side = 1i64 << (self.span_log2 - 1);
        let span_log2 = self.span_log2 - 1;
        [
            AlignedBox { min_lat: self.min_lat + side, min_lon: self.min_lon, span_log2 }, // NW
            AlignedBox { min_lat: self.min_lat + side, min_lon: self.min_lon + side, span_log2 }, // NE
            AlignedBox { min_lat: self.min_lat, min_lon: self.min_lon, span_log2 },        // SW
            AlignedBox { min_lat: self.min_lat, min_lon: self.min_lon + side, span_log2 }, // SE
        ]
    }
}

/// OBCA §4.2: the **minimal** grid-aligned power-of-two box containing every cell of `cells`, with
/// its position snapped to `s_max_log2` and its span at least `2^s_max_log2`.
///
/// The box is square, so a selection much wider than tall is padded with empty leaves — one `uint32`
/// per empty node and nothing else. The assembler MUST NOT shrink it afterwards: that would destroy
/// the alignment the whole scheme rests on.
pub fn assembly_box(cells: &[CellId], s_max_log2: u32) -> Result<AlignedBox, String> {
    if cells.is_empty() {
        return Err("an assembly needs at least one cell".into());
    }
    let mut min_lat = i64::MAX;
    let mut min_lon = i64::MAX;
    let mut max_lat = i64::MIN;
    let mut max_lon = i64::MIN;
    for c in cells {
        let (c_min_lon, c_min_lat, c_max_lon, c_max_lat) = c.square();
        min_lat = min_lat.min(c_min_lat);
        min_lon = min_lon.min(c_min_lon);
        max_lat = max_lat.max(c_max_lat);
        max_lon = max_lon.max(c_max_lon);
    }
    let s_max = 1i64 << s_max_log2;
    let snap = |v: i64| GRID_ORIGIN + (v - GRID_ORIGIN).div_euclid(s_max) * s_max;
    let a_lat = snap(min_lat);
    let a_lon = snap(min_lon);
    let mut span_log2 = s_max_log2;
    while span_log2 <= MAX_SPAN_LOG2 {
        let side = 1i64 << span_log2;
        if a_lat + side >= max_lat && a_lon + side >= max_lon {
            return Ok(AlignedBox { min_lat: a_lat, min_lon: a_lon, span_log2 });
        }
        span_log2 += 1;
    }
    Err(format!(
        "the selection spans more than 2^{MAX_SPAN_LOG2} µdeg (lat {min_lat}..{max_lat}, lon {min_lon}..{max_lon}): \
         no legal assembly bbox exists (OBCA §2.1)"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_constants_and_nesting() {
        assert_eq!(GRID_ORIGIN, -268_435_456);
        assert_eq!(WORLD_SIDE, 536_870_912);
        for log2 in MIN_CELL_LOG2..=MAX_CELL_LOG2 {
            let s = 1i64 << log2;
            assert_eq!(GRID_ORIGIN % s, 0, "every permitted size divides the origin (2^{log2})");
            assert_eq!(axis_cells(log2) * s, WORLD_SIDE);
        }
    }

    /// OBCA §7's worked example, end to end: two neighbouring `2^18` cells, the `2^19` assembly box
    /// they snap into, and the depth at which the box's nodes *are* those cells.
    #[test]
    fn worked_example_box_and_depth() {
        let a = CellId::parse("18/1204/1052").expect("valid id");
        let b = CellId::parse("18/1204/1053").expect("valid id");
        assert_eq!(a.square(), (7_340_032, 47_185_920, 7_602_176, 47_448_064));
        assert_eq!(b.square(), (7_602_176, 47_185_920, 7_864_320, 47_448_064));
        // S_MAX = 2^18 for this toy table (§7 uses one band).
        let bx = assembly_box(&[a, b], 18).expect("aligned box");
        assert_eq!(bx.min_lat, 47_185_920);
        assert_eq!(bx.min_lon, 7_340_032);
        assert_eq!(bx.span_log2, 19, "the two-cell union needs a 2^19 square");
        assert_eq!(bx.ubox(), (7_340_032, 47_185_920, 7_864_320, 47_710_208));
        assert_eq!(bx.cell_depth(18), 1);
        // The depth-1 children in NW/NE/SW/SE order: SW is cell A, SE is cell B — to the microdegree.
        let kids = quad_children(bx.ubox());
        assert_eq!(kids[2], a.square(), "SW is cell A");
        assert_eq!(kids[3], b.square(), "SE is cell B");
    }

    /// The theorem itself, checked against the packer's own midpoint arithmetic: subdividing a
    /// grid-aligned power-of-two box `n − s` times yields exactly the cells of size `2^s`.
    fn assert_alignment(b: AlignedBox, s: u32) {
        let mut level = vec![b.ubox()];
        for _ in 0..b.cell_depth(s) {
            level = level.into_iter().flat_map(quad_children).collect();
        }
        assert_eq!(level.len(), 1usize << (2 * b.cell_depth(s)));
        for node in &level {
            let (min_lon, min_lat, max_lon, max_lat) = *node;
            assert_eq!(max_lat - min_lat, 1 << s);
            assert_eq!(max_lon - min_lon, 1 << s);
            assert_eq!(CellId::containing(s, min_lat, min_lon).square(), *node, "a depth-d node IS a cell");
        }
    }

    #[test]
    fn alignment_theorem_holds_at_every_band_size() {
        let b = assembly_box(&[CellId::parse("20/0301/0263").unwrap()], 20).unwrap();
        for s in [18, 19, 20] {
            assert_alignment(AlignedBox { span_log2: 21, ..b }, s);
        }
        // …and at the negative origin, where a truncating division would drift off the grid.
        let origin = AlignedBox { min_lat: GRID_ORIGIN, min_lon: GRID_ORIGIN, span_log2: 22 };
        assert_alignment(origin, 18);
        assert_alignment(origin, 20);
    }

    #[test]
    fn assembly_box_snaps_outward_and_stays_square() {
        // One 2^18 cell under a 2^20 S_MAX: the box snaps down to the 2^20 lattice and spans 2^20.
        let c = CellId::parse("18/1204/1052").unwrap();
        let b = assembly_box(&[c], 20).unwrap();
        assert_eq!(b.span_log2, 20);
        assert!(on_grid_line(b.min_lat, 20) && on_grid_line(b.min_lon, 20), "the corner is S_MAX-aligned");
        assert!(b.contains_cell(c));
        // A wide-but-short selection is padded to a square rather than shrunk to the content.
        let row: Vec<CellId> = (1052..1060).map(|j| CellId::new(18, 1204, j).unwrap()).collect();
        let b = assembly_box(&row, 20).unwrap();
        let (min_lon, min_lat, max_lon, max_lat) = b.ubox();
        assert_eq!(max_lat - min_lat, max_lon - min_lon, "square");
        assert!(row.iter().all(|c| b.contains_cell(*c)));
    }

    #[test]
    fn on_grid_line_tracks_band_nesting() {
        let coarse = GRID_ORIGIN + 3 * (1 << 20);
        assert!(on_grid_line(coarse, 20) && on_grid_line(coarse, 18), "a 2^20 line is also a 2^18 line");
        assert!(!on_grid_line(GRID_ORIGIN + (1 << 18), 20), "the converse is not true");
        assert!(on_grid_boundary(47_185_920, 12_345, 18));
    }

    #[test]
    fn id_round_trips_and_rejects_out_of_range() {
        assert_eq!(CellId::parse("18/7/9").unwrap().to_string(), "18/0007/0009", "lenient in, canonical out");
        assert_eq!(id_width(10), 6);
        assert!(CellId::parse("18/2048/0").is_err());
        assert!(CellId::parse("29/0/0").is_err());
        assert!(CellId::parse("18/0/0/0").is_err());
    }
}
