//! Wire documents shared by catalog generation, schema generation, and callers.
//!
//! This module owns only the serialized catalog model and its canonical cell-id
//! boundary. Tree scanning, validation, hashing, and publication stay in the catalog
//! façade until their own ownership-complete seams are extracted.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::grid::{axis_cells, id_width, CellId, GRID_ORIGIN, MAX_CELL_LOG2, MIN_CELL_LOG2, WORLD_SIDE};

use super::boundary;

// --- the grid (OBCA_Spec.md §1) -----------------------------------------------------------
//
// The grid itself lives in [`crate::grid`] — one `CellId`, one origin, one padding rule for
// the cutter, the catalog generator and every consumer of both. What is local here is the
// *catalog's* two obligations on top of it: the JSON boundary is `i32`, and an id that
// reaches a content-addressed store must be canonical.

/// [`GRID_ORIGIN`] at the JSON boundary. [`GridEntry`] publishes the origin as an `i32`
/// because that is the width an OBCM header stores a coordinate in, so the narrowing is
/// spelled out once, here, rather than at each use.
///
/// The value is `−2^28` rather than `−90 000 000` because it is divisible by every
/// permitted cell size, which is what makes quadtree midpoints and cell boundaries
/// coincide (`OBCA_Spec.md` §1.1).
pub const GRID_ORIGIN_UDEG: i32 = GRID_ORIGIN as i32;

/// [`WORLD_SIDE`] at the JSON boundary: `2^29` µdeg. The world box (≈ ±268°) is
/// deliberately wider than the geographic domain, so a cell may overhang ±90°/±180° and
/// MUST NOT be clamped.
pub const WORLD_SIDE_UDEG: i32 = WORLD_SIDE as i32;

/// Parse the canonical `<log2>/<i>/<j>` id (`OBCA_Spec.md` §1.3), **strictly**.
///
/// [`CellId::parse`] is deliberately lenient about the zero padding — a human types ids at
/// a CLI. A catalog cannot be: producers MUST widen rather than truncate, so `18/1204/52`
/// and `18/01204/1052` are *different strings for the same cell* and exactly the kind of
/// ambiguity a content-addressed store must not have. Every id this module reads out of a
/// document or off a path comes through here.
pub fn parse_strict_id(s: &str) -> Result<CellId, String> {
    let mut parts = s.split('/');
    let (Some(log2), Some(i), Some(j), None) = (parts.next(), parts.next(), parts.next(), parts.next()) else {
        return Err(format!("cell id `{s}` is not `<log2>/<i>/<j>`"));
    };
    if log2.is_empty() || log2.len() > 2 || !log2.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("cell id `{s}`: `{log2}` is not a 1–2 digit cell size"));
    }
    let log2: u32 = log2.parse().map_err(|_| format!("cell id `{s}`: bad cell size"))?;
    if !(MIN_CELL_LOG2..=MAX_CELL_LOG2).contains(&log2) {
        return Err(format!("cell id `{s}`: cell size 2^{log2} µdeg is outside 2^{MIN_CELL_LOG2}..=2^{MAX_CELL_LOG2}"));
    }
    let width = id_width(log2);
    let count = axis_cells(log2);
    let mut idx = [0i64; 2];
    for (slot, (text, axis)) in idx.iter_mut().zip([(i, "i"), (j, "j")]) {
        if text.len() != width || !text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!("cell id `{s}`: `{axis}` must be {width} digits, zero-padded (got `{text}`)"));
        }
        let v: i64 = text.parse().map_err(|_| format!("cell id `{s}`: `{text}` is not a number"))?;
        if v >= count {
            return Err(format!("cell id `{s}`: `{axis}` = {v} is off the grid (0..{count})"));
        }
        *slot = v;
    }
    Ok(CellId { log2, i: idx[0], j: idx[1] })
}

// --- the root document (§3) ------------------------------------------------------------

/// The catalog root: small, short-cached, and the only document a consumer reads before it
/// knows what the catalog offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Catalog {
    /// Envelope version, `2`. Checked before any other field.
    pub schema_version: u32,
    /// When this root was generated, RFC 3339 UTC — the only wall clock on the
    /// generation path (`OBCC_Spec.md` §3).
    pub generated_at: String,
    /// The cell store's data provenance and licence (§3.1). The store is a derivative
    /// database of OpenStreetMap, and the ODbL's share-alike terms require the
    /// published store to say so — this block is that statement, in the one document
    /// every consumer reads first. The generator always writes it; `Option` only so
    /// documents published before the field existed still deserialize (the bake
    /// guard reads the live root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceEntry>,
    /// The catalog's **single** schema. Not an array: the hosted store carries the
    /// 14-LOD bikepacking ladder and nothing else, because a second schema would make
    /// the whole planet-shaped cell store exist twice (§3, epic #1016 D2).
    pub schema: SchemaEntry,
    /// Every skin offered, sorted by `id`. Inlined rather than referenced: one is
    /// ≈ 2 KB and a builder needs all of them at once to draw a picker.
    pub skins: Vec<SkinEntry>,
    /// Named selections, sorted by `id`.
    pub regions: Vec<RegionEntry>,
    /// One entry per band, sorted by `cell_log2` descending (§8).
    pub cell_index: Vec<CellIndexRef>,
    /// The terrain artifact class, when the catalog publishes one (§13). Absent is a
    /// complete, valid catalog: every consumer degrades to "no elevation here", which
    /// is exactly what a map with no terrain has always done.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain: Option<TerrainEntry>,
    /// **The one coupling between the two revision tracks** (§13.4). Network-band cells
    /// are baked sampling OBCT, so their `Ascent M` values are a function of a
    /// particular terrain revision; this records which one. `None` for a terrain-less
    /// bake, whose ascents are all zero and depend on nothing.
    ///
    /// It is at the root rather than in [`SchemaEntry`] on purpose: the schema is the
    /// identity of the OBCM store and must not acquire a terrain field, or a terrain
    /// re-bake would look like a schema change to every consumer that compares schemas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_terrain_revision: Option<u32>,
}

/// §3.1's source declaration: what the cells derive from and what that obliges.
///
/// The values are [`OSM_SOURCE`]'s constants rather than tree inputs, because the packer
/// ingests exactly one dataset — the day a second source exists is the day this becomes
/// data, not before (the OBCM v13 lesson). Consumers that describe the map data take
/// these strings from the catalog rather than hard-coding them (§3.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SourceEntry {
    /// Kebab-case id of the source dataset.
    pub dataset_id: String,
    /// The dataset's required credit, verbatim.
    pub attribution: String,
    /// SPDX-style identifier of the licence the published store is offered under.
    pub license: String,
    /// Where that licence's text lives.
    pub license_url: String,
}

/// The one source the packer ingests, as §3.1 publishes it.
pub fn osm_source() -> SourceEntry {
    SourceEntry {
        dataset_id: "openstreetmap".into(),
        attribution: "\u{00a9} OpenStreetMap contributors".into(),
        license: "ODbL-1.0".into(),
        license_url: "https://opendatacommons.org/licenses/odbl/1-0/".into(),
    }
}

/// The schema: the identity of the cell store. Everything a consumer needs to price a
/// selection and an assembler needs to stamp a skin, with no out-of-band constant
/// (§4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct SchemaEntry {
    /// Stable id, e.g. `bikepacking`.
    pub id: String,
    /// Monotone content revision. Every cell states the revision it was baked at, and
    /// a bump invalidates the whole store — assembly copies chunk bytes between files,
    /// which is only meaningful within one revision (`OBCA_Spec.md` §6.3).
    pub revision: u32,
    pub name: String,
    pub description: String,
    /// OBCM format version, read from the **cells' own headers**; every cell agrees or
    /// generation fails (§10).
    pub obcm_version: u8,
    /// The grid's constants, restated so no consumer hard-codes them.
    pub grid: GridEntry,
    /// The LOD ladder, coarsest first.
    pub lods: Vec<LodEntry>,
    /// The band table: which LODs and which non-geometry sections live in which cell
    /// size, and which file of a volume set they assemble into.
    pub bands: Vec<BandEntry>,
    /// The canonical style-id assignment. `obc-pack` numbers feature types 1-based in
    /// config document order and every feature header in every chunk references those
    /// ids, so the assignment is part of the cells' bytes — schema data, never skin
    /// data (§4).
    pub styles: Vec<StyleAssignment>,
    /// Routing facts baked into the cells. The island-prune threshold is schema data:
    /// two cells pruned at different thresholds do not assemble into a graph with
    /// consistent semantics (`OBCA_Spec.md` §3.5).
    pub routing: RoutingEntry,
    /// The per-LOD chunk capacity bound the cells were written with (`OBCM_Spec.md`
    /// §3). An assembler re-checks every copied offset pair against it.
    pub chunk_size: u32,
}

/// `OBCA_Spec.md` §1.1's constants. Published so a consumer computing a cell's square
/// or an assembly bbox never hard-codes a power of two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GridEntry {
    /// `−2^28` µdeg, on both axes.
    pub origin_udeg: i32,
    /// `2^29` µdeg: the world box's side.
    pub world_side_udeg: i32,
}

/// One rung of the LOD ladder.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LodEntry {
    /// Ladder index, `0` = coarsest.
    pub index: u32,
    /// Meters-per-pixel upper bound; `null` for the `+inf` coarsest level
    /// (`OBCM_Spec.md` §3).
    pub max_mpp: Option<f64>,
    /// The band whose cells carry this LOD.
    pub band: String,
}

/// Which file of a volume set a band's content assembles into
/// (`OBCA_Spec.md` §5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BandRole {
    /// The one unsplittable file: nav graph, POIs, style table. Carries no LOD,
    /// because its headroom under `4 GiB − 1` is the design's hard limit.
    Core,
    /// The single whole-assembly coarse shard that keeps a zoomed-out viewport a
    /// one-file read.
    Coarse,
    /// An ordinary geometry shard's content; splits by bbox as needed.
    Geometry,
}

/// A non-geometry OBCM section a band may carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum BandSection {
    /// The navigation graph (`OBCM_Spec.md` §8).
    Nav,
    /// POIs and the hours pool (`OBCM_Spec.md` §7).
    Poi,
}

/// One band: a cell size plus the content its cells carry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BandEntry {
    /// Stable band id, e.g. `fine`. Also the band's segment in a cell URL.
    pub id: String,
    /// `log2(S)` in µdeg.
    pub cell_log2: u8,
    /// Ladder LODs this band's cells carry, ascending. Empty for the `core` band.
    pub lods: Vec<u32>,
    /// Non-geometry sections this band's cells carry. Only the `core` band has any.
    pub sections: Vec<BandSection>,
    /// Which file of a volume set this band becomes.
    pub role: BandRole,
}

/// One feature type's canonical style id — the assignment baked into every chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct StyleAssignment {
    /// 1-based style id, as referenced by feature headers (`OBCM_Spec.md` §2, §5.2).
    pub id: u8,
    /// `<tag_key>.<tag_value>`, e.g. `highway.primary`.
    pub feature_type: String,
}

/// The routing facts a cell's nav bytes were produced under.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RoutingEntry {
    /// Island-prune threshold, applied at **assembly** over the merged graph
    /// (`OBCA_Spec.md` §3.5/§4.6) — a cell bake prunes only strictly interior
    /// components.
    pub min_component_edges: u32,
    /// Profile names in `OBCM_Spec.md` §8.6 table order.
    pub profiles: Vec<String>,
}

/// A skin is stamped onto ~2 KB of an assembly at assembly time, so it is baked
/// into nothing and invalidates nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkinEntry {
    pub id: String,
    pub name: String,
    pub description: String,
    /// The skin's content version. No cell carries a skin.
    pub version: u32,
    /// RGB565 user-position marker color (`OBCM_Spec.md` §1).
    pub marker_color: u16,
    /// One entry per `schema.styles` feature type, in schema id order. A skin that
    /// misses one would ship a map with an invisible layer; one that names a feature
    /// type the schema lacks is stale (§5).
    pub styles: Vec<SkinStyle>,
    /// Optional digest-pinned rendered sample. It is presentation only and never
    /// participates in cell selection or assembly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<SkinPreview>,
}

/// One skin rendered over the bakery's canonical map scene.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkinPreview {
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
}

/// One feature type's presentation values — the bytes of a style record a skin owns (colors,
/// weight, z, priority and the `Flags` bits). The **id** is not here: it belongs to the schema, and
/// a skin may not renumber it (`OBCA_Spec.md` §4.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SkinStyle {
    pub feature_type: String,
    /// RGB565.
    pub color: u16,
    pub weight: u8,
    pub z_index: i8,
    /// 1..=4.
    pub priority: u8,
    pub dashed: bool,
    /// Style-record flag bit 4 (#1095): the weight is used verbatim on screen, off the zoom width
    /// ramp. Defaulted so a catalog written before the bit existed still parses.
    #[serde(default)]
    pub fixed_width: bool,
    /// Style-record flag bit 5 (#1095): part of the suppressible terrain layer.
    #[serde(default)]
    pub terrain_layer: bool,
    /// Optional RGB565 secondary color; `null` when the style has none.
    pub color2: Option<u16>,
}

/// A named region: a boundary to draw and a cell set to fetch (§6).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RegionEntry {
    /// Slash-separated id, matching the extract hierarchy.
    pub id: String,
    pub name: String,
    /// The enclosing region's id, when the curation nests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// Simplified outline for drawing (§7). Presentation only.
    pub boundary: Boundary,
    /// Total bytes of every cell in this region's set, across all bands.
    pub bytes: u64,
    /// Those bytes per band, summing to `bytes`. This is what makes the §5.7
    /// pre-download projection *per file* rather than merely per set: a volume set's
    /// roles partition by band, so the `core` band's bytes are the core file's bytes.
    pub bytes_by_band: BTreeMap<String, u64>,
    /// Cells per band.
    pub cell_count: BTreeMap<String, u32>,
    /// Partial cells per band, including zeroes for fully covered bands.
    pub partial_cell_count_by_band: BTreeMap<String, u32>,
    /// This region's terrain footprint (§13.3), when the catalog publishes terrain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terrain: Option<RegionTerrain>,
    /// Where the region's cell-id list lives (§6).
    pub cells_url: String,
    /// Size of that document, in bytes.
    pub cells_bytes: u64,
    /// Its digest — the pin that keeps the satellite all-or-nothing.
    pub cells_sha256: String,
}

/// A region's simplified outline: rings of `[lat, lon]` integer microdegrees
/// (§7). Microdegrees and lat-first, matching the OBCM header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Boundary {
    /// The tolerance the outline was reduced at.
    pub tolerance_udeg: i32,
    /// One or more closed rings; the first is an exterior and any ring nested in it is
    /// a hole. A region that is several disjoint pieces repeats that pattern.
    pub rings: Vec<boundary::Ring>,
}

/// The root's pin on one band's cell index (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CellIndexRef {
    pub band: String,
    /// `log2(S)`; matches the band's entry in `schema.bands`.
    pub cell_log2: u8,
    /// Downloadable OBCM artifact entries in the referenced document.
    pub cell_count: u32,
    /// Canonical cells represented as verified-empty ranges rather than OBCM artifacts.
    pub known_empty_count: u32,
    /// Size of the referenced document.
    pub bytes: u64,
    /// Its digest. A satellite that does not match MUST be rejected and the root kept.
    pub sha256: String,
    pub url: String,
}

/// A band's cell index: the satellite the root pins (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CellIndexDocument {
    pub schema_version: u32,
    /// The revision every cell in this document was baked at.
    pub schema_revision: u32,
    pub band: String,
    /// Sorted by `(i, j)`.
    pub cells: Vec<CellEntry>,
    /// Canonical, non-overlapping row runs that carry no bytes for this band.
    pub known_empty: Vec<KnownEmptyRun>,
}

/// An inclusive row run of cells proven to carry no content for one band.
///
/// Empty coverage is explicit because absence means a coverage hole. Runs avoid
/// publishing both an OBCM object and one JSON entry for every empty ocean or
/// network cell in a planet catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct KnownEmptyRun {
    /// First canonical cell id in the run.
    pub start: String,
    /// Last canonical cell id in the run (inclusive, same row as `start`).
    pub end: String,
    /// RFC 3339 UTC, recorded by the bake job.
    pub built_at: String,
    /// Every source extract against which emptiness was established.
    pub sources: Vec<CellSource>,
}

/// One published cell. **No bbox, deliberately**: the `id` determines the square and
/// the generator verifies the artifact's header against it (§8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CellEntry {
    /// Canonical cell id, `<log2>/<i>/<j>` (`OBCA_Spec.md` §1.3).
    pub id: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
    /// RFC 3339 UTC, recorded by the bake job.
    pub built_at: String,
    /// Every source extract this cell was baked from, sorted by `extract_id`.
    pub sources: Vec<CellSource>,
    /// `true` iff those sources do not fully cover the cell's square
    /// (`OBCA_Spec.md` §3.7). A consumer MUST NOT present a partial cell as canonical
    /// coverage.
    pub partial: bool,
}

/// One source extract behind a cell.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
pub struct CellSource {
    /// The extract's identifier, e.g. `europe/switzerland`.
    pub extract_id: String,
    /// That extract's snapshot date, `YYYY-MM-DD`.
    pub snapshot: String,
}

/// A region's cell list: the satellite `RegionEntry::cells_url` points at (§6).
///
/// Cell ids only — every other fact is in the band's index, keyed by the same id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegionCellsDocument {
    pub schema_version: u32,
    pub schema_revision: u32,
    pub region_id: String,
    /// Band id → sorted cell ids.
    pub cells: BTreeMap<String, Vec<String>>,
    /// Sorted terrain cell ids on the terrain grid (§13.3). A separate field rather
    /// than a `cells` key: `cells` is keyed by *schema band*, and terrain is not a
    /// band — it has no LOD, no section, no assembly role and no schema revision.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub terrain: Vec<String>,
}

// --- the terrain artifact class (§13) -----------------------------------------------------

/// The catalog's terrain block: what the raster is, at what resolution, and the one
/// pinned index that lists its cells (`OBCC_Spec.md` §13.1).
///
/// The four fields `dataset_version`, `posting_log2`, `cell_log2` and
/// `terrain_revision` are terrain's **whole** lockstep rule (§13.2). Nothing about the
/// OBCM store appears here, and nothing about terrain appears in [`SchemaEntry`] —
/// which is the shape of the independence, not a comment about it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerrainEntry {
    /// Stable kebab-case id of the source dataset, e.g. `copernicus-glo-30`.
    pub dataset_id: String,
    /// That dataset's release identity, e.g. `2021-1`. Opaque to a consumer: it is
    /// compared for equality, never parsed.
    pub dataset_version: String,
    /// `log2(P)` of the sample lattice, µdeg (`OBCT_Spec.md` §1.1).
    pub posting_log2: u8,
    /// `log2(S)` of the terrain cell, µdeg. Independent of any band's `cell_log2`.
    pub cell_log2: u8,
    /// Monotone content revision of the terrain store, bumped by a re-bake. Unrelated
    /// to `schema.revision`: neither invalidates the other (§13.2).
    pub terrain_revision: u32,
    /// The source licence's required credit, verbatim, so a consumer displays it from
    /// the catalog rather than hard-coding a string that can go stale (§13.5).
    pub attribution: String,
    /// The single pinned terrain cell index.
    pub cell_index: TerrainIndexRef,
}

/// The root's pin on the terrain cell index — the §8 machinery, one document instead
/// of one per band, because terrain has no bands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerrainIndexRef {
    /// Downloadable OBCT artifact entries in the referenced document.
    pub cell_count: u32,
    /// Canonical cells represented as all-`NODATA` runs rather than OBCT artifacts.
    pub known_empty_count: u32,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
}

/// The terrain cell index: the satellite the root's terrain block pins (§13.1).
///
/// It restates the four lockstep fields so the document is self-describing when it is
/// fetched on its own, and it deliberately carries **no** `schema_revision`: a terrain
/// cell does not know which OBCM schema it is being used beside, and adding the field
/// would make an OBCM bump rewrite this document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerrainIndexDocument {
    pub schema_version: u32,
    pub terrain_revision: u32,
    pub dataset_id: String,
    pub dataset_version: String,
    pub posting_log2: u8,
    pub cell_log2: u8,
    /// Sorted by cell id.
    pub cells: Vec<TerrainCellEntry>,
    /// Canonical, non-overlapping row runs whose every sample is `NODATA` — ocean.
    pub known_empty: Vec<TerrainEmptyRun>,
}

/// One published terrain cell. No bbox and no source list: the `id` is the square
/// (§13.1), and the provenance is one dataset stated once in the root block rather
/// than repeated on every one of thousands of entries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerrainCellEntry {
    /// Canonical cell id, `<cell_log2>/<i>/<j>`, on the terrain grid.
    pub id: String,
    pub bytes: u64,
    pub sha256: String,
    pub url: String,
    /// RFC 3339 UTC, recorded by the bake job.
    pub built_at: String,
}

/// An inclusive row run of terrain cells that are all `NODATA` — open ocean, which
/// [`obc-dem`](https://github.com/timohueser/OpenBikeComputer) does not write an object
/// for at all (`OBCT_Spec.md` §4.3 makes an absent cell and an all-void one answer
/// identically). The catalog says so instead, so a hole is still distinguishable from
/// a coverage gap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TerrainEmptyRun {
    pub start: String,
    pub end: String,
    pub built_at: String,
}

/// A region's terrain footprint, when the catalog publishes terrain. Kept out of
/// `bytes`/`bytes_by_band`, which are the OBCM volume set's per-file projection: a
/// rider may take the map without the raster, so the two prices are separate numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RegionTerrain {
    pub cell_count: u32,
    pub known_empty_count: u32,
    /// Sum of the real bytes of this region's downloadable terrain cells.
    pub bytes: u64,
}
