//! `grid.rs` — the OBCA cell grid: the fixed global µdeg lattice, cell identity, the
//! per-band cell-size table, and the integer arithmetic the cell cutter cuts with.
//!
//! Everything here is normative in [`OBCA_Spec.md`](../../../specs/OBCA_Spec.md) §1–§2 and
//! deliberately **integer-only**: the grid exists so that an OBCM quadtree's floor-midpoint
//! subdivision lands *exactly* on cell boundaries (§2, the alignment theorem), and a single
//! rounding step in the wrong direction would break that. No float ever reaches a coordinate
//! computed in this module.
//!
//! Two things are constants here and never change: the origin ([`GRID_ORIGIN`]) and the *shape* of
//! the grid (square, power-of-two, one origin for every band and size). The actual **cell sizes and
//! band membership are schema data** — [`BandTable`] carries them, [`BandTable::recommended`] is the measured
//! recommendation of OBCA §1.5, and a bakery is expected to hand in the table its catalog
//! publishes rather than trust a default. Retuning a band is a re-bake, not a format bump (§1.2).

use std::collections::HashSet;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Origin of the fixed global cell grid, µdeg, on **both** axes (OBCA §1.1).
///
/// A power of two, so every permitted cell size divides it exactly — which is the whole reason the
/// grid is not anchored at −90/−180 (`−90_000_000` is a multiple of no candidate cell size, so no
/// size would ever make a quadtree midpoint coincide with a cell edge).
pub const GRID_ORIGIN: i64 = -(1 << 28);

/// Side of the world box, µdeg (OBCA §1.1): `2^29`, i.e. ≈ ±268.435456°. Strictly larger than the
/// geographic domain, and the grid does **not** wrap — cells may legally overhang ±90 / ±180 and
/// producers MUST NOT clamp them (§1.4).
pub const WORLD_SIDE: i64 = 1 << 29;

/// Smallest permitted cell size as `log2(µdeg)` (OBCA §1.1).
pub const MIN_CELL_LOG2: u32 = 10;

/// Largest permitted cell size as `log2(µdeg)` (OBCA §1.1).
pub const MAX_CELL_LOG2: u32 = 28;

/// A bbox in the packer's own order: `(min_lon, min_lat, max_lon, max_lat)`, µdeg.
///
/// The order is the serializer's ([`crate::serialize`]) and not the spec tables' `lat, lon` — a cell
/// square is handed straight to the quadtree builder and the header writer, so matching them is what
/// keeps the cutter free of swap bugs.
pub type UBox = (i64, i64, i64, i64);

/// One cell of the grid: a size (as `log2(µdeg)`) and its **latitude** index `i` and **longitude**
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

/// Zero-padding width of a cell id's indices (OBCA §1.3): `max(4, digits(cells_per_axis − 1))`.
/// Four for every size at or above `2^16`, wider below — producers MUST widen rather than truncate.
///
/// The rule itself lives in [`obc_elevation::grid::id_width`]: `obc-dem` names published terrain
/// cells by the same id and cannot depend on this crate, so the arithmetic has one home in the
/// `no_std` leaf both reach.
pub fn id_width(log2: u32) -> usize {
    obc_elevation::grid::id_width(log2 as u8)
}

impl CellId {
    /// Reject a size outside OBCA §1.1's `2^10 .. 2^28`.
    fn check_log2(log2: u32) -> Result<(), String> {
        if !(MIN_CELL_LOG2..=MAX_CELL_LOG2).contains(&log2) {
            return Err(format!("cell size 2^{log2} µdeg is outside the grid's {MIN_CELL_LOG2}..={MAX_CELL_LOG2}"));
        }
        Ok(())
    }

    /// A cell by size + indices, validated against the world box.
    pub fn new(log2: u32, i: i64, j: i64) -> Result<Self, String> {
        Self::check_log2(log2)?;
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
    ///
    /// Half-open is the whole point: a coordinate exactly on a cell's `max` edge belongs to the
    /// **next** cell, so a point is owned by exactly one cell and no feature is written twice.
    #[inline]
    pub fn containing(log2: u32, lat: i64, lon: i64) -> Self {
        CellId { log2, i: (lat - GRID_ORIGIN).div_euclid(1 << log2), j: (lon - GRID_ORIGIN).div_euclid(1 << log2) }
    }

    /// Whether `(lat, lon)` lies in this cell's half-open square.
    #[inline]
    pub fn contains(self, lat: i64, lon: i64) -> bool {
        let (min_lon, min_lat, max_lon, max_lat) = self.square();
        lat >= min_lat && lat < max_lat && lon >= min_lon && lon < max_lon
    }

    /// Parse a canonical id `<log2>/<i>/<j>` (OBCA §1.3). Lenient about zero padding on the way in
    /// (any decimal width is accepted), strict on the way out ([`fmt::Display`] pads canonically).
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
    /// The canonical, zero-padded id (OBCA §1.3).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let w = id_width(self.log2);
        write!(f, "{}/{:0w$}/{:0w$}", self.log2, self.i, self.j, w = w)
    }
}

/// Every cell of size `2^log2` whose square intersects `bbox`, in ascending `(i, j)` order.
///
/// Intersection is decided on the **half-open** squares, so the `max` edges of `bbox` are inclusive
/// of the cell that owns them: a vertex sitting exactly on a grid line belongs to the cell above /
/// east of it, and that cell is therefore part of the covering. A bbox reaching past the world box
/// is clamped to it rather than wrapped (OBCA §1.4) — the grid has no wrap, so there is nothing on
/// the other side to reach.
pub fn cells_intersecting(log2: u32, bbox: UBox) -> Vec<CellId> {
    let (min_lon, min_lat, max_lon, max_lat) = bbox;
    if min_lon > max_lon || min_lat > max_lat {
        return Vec::new();
    }
    let s = 1i64 << log2;
    let n = axis_cells(log2);
    let lo = |v: i64| (v - GRID_ORIGIN).div_euclid(s).clamp(0, n - 1);
    let (i0, i1) = (lo(min_lat), lo(max_lat));
    let (j0, j1) = (lo(min_lon), lo(max_lon));
    let mut out = Vec::with_capacity(((i1 - i0 + 1) * (j1 - j0 + 1)) as usize);
    for i in i0..=i1 {
        for j in j0..=j1 {
            out.push(CellId { log2, i, j });
        }
    }
    out
}

/// Whether `v` lies exactly on a grid line of size `2^log2` — i.e. on a cell boundary.
///
/// This is the seam predicate: OBCA §3.4 makes every vertex on a boundary line a junction, and §4.6
/// admits **only** such coordinates to unification. It is a pure function of the coordinate, which
/// is why two neighbours cannot disagree about it.
#[inline]
pub fn on_grid_line(v: i64, log2: u32) -> bool {
    (v - GRID_ORIGIN) & ((1 << log2) - 1) == 0
}

/// Whether `(lat, lon)` lies on any boundary line of the `2^log2` grid.
#[inline]
pub fn on_grid_boundary(lat: i64, lon: i64, log2: u32) -> bool {
    on_grid_line(lat, log2) || on_grid_line(lon, log2)
}

/// The floor-division midpoint the OBCM quadtree splits at (`OBCM_Spec.md` §4), spelled out once so
/// the alignment theorem (OBCA §2) can be *tested* against the same arithmetic the packer and the
/// reader use.
#[inline]
pub fn quad_mid(min: i64, max: i64) -> i64 {
    (min + max).div_euclid(2)
}

// --- exact rational rounding, for the boundary-junction formula (OBCA §3.4) -------------------

/// `num / den` rounded **half to even** in exact integer arithmetic (banker's rounding), for any
/// sign of either operand. `den` must be non-zero.
///
/// OBCA §3.4 mandates this exact mode for the boundary-junction interpolation: both neighbours run
/// the same formula over the same two source vertices, so anything short of an exactly specified
/// rounding rule would let them land a µdeg apart — and a µdeg apart is a *different* junction that
/// no assembler may unify (§3.4's epsilon rule).
pub fn div_round_half_even(num: i128, den: i128) -> i64 {
    debug_assert!(den != 0, "a crossing on a degenerate segment is never computed");
    let (num, den) = if den < 0 { (-num, -den) } else { (num, den) };
    let q = num.div_euclid(den);
    // `q` is the floor, so the remainder is the fractional part: below half rounds down, above half
    // rounds up, and an exact half goes to whichever of `q`, `q + 1` is even.
    let q = match (2 * num.rem_euclid(den)).cmp(&den) {
        std::cmp::Ordering::Less => q,
        std::cmp::Ordering::Greater => q + 1,
        std::cmp::Ordering::Equal => q + i128::from(q % 2 != 0),
    };
    q as i64
}

/// Which axis a cell-edge line runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// A line of constant latitude (a horizontal edge).
    Lat,
    /// A line of constant longitude (a vertical edge).
    Lon,
}

/// The boundary junction where segment `p`–`q` crosses the cell-edge line `axis = c`, as
/// `(lat, lon)` µdeg — the verbatim OBCA §3.4 computation.
///
/// Two properties make this the seam's foundation, and both are deliberate:
///
/// - **Direction-independent.** The endpoints are ordered canonically by `(lat, lon)` before the
///   interpolation, so a way and its reversed copy produce the identical integer pair.
/// - **Exact.** The interpolation runs in `i128` with [`div_round_half_even`]; no float is involved,
///   so the result is reproducible on any toolchain, not merely on this one.
///
/// Returns `None` when the segment does not properly cross the line (it is parallel to it, or the
/// line passes through an endpoint — §3.4(1): such a vertex *is* the boundary junction, so there is
/// nothing to interpolate).
pub fn segment_crossing(p: (i64, i64), q: (i64, i64), axis: Axis, c: i64) -> Option<(i64, i64)> {
    // Canonical endpoint order (lat, lon) lexicographic — §3.4(2).
    let (p, q) = if p <= q { (p, q) } else { (q, p) };
    let (p_lat, p_lon) = p;
    let (q_lat, q_lon) = q;
    match axis {
        Axis::Lon => {
            let (lo, hi) = (p_lon.min(q_lon), p_lon.max(q_lon));
            if c <= lo || c >= hi {
                return None; // parallel, or the line hits an endpoint
            }
            let lat =
                p_lat + div_round_half_even((q_lat - p_lat) as i128 * (c - p_lon) as i128, (q_lon - p_lon) as i128);
            Some((lat, c))
        }
        Axis::Lat => {
            let (lo, hi) = (p_lat.min(q_lat), p_lat.max(q_lat));
            if c <= lo || c >= hi {
                return None;
            }
            let lon =
                p_lon + div_round_half_even((q_lon - p_lon) as i128 * (c - p_lat) as i128, (q_lat - p_lat) as i128);
            Some((c, lon))
        }
    }
}

// --- the band table (schema data, OBCA §1.2/§1.5) ---------------------------------------------

/// Which physical file of a volume set a band's content is assembled into (OBCA §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BandRole {
    /// The one file that cannot be split by bbox: nav graph, POIs, style table.
    Core,
    /// The single whole-assembly shard carrying the coarsest LODs.
    Coarse,
    /// An ordinary splittable geometry shard.
    Geometry,
}

impl BandRole {
    fn as_str(self) -> &'static str {
        match self {
            BandRole::Core => "core",
            BandRole::Coarse => "coarse",
            BandRole::Geometry => "geometry",
        }
    }
}

/// One band: a named class of cell content with one cell size (OBCA §1.2).
///
/// The JSON shape is `OBCC_Spec.md` §4's `bands` entry verbatim, so a bakery can hand the
/// catalog's own schema straight to the cutter.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Band {
    /// Stable band id, e.g. `fine`. Also the cutter's output directory name.
    pub id: String,
    /// Cell size, `log2(µdeg)`.
    pub cell_log2: u32,
    /// Ladder LOD indices this band's cells carry. Every other LOD is written **empty** (§3.1).
    #[serde(default)]
    pub lods: Vec<usize>,
    /// Non-geometry sections this band carries: `"nav"` and/or `"poi"`.
    #[serde(default)]
    pub sections: Vec<String>,
    pub role: BandRole,
}

impl Band {
    /// Whether this band's cells carry the §8 nav graph.
    pub fn has_nav(&self) -> bool {
        self.sections.iter().any(|s| s == "nav")
    }

    /// Whether this band's cells carry the §7 POI section + hours pool.
    pub fn has_poi(&self) -> bool {
        self.sections.iter().any(|s| s == "poi")
    }
}

/// The schema's band table: which LODs and sections live in which cell size (OBCA §1.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BandTable {
    pub bands: Vec<Band>,
}

impl BandTable {
    /// The **recommended band table** — OBCA §1.5, measured for schema `bikepacking`
    /// revision 1 over the shipped 9-LOD ladder:
    ///
    /// | band | size | carries |
    /// | :-- | :-- | :-- |
    /// | `coarse` | `2^20` | LOD 0, 1, 2, 3, 4 |
    /// | `mid` | `2^19` | LOD 5, 6 |
    /// | `fine` | `2^18` | LOD 7, 8 |
    /// | `network` | `2^18` | nav graph + POIs, **no** LOD |
    ///
    /// The two far-zoom tiers the ladder gained (LOD 0 and 1) join `coarse`, which is where
    /// the boundaries were measured: `coarse` still ends at the same *content* it always
    /// carried (the tiers now numbered 2, 3, 4), and the two additions are the most
    /// aggressively culled levels on the map, so the shard they land in is the one whose
    /// budget can absorb them.
    ///
    /// These are *values*, not format constants: a catalog states them, a producer reads them from
    /// the catalog, and this default exists so the CLI and the tests have something to run with.
    pub fn recommended() -> Self {
        let band = |id: &str, cell_log2: u32, lods: &[usize], sections: &[&str], role: BandRole| Band {
            id: id.to_string(),
            cell_log2,
            lods: lods.to_vec(),
            sections: sections.iter().map(|s| (*s).to_string()).collect(),
            role,
        };
        BandTable {
            bands: vec![
                band("coarse", 20, &[0, 1, 2, 3, 4], &[], BandRole::Coarse),
                band("mid", 19, &[5, 6], &[], BandRole::Geometry),
                band("fine", 18, &[7, 8], &[], BandRole::Geometry),
                band("network", 18, &[], &["nav", "poi"], BandRole::Core),
            ],
        }
    }

    /// Read a band table from JSON: either `{"bands": [...]}` or a bare `[...]` array of bands.
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim_start();
        let table = if trimmed.starts_with('[') {
            let bands: Vec<Band> = serde_json::from_str(text).map_err(|e| format!("band table: {e}"))?;
            BandTable { bands }
        } else {
            serde_json::from_str(text).map_err(|e| format!("band table: {e}"))?
        };
        Ok(table)
    }

    /// Read a band table from a file.
    pub fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?;
        Self::parse(&text)
    }

    /// The band with this id.
    pub fn band(&self, id: &str) -> Option<&Band> {
        self.bands.iter().find(|b| b.id == id)
    }

    /// `S_MAX` as `log2` — the largest cell size in the table, which is the assembly bbox's
    /// alignment modulus (OBCA §2.1).
    pub fn max_cell_log2(&self) -> u32 {
        self.bands.iter().map(|b| b.cell_log2).max().unwrap_or(MIN_CELL_LOG2)
    }

    /// Check the table against a ladder of `lod_count` levels: OBCA §1.2's **partition** rule and
    /// §5.1's role rules, both of which a consumer MUST reject a violation of.
    ///
    /// A LOD in no band is a map that is blank at that zoom; a LOD in two bands is a map that
    /// carries it twice; a `core` band carrying geometry would spend the one file that cannot be
    /// split by bbox on bytes that could have been.
    pub fn validate(&self, lod_count: usize) -> Result<(), String> {
        if self.bands.is_empty() {
            return Err("band table is empty".into());
        }
        let mut ids = HashSet::new();
        for b in &self.bands {
            if b.id.is_empty() {
                return Err("a band has an empty id".into());
            }
            if !ids.insert(b.id.as_str()) {
                return Err(format!("band id {:?} appears twice", b.id));
            }
            CellId::check_log2(b.cell_log2).map_err(|e| format!("band {:?}: {e}", b.id))?;
            for s in &b.sections {
                if s != "nav" && s != "poi" {
                    return Err(format!("band {:?}: unknown section {s:?} (expected \"nav\" or \"poi\")", b.id));
                }
            }
        }
        // Partition: every ladder LOD in exactly one band.
        let mut owner: Vec<Option<&str>> = vec![None; lod_count];
        for b in &self.bands {
            for &l in &b.lods {
                let slot = owner
                    .get_mut(l)
                    .ok_or_else(|| format!("band {:?} claims LOD {l}, past the ladder's {lod_count} level(s)", b.id))?;
                if let Some(other) = slot {
                    return Err(format!("LOD {l} is in two bands ({other} and {})", b.id));
                }
                *slot = Some(&b.id);
            }
        }
        if let Some(l) = owner.iter().position(Option::is_none) {
            return Err(format!("LOD {l} is in no band — its cells would be blank at that zoom"));
        }
        // Sections: nav in exactly one band, POI in exactly one band.
        for (name, count) in [
            ("nav", self.bands.iter().filter(|b| b.has_nav()).count()),
            ("poi", self.bands.iter().filter(|b| b.has_poi()).count()),
        ] {
            if count != 1 {
                return Err(format!("the {name} section must be in exactly one band, found {count}"));
            }
        }
        // Roles (OBCA §5.1).
        let cores: Vec<&Band> = self.bands.iter().filter(|b| b.role == BandRole::Core).collect();
        if cores.len() != 1 {
            return Err(format!("exactly one band must have role \"core\", found {}", cores.len()));
        }
        let core = cores[0];
        if !core.lods.is_empty() {
            return Err(format!(
                "the core band {:?} carries LOD(s) {:?}: geometry belongs in a splittable shard, never in the one \
                 file a volume set cannot split (OBCA §5.1)",
                core.id, core.lods
            ));
        }
        if !(core.has_nav() && core.has_poi()) {
            return Err(format!("the core band {:?} must carry both the nav and POI sections", core.id));
        }
        if self.bands.iter().filter(|b| b.role == BandRole::Coarse).count() > 1 {
            return Err("at most one band may have role \"coarse\"".into());
        }
        for b in &self.bands {
            if b.role != BandRole::Core {
                if b.lods.is_empty() {
                    return Err(format!("band {:?} (role {}) carries no LOD", b.id, b.role.as_str()));
                }
                if !b.sections.is_empty() {
                    return Err(format!(
                        "band {:?} (role {}) carries section(s) {:?}: only the core band may",
                        b.id,
                        b.role.as_str(),
                        b.sections
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// OBCA §1.1's constants, and the divisibility that makes every band nest.
    #[test]
    fn grid_constants_and_nesting() {
        assert_eq!(GRID_ORIGIN, -268_435_456);
        assert_eq!(WORLD_SIDE, 536_870_912);
        for log2 in MIN_CELL_LOG2..=MAX_CELL_LOG2 {
            let s = 1i64 << log2;
            assert_eq!(GRID_ORIGIN % s, 0, "every permitted size divides the origin (2^{log2})");
            assert_eq!(WORLD_SIDE % s, 0, "…and the world side (2^{log2})");
            assert_eq!(axis_cells(log2) * s, WORLD_SIDE, "cells of one size tile the world exactly");
        }
        // The world box contains the geographic domain with room to spare.
        const { assert!(GRID_ORIGIN < -180_000_000 && GRID_ORIGIN + WORLD_SIDE > 180_000_000) };
    }

    /// The spec's worked example (OBCA §7): cell A = `18/1204/1052` is
    /// lat [47185920, 47448064) × lon [7340032, 7602176).
    #[test]
    fn worked_example_squares() {
        let a = CellId::parse("18/1204/1052").expect("valid id");
        assert_eq!(a.square(), (7_340_032, 47_185_920, 7_602_176, 47_448_064));
        let b = CellId::parse("18/1204/1053").expect("valid id");
        assert_eq!(b.square(), (7_602_176, 47_185_920, 7_864_320, 47_448_064));
        // Neighbours share the seam exactly: A's max_lon is B's min_lon.
        assert_eq!(a.square().2, b.square().0);
        assert_eq!(a.to_string(), "18/1204/1052", "canonical id round-trips");
    }

    /// Half-open ownership: a coordinate on a `max` edge belongs to the next cell, so every point
    /// is in exactly one cell of a size.
    #[test]
    fn half_open_ownership_is_exclusive() {
        let a = CellId::parse("18/1204/1052").unwrap();
        let (min_lon, min_lat, max_lon, max_lat) = a.square();
        assert!(a.contains(min_lat, min_lon), "the min corner is inside");
        assert!(!a.contains(max_lat, min_lon), "the max lat edge is not");
        assert!(!a.contains(min_lat, max_lon), "the max lon edge is not");
        assert_eq!(CellId::containing(18, min_lat, min_lon), a);
        assert_eq!(CellId::containing(18, max_lat, max_lon), CellId::new(18, a.i + 1, a.j + 1).unwrap());
        assert_eq!(CellId::containing(18, max_lat - 1, max_lon - 1), a);
    }

    /// Id widths follow §1.3: four digits at `2^16` and above, wider below, never truncated.
    #[test]
    fn id_padding_widths() {
        assert_eq!(id_width(20), 4, "2^20 ⇒ 512 cells/axis ⇒ 3 digits, floored to 4");
        assert_eq!(id_width(18), 4, "2^18 ⇒ 2048 cells/axis");
        assert_eq!(id_width(16), 4, "2^16 ⇒ 8192 cells/axis");
        assert_eq!(id_width(10), 6, "2^10 ⇒ 524288 cells/axis ⇒ 6 digits");
        assert_eq!(CellId::new(10, 7, 9).unwrap().to_string(), "10/000007/000009");
        assert_eq!(CellId::parse("10/000007/000009").unwrap(), CellId::new(10, 7, 9).unwrap());
        // Lenient in, canonical out.
        assert_eq!(CellId::parse("18/7/9").unwrap().to_string(), "18/0007/0009");
    }

    /// Ids outside the world box, or of an unsupported size, are rejected rather than wrapped.
    #[test]
    fn id_parse_rejects_out_of_range() {
        assert!(CellId::parse("18/2048/0").is_err(), "2^18 has 2048 cells/axis, so 2048 is past the end");
        assert!(CellId::parse("9/0/0").is_err(), "2^9 is below the grid's minimum size");
        assert!(CellId::parse("29/0/0").is_err(), "2^29 is above the maximum");
        assert!(CellId::parse("18/-1/0").is_err());
        assert!(CellId::parse("18/0").is_err());
        assert!(CellId::parse("18/0/0/0").is_err());
    }

    /// **The alignment theorem** (OBCA §2), checked against the packer's own arithmetic: subdividing
    /// a grid-aligned power-of-two box by [`quad_mid`] `n − s` times yields *exactly* the cells of
    /// size `2^s`, to the microdegree.
    fn assert_alignment(a_lat: i64, a_lon: i64, n: u32, s: u32) {
        // Depth-by-depth subdivision, exactly as `quadtree::build_node` splits.
        let mut level: Vec<UBox> = vec![(a_lon, a_lat, a_lon + (1 << n), a_lat + (1 << n))];
        for _ in 0..(n - s) {
            let mut next = Vec::with_capacity(level.len() * 4);
            for (min_lon, min_lat, max_lon, max_lat) in level {
                let mid_lon = quad_mid(min_lon, max_lon);
                let mid_lat = quad_mid(min_lat, max_lat);
                // NW, NE, SW, SE — the packer's child order.
                next.push((min_lon, mid_lat, mid_lon, max_lat));
                next.push((mid_lon, mid_lat, max_lon, max_lat));
                next.push((min_lon, min_lat, mid_lon, mid_lat));
                next.push((mid_lon, min_lat, max_lon, mid_lat));
            }
            level = next;
        }
        assert_eq!(level.len(), 1usize << (2 * (n - s)), "the depth is fully populated");
        for b in &level {
            let (min_lon, min_lat, max_lon, max_lat) = *b;
            assert_eq!(max_lat - min_lat, 1 << s, "span is exactly the cell size on lat");
            assert_eq!(max_lon - min_lon, 1 << s, "…and on lon");
            assert!(on_grid_line(min_lat, s) && on_grid_line(min_lon, s), "the min corner is a grid corner");
            let cell = CellId::containing(s, min_lat, min_lon);
            assert_eq!(cell.square(), *b, "a depth-{} node IS a cell of size 2^{s}", n - s);
        }
    }

    #[test]
    fn alignment_theorem_holds_at_every_band_size() {
        // A grid-aligned assembly bbox around the spec's example, S_MAX = 2^20.
        let s_max = 20u32;
        let a_lat = GRID_ORIGIN + (47_185_920 - GRID_ORIGIN).div_euclid(1 << s_max) * (1 << s_max);
        let a_lon = GRID_ORIGIN + (7_340_032 - GRID_ORIGIN).div_euclid(1 << s_max) * (1 << s_max);
        for s in [18, 19, 20] {
            assert_alignment(a_lat, a_lon, 21, s);
        }
    }

    /// The case a signed floor-division bug hides in: cells at the **negative** origin, where
    /// `(min + max) / 2` truncating toward zero would land off the grid.
    #[test]
    fn alignment_theorem_holds_at_the_negative_origin() {
        assert_alignment(GRID_ORIGIN, GRID_ORIGIN, 22, 18);
        assert_alignment(GRID_ORIGIN, GRID_ORIGIN, 22, 20);
        // The very first cell, and its neighbours across the origin's own lines.
        let c = CellId::new(20, 0, 0).unwrap();
        assert_eq!(c.square(), (GRID_ORIGIN, GRID_ORIGIN, GRID_ORIGIN + (1 << 20), GRID_ORIGIN + (1 << 20)));
        assert_eq!(CellId::containing(20, GRID_ORIGIN, GRID_ORIGIN), c);
        // A midpoint of a negative-spanning box must floor, not truncate.
        assert_eq!(quad_mid(-3, 0), -2, "div_euclid floors toward −∞");
        assert_eq!(quad_mid(GRID_ORIGIN, GRID_ORIGIN + (1 << 21)), GRID_ORIGIN + (1 << 20));
    }

    /// A negative-latitude band (the southern hemisphere is where a truncating grid would drift).
    #[test]
    fn cells_south_of_the_equator() {
        let c = CellId::containing(18, -33_900_000, 18_400_000); // Cape Town
        let (min_lon, min_lat, max_lon, max_lat) = c.square();
        assert!(min_lat <= -33_900_000 && -33_900_000 < max_lat);
        assert!(min_lon <= 18_400_000 && 18_400_000 < max_lon);
        assert_eq!(max_lat - min_lat, 1 << 18);
        assert!(on_grid_line(min_lat, 18) && on_grid_line(min_lon, 18));
    }

    #[test]
    fn cells_intersecting_covers_the_box_and_its_edges() {
        let a = CellId::parse("18/1204/1052").unwrap();
        let (min_lon, min_lat, max_lon, max_lat) = a.square();
        // Strictly inside ⇒ one cell.
        let one = cells_intersecting(18, (min_lon + 1, min_lat + 1, max_lon - 1, max_lat - 1));
        assert_eq!(one, vec![a]);
        // Reaching the shared edge ⇒ the neighbour comes along, because a vertex exactly on the
        // line belongs to it.
        let two = cells_intersecting(18, (min_lon + 1, min_lat + 1, max_lon, max_lat - 1));
        assert_eq!(two, vec![a, CellId::new(18, a.i, a.j + 1).unwrap()]);
        // A 2x2 block, in ascending (i, j).
        let four = cells_intersecting(18, (min_lon + 1, min_lat + 1, max_lon, max_lat));
        assert_eq!(four.len(), 4);
        assert!(four.windows(2).all(|w| (w[0].i, w[0].j) < (w[1].i, w[1].j)), "sorted by (i, j)");
    }

    #[test]
    fn on_grid_line_only_at_the_lines() {
        let s = 1i64 << 18;
        assert!(on_grid_line(GRID_ORIGIN, 18));
        assert!(on_grid_line(GRID_ORIGIN + 5 * s, 18));
        assert!(!on_grid_line(GRID_ORIGIN + 5 * s + 1, 18));
        assert!(on_grid_line(47_185_920, 18), "the worked example's cell min");
        assert!(!on_grid_line(47_185_921, 18));
        // A `2^20` line is also a `2^18` line (the bands nest); the converse is not true.
        let coarse = GRID_ORIGIN + 3 * (1 << 20);
        assert!(on_grid_line(coarse, 20) && on_grid_line(coarse, 18));
        assert!(on_grid_line(GRID_ORIGIN + (1 << 18), 18) && !on_grid_line(GRID_ORIGIN + (1 << 18), 20));
    }

    /// Banker's rounding, both signs, and the tie cases that a `round()` would get wrong.
    #[test]
    fn half_even_rounding_rules() {
        assert_eq!(div_round_half_even(1, 2), 0, "0.5 → 0 (even)");
        assert_eq!(div_round_half_even(3, 2), 2, "1.5 → 2 (even)");
        assert_eq!(div_round_half_even(5, 2), 2, "2.5 → 2 (even)");
        assert_eq!(div_round_half_even(7, 2), 4, "3.5 → 4 (even)");
        assert_eq!(div_round_half_even(-1, 2), 0, "−0.5 → 0");
        assert_eq!(div_round_half_even(-3, 2), -2, "−1.5 → −2");
        assert_eq!(div_round_half_even(-5, 2), -2, "−2.5 → −2");
        assert_eq!(div_round_half_even(2, 3), 1);
        assert_eq!(div_round_half_even(1, 3), 0);
        assert_eq!(div_round_half_even(-2, 3), -1);
        // Sign of the denominator must not change the answer.
        assert_eq!(div_round_half_even(5, -2), div_round_half_even(-5, 2));
    }

    /// The boundary-junction formula: on the line, exact, and **direction-independent** — the
    /// property that makes two neighbours agree without talking to each other.
    #[test]
    fn segment_crossing_is_exact_and_direction_free() {
        let c = 7_602_176; // the worked example's seam
        let p = (47_200_000, 7_600_000);
        let q = (47_210_000, 7_610_000);
        let fwd = segment_crossing(p, q, Axis::Lon, c).expect("crosses the seam");
        let rev = segment_crossing(q, p, Axis::Lon, c).expect("crosses the seam");
        assert_eq!(fwd, rev, "a reversed way yields the same junction");
        assert_eq!(fwd.1, c, "the junction sits exactly on the line");
        // lat = 47_200_000 + round(10_000 * 2176 / 10_000) = 47_202_176.
        assert_eq!(fwd, (47_202_176, c));

        // A latitude line, and the ties that exercise banker's rounding. Note the rounding applies
        // to the interpolated **delta** from the canonical first endpoint, exactly as §3.4 writes it.
        assert_eq!(segment_crossing((100, 0), (102, 1), Axis::Lat, 101), Some((101, 0)), "delta 0.5 → 0 (even)");
        assert_eq!(segment_crossing((100, 0), (102, 3), Axis::Lat, 101), Some((101, 2)), "delta 1.5 → 2 (even)");
        assert_eq!(segment_crossing((100, 0), (102, 5), Axis::Lat, 101), Some((101, 2)), "delta 2.5 → 2 (even)");
        assert_eq!(segment_crossing((100, 0), (102, 4), Axis::Lat, 101), Some((101, 2)), "delta 2 → 2, no tie");

        // No crossing: parallel, past the segment, or through an endpoint (§3.4(1)).
        assert_eq!(segment_crossing((0, c), (10, c), Axis::Lon, c), None, "collinear with the line");
        assert_eq!(segment_crossing(p, q, Axis::Lon, 9_000_000), None, "the line misses the segment");
        assert_eq!(segment_crossing((0, c), (10, c + 5), Axis::Lon, c), None, "the line hits an endpoint");
    }

    // --- the band table -----------------------------------------------------------------------

    #[test]
    fn recommended_band_table_is_the_spec_table() {
        let t = BandTable::recommended();
        t.validate(9).expect("the recommended table partitions the 9-LOD ladder");
        assert_eq!(t.max_cell_log2(), 20, "S_MAX = 2^20");
        let by = |id: &str| t.band(id).expect("band").clone();
        assert_eq!((by("coarse").cell_log2, by("coarse").lods), (20, vec![0, 1, 2, 3, 4]));
        assert_eq!((by("mid").cell_log2, by("mid").lods), (19, vec![5, 6]));
        assert_eq!((by("fine").cell_log2, by("fine").lods), (18, vec![7, 8]));
        let net = by("network");
        assert_eq!((net.cell_log2, net.role), (18, BandRole::Core));
        assert!(net.lods.is_empty(), "the core band carries no geometry (OBCA §5.1)");
        assert!(net.has_nav() && net.has_poi());
        // The ladder must match the table's expectation, not the other way round.
        assert!(t.validate(8).is_err(), "an 8-LOD ladder leaves LOD 8 claimed by no level");
        assert!(t.validate(10).is_err(), "a 10-LOD ladder leaves LOD 9 in no band");
    }

    #[test]
    fn band_table_validation_catches_the_partition_and_role_traps() {
        let err = |t: &BandTable| t.validate(9).expect_err("must be rejected");
        let mut t = BandTable::recommended();
        // A LOD in two bands.
        t.bands[1].lods.push(4);
        assert!(err(&t).contains("in two bands"), "got {}", err(&t));
        // A LOD in no band.
        let mut t = BandTable::recommended();
        t.bands[2].lods = vec![7];
        assert!(err(&t).contains("LOD 8 is in no band"));
        // Geometry in the core file.
        let mut t = BandTable::recommended();
        t.bands[3].lods = vec![8];
        t.bands[2].lods = vec![7];
        assert!(err(&t).contains("core band"));
        // Two nav bands.
        let mut t = BandTable::recommended();
        t.bands[2].sections = vec!["nav".into()];
        assert!(err(&t).contains("nav section must be in exactly one band"));
        // A section on a geometry band.
        let mut t = BandTable::recommended();
        t.bands[1].sections = vec!["poi".into()];
        assert!(!err(&t).is_empty());
        // Duplicate ids.
        let mut t = BandTable::recommended();
        t.bands[1].id = "fine".into();
        assert!(err(&t).contains("appears twice"));
        // An unsupported cell size.
        let mut t = BandTable::recommended();
        t.bands[0].cell_log2 = 30;
        assert!(err(&t).contains("outside the grid"));
    }

    #[test]
    fn band_table_json_round_trip() {
        let t = BandTable::recommended();
        let json = serde_json::to_string(&t).expect("serialize");
        assert_eq!(BandTable::parse(&json).expect("parse"), t);
        // A bare array is accepted too — that is the shape `OBCC_Spec.md` §4 nests.
        let bare = serde_json::to_string(&t.bands).expect("serialize bands");
        assert_eq!(BandTable::parse(&bare).expect("parse bare"), t);
        // Roles are lowercase strings on the wire.
        assert!(json.contains("\"role\":\"core\""));
        assert!(BandTable::parse("{\"bands\":[{\"id\":\"x\",\"cell_log2\":18,\"role\":\"nope\"}]}").is_err());
    }
}
