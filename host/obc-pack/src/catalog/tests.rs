//! Tests for the `schema_version 2` producer.
//!
//! The synthetic tree in [`example_tree`] is the source of the four checked-in
//! worked examples, so it is deliberately small and deliberately covers the shapes a
//! consumer has to handle: four bands (two of them the same cell size), a partial
//! cell, a co-baked border cell with two sources, a nested region, a skin with a
//! preview, and a terrain artifact class on its own revision track.

use std::fs;

use super::*;

use obc_formats::io::put_i32;
use obc_formats::obcm::{HEADER_LEN, MAGIC};

// --- fixtures ------------------------------------------------------------------------------

/// A scratch directory that removes itself (the packer
/// builds its own temp paths rather than adding a dependency for one).
struct TempTree(PathBuf);

impl TempTree {
    fn new(tag: &str) -> TempTree {
        let mut p = std::env::temp_dir();
        p.push(format!("obc-catalog-{tag}-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).expect("temp tree");
        TempTree(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, body).expect("write");
}

/// The style block both the schema and every skin must agree on: four feature types,
/// so ids 1..=4 in document order.
const FEATURES: &str = r#""features": {
    "highway": {
      "primary": { "color": "0xFAA0", "z_index": 60, "weight": 3, "priority": 2, "min_lod": 0 },
      "track": { "color": "0xAA80", "z_index": 30, "weight": 1, "priority": 3, "min_lod": 2, "line_style": "dashed" }
    },
    "natural": {
      "water": { "color": "0x55DF", "z_index": 10, "weight": 1, "priority": 3, "min_lod": 0 }
    },
    "landuse": {
      "forest": { "color": "0x5B45", "z_index": 5, "weight": 1, "priority": 4, "min_lod": 1 }
    }
  }"#;

/// The same four feature types, in the same document order — so the same ids — with
/// only the presentation values a **skin** is allowed to state. `min_lod` is missing
/// on purpose: it decides the level a feature is first written at, which is a decision
/// already baked into every cell a skin gets stamped onto, so a skin carrying it is
/// refused ([`super::check_skin_document`]).
const SKIN_FEATURES: &str = r#""features": {
    "highway": {
      "primary": { "color": "0xFAA0", "z_index": 60, "weight": 3, "priority": 2 },
      "track": { "color": "0xAA80", "z_index": 30, "weight": 1, "priority": 3, "line_style": "dashed" }
    },
    "natural": {
      "water": { "color": "0x55DF", "z_index": 10, "weight": 1, "priority": 3 }
    },
    "landuse": {
      "forest": { "color": "0x5B45", "z_index": 5, "weight": 1, "priority": 4 }
    }
  }"#;

const BANDS: &str = r#""bands": [
      { "id": "coarse", "cell_log2": 20, "lods": [0], "role": "coarse" },
      { "id": "mid", "cell_log2": 19, "lods": [1], "role": "geometry" },
      { "id": "fine", "cell_log2": 18, "lods": [2], "role": "geometry" },
      { "id": "network", "cell_log2": 18, "sections": ["nav", "poi"], "role": "core" }
    ]"#;

const SCHEMA_BLURB: &str =
    "Three LOD rungs over four bands — the shape of the shipped bikepacking ladder at example scale.";

/// `schema.json`: the packer config the cells were baked with, plus the `_meta` block
/// that names the revision and the band table.
fn schema_doc(revision: u32) -> String {
    format!(
        r#"{{
  "_meta": {{
    "id": "bikepacking",
    "name": "Bikepacking",
    "description": "{SCHEMA_BLURB}",
    "revision": {revision},
    {BANDS}
  }},
  "chunk_size": 4096,
  "lods": [
    {{ "max_mpp": null, "simplify": 200, "min_area_px": 50 }},
    {{ "max_mpp": 16, "simplify": 40, "min_area_px": 30 }},
    {{ "max_mpp": 3, "simplify": 3 }}
  ],
  "marker": {{ "color": "0xF800" }},
  "routing": {{
    "min_component_edges": 50,
    "profiles": [
      {{ "name": "Road", "default": 2.0, "highway": {{ "primary": 1.5 }} }},
      {{ "name": "Gravel", "default": 1.5, "highway": {{ "track": 1.0 }} }}
    ]
  }},
  {FEATURES}
}}
"#
    )
}

/// A skin: the same feature types and the same ids, different values.
fn skin_doc(id: &str, name: &str, description: &str, version: u32, marker: &str) -> String {
    format!(
        r#"{{
  "_meta": {{
    "id": "{id}",
    "name": "{name}",
    "description": "{description}",
    "version": {version}
  }},
  "marker": {{ "color": "{marker}" }},
  {SKIN_FEATURES}
}}
"#
    )
}

fn write_schema(tree: &Path, revision: u32) {
    write(&tree.join(SCHEMA_DOC), &schema_doc(revision));
}

fn write_skin(tree: &Path, id: &str, name: &str, description: &str, version: u32, marker: &str) {
    write(&tree.join(SKINS_DIR).join(format!("{id}.json")), &skin_doc(id, name, description, version, marker));
}

/// A minimal but *real* OBCM header — magic, version, bbox — which is everything this
/// generator reads out of a cell and nothing it does not.
fn obcm_bytes(version: u8, square: UBox, pad: usize) -> Vec<u8> {
    let (min_lon, min_lat, max_lon, max_lat) = square;
    let mut h = vec![0u8; HEADER_LEN + pad];
    h[..4].copy_from_slice(&MAGIC);
    h[4] = version;
    put_i32(&mut h, 5, min_lat as i32);
    put_i32(&mut h, 9, min_lon as i32);
    put_i32(&mut h, 13, max_lat as i32);
    put_i32(&mut h, 17, max_lon as i32);
    h
}

fn cell(log2: u32, i: i64, j: i64) -> CellId {
    CellId { log2, i, j }
}

fn cell_dir(tree: &Path, band: &str, id: CellId) -> PathBuf {
    let w = id_width(id.log2);
    tree.join(CELLS_DIR).join(band).join(format!("{:0w$}", id.i, w = w))
}

fn cell_path(tree: &Path, band: &str, id: CellId, ext: &str) -> PathBuf {
    let w = id_width(id.log2);
    cell_dir(tree, band, id).join(format!("{:0w$}{ext}", id.j, w = w))
}

/// The schema revision the example tree is baked at. A revision, not `1`, because the
/// interesting property is that the number in every cell sidecar and every satellite
/// is the *same* number — a store that mixes revisions is refused (`OBCA_Spec.md` §6.3).
const EXAMPLE_REVISION: u32 = 7;

/// Write a cell artifact whose header bbox *is* its grid square, plus its sidecar.
fn write_cell(
    tree: &Path,
    band: &str,
    id: CellId,
    pad: usize,
    built_at: &str,
    sources: &[(&str, &str)],
    partial: bool,
) {
    write_cell_at(tree, band, id, OBCM_VERSION, id.square(), pad, EXAMPLE_REVISION, built_at, sources, partial);
}

#[allow(clippy::too_many_arguments)]
fn write_cell_at(
    tree: &Path,
    band: &str,
    id: CellId,
    version: u8,
    square: UBox,
    pad: usize,
    revision: u32,
    built_at: &str,
    sources: &[(&str, &str)],
    partial: bool,
) {
    let dir = cell_dir(tree, band, id);
    fs::create_dir_all(&dir).expect("mkdir");
    fs::write(cell_path(tree, band, id, CELL_EXT), obcm_bytes(version, square, pad)).expect("cell");
    let sources: Vec<String> = sources
        .iter()
        .map(|(extract, snapshot)| format!("\n    {{ \"extract_id\": \"{extract}\", \"snapshot\": \"{snapshot}\" }}"))
        .collect();
    write(
        &cell_path(tree, band, id, CELL_SIDECAR_EXT),
        &format!(
            "{{\n  \"schema_revision\": {revision},\n  \"built_at\": \"{built_at}\",\n  \"sources\": [{}\n  ],\n  \
             \"partial\": {partial},\n  \"terrain_revision\": {TERRAIN_REVISION}\n}}\n",
            sources.join(",")
        ),
    );
}

fn write_known_empty(tree: &Path, band: &str, runs: Vec<KnownEmptyRun>) {
    let dir = tree.join(CELLS_DIR).join(band);
    fs::create_dir_all(&dir).expect("known-empty dir");
    let state = KnownEmptyState { schema_revision: EXAMPLE_REVISION, band: band.to_string(), known_empty: runs };
    let mut text = serde_json::to_string_pretty(&state).expect("known-empty JSON");
    text.push('\n');
    write(&dir.join(KNOWN_EMPTY_STATE_NAME), &text);
}

/// A rectangular Osmosis `.poly`, which is all a fixture outline needs to be.
fn poly(name: &str, lon: (f64, f64), lat: (f64, f64)) -> String {
    format!(
        "{name}\n1\n   {:.6}   {:.6}\n   {:.6}   {:.6}\n   {:.6}   {:.6}\n   {:.6}   {:.6}\n   {:.6}   {:.6}\nEND\nEND\n",
        lon.0, lat.0, lon.1, lat.0, lon.1, lat.1, lon.0, lat.1, lon.0, lat.0
    )
}

fn write_region(tree: &Path, id: &str, name: &str, cells: &[(&str, Vec<CellId>)], lon: (f64, f64), lat: (f64, f64)) {
    write_region_with_terrain(tree, id, name, cells, &[], lon, lat);
}

fn write_region_with_terrain(
    tree: &Path,
    id: &str,
    name: &str,
    cells: &[(&str, Vec<CellId>)],
    terrain: &[CellId],
    lon: (f64, f64),
    lat: (f64, f64),
) {
    let dir = tree.join(REGIONS_DIR).join(id);
    let bands: Vec<String> = cells
        .iter()
        .map(|(band, ids)| {
            let list: Vec<String> = ids.iter().map(|c| format!("\"{c}\"")).collect();
            format!("\n    \"{band}\": [{}]", list.join(", "))
        })
        .collect();
    let terrain_key = match terrain {
        [] => String::new(),
        ids => format!(",\n  \"terrain\": [{}]", ids.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(", ")),
    };
    write(
        &dir.join(REGION_DOC),
        &format!("{{\n  \"name\": \"{name}\",\n  \"cells\": {{{}\n  }}{terrain_key}\n}}\n", bands.join(",")),
    );
    write(&dir.join(REGION_POLY), &poly(id, lon, lat));
}

// --- the terrain fixture (§13) --------------------------------------------------------------

/// The example's terrain pairing: the **smallest** OBCT permits (a cell exactly one tile wide), so
/// a checked-in fixture is 548 bytes instead of 2 MiB. A real bake is `2^9` / `2^19`
/// (`OBCT_Spec.md` §1.3); nothing in this generator reads the pairing except to check it against
/// the cells' own headers, so the small one exercises the identical path.
const TERRAIN_POSTING_LOG2: u8 = 9;
/// Deliberately a cell size **no band uses**. Terrain is not a band, and its grid is chosen for the
/// raster rather than for a LOD ladder — a fixture where the two happened to coincide would hide a
/// generator that had quietly keyed terrain off a band.
const TERRAIN_CELL_LOG2: u8 = 13;
const TERRAIN_REVISION: u32 = 3;
const TERRAIN_DATASET_VERSION: &str = "2021-1";
/// The credit `obc-dem` owns and the bakery stamps; restated here because this crate has no
/// dependency on `obc-dem` and must not grow one to read a raster it never decodes.
const TERRAIN_ATTRIBUTION: &str = "produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and © Airbus \
                                   Defence and Space GmbH 2014-2018 provided under COPERNICUS by the European Union \
                                   and ESA; all rights reserved";

/// A terrain cell of the example's grid.
fn terrain_cell(i: i64, j: i64) -> CellId {
    cell(u32::from(TERRAIN_CELL_LOG2), i, j)
}

/// The four terrain cells the example publishes, and the ocean square beside them. The first is the
/// north-west corner of `fine_west`'s square, which the power-of-two nesting makes exact.
fn terrain_nw() -> CellId {
    terrain_cell(38_528, 33_664)
}
fn terrain_ne() -> CellId {
    terrain_cell(38_528, 33_665)
}
fn terrain_sea() -> CellId {
    terrain_cell(38_528, 33_667)
}

/// A real OBCT 1 × 1 container: header, a one-entry directory, one cell block
/// (`OBCT_Spec.md` §4.1). Hand-written from `obc-formats`' own field offsets for the same reason
/// [`obcm_bytes`] is: a fixture built by the code that reads it proves only self-consistency.
fn obct_bytes(posting_log2: u8, id: CellId, fill: u8) -> Vec<u8> {
    let block_len = obct::cell_block_len(posting_log2, id.log2 as u8).expect("a pairing OBCT permits") as usize;
    let mut out = vec![0u8; obct::HEADER_LEN + obct::DIR_ENTRY_LEN + block_len];
    out[obct::HDR_MAGIC..obct::HDR_MAGIC + 4].copy_from_slice(&obct::MAGIC);
    out[obct::HDR_VERSION] = obct::VERSION;
    out[obct::HDR_POSTING_LOG2] = posting_log2;
    out[obct::HDR_CELL_LOG2] = id.log2 as u8;
    out[obct::HDR_CELL_MIN_I..obct::HDR_CELL_MIN_I + 4].copy_from_slice(&(id.i as u32).to_le_bytes());
    out[obct::HDR_CELL_MIN_J..obct::HDR_CELL_MIN_J + 4].copy_from_slice(&(id.j as u32).to_le_bytes());
    out[obct::HDR_CELL_ROWS..obct::HDR_CELL_ROWS + 2].copy_from_slice(&1u16.to_le_bytes());
    out[obct::HDR_CELL_COLS..obct::HDR_CELL_COLS + 2].copy_from_slice(&1u16.to_le_bytes());
    let dir_at = obct::HEADER_LEN as u32;
    out[obct::HDR_DIRECTORY_OFFSET..obct::HDR_DIRECTORY_OFFSET + 4].copy_from_slice(&dir_at.to_le_bytes());
    let block_at = dir_at + obct::DIR_ENTRY_LEN as u32;
    out[obct::HEADER_LEN..obct::HEADER_LEN + 4].copy_from_slice(&block_at.to_le_bytes());
    out[block_at as usize..].fill(fill);
    out
}

fn terrain_doc_json(revision: u32, dataset_version: &str) -> String {
    format!(
        "{{\n  \"dataset_id\": \"copernicus-glo-30\",\n  \"dataset_version\": \"{dataset_version}\",\n  \
         \"posting_log2\": {TERRAIN_POSTING_LOG2},\n  \"cell_log2\": {TERRAIN_CELL_LOG2},\n  \"revision\": \
         {revision},\n  \"attribution\": \"{TERRAIN_ATTRIBUTION}\"\n}}\n"
    )
}

fn terrain_path(tree: &Path, id: CellId, ext: &str) -> PathBuf {
    let w = id_width(id.log2);
    tree.join(CELLS_DIR).join(TERRAIN_DIR).join(format!("{:0w$}", id.i, w = w)).join(format!(
        "{:0w$}{ext}",
        id.j,
        w = w
    ))
}

fn write_terrain_cell(tree: &Path, id: CellId, fill: u8, built_at: &str, revision: u32, dataset_version: &str) {
    let path = terrain_path(tree, id, TERRAIN_EXT);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(&path, obct_bytes(TERRAIN_POSTING_LOG2, id, fill)).expect("terrain cell");
    write(
        &terrain_path(tree, id, TERRAIN_SIDECAR_EXT),
        &format!(
            "{{\n  \"terrain_revision\": {revision},\n  \"dataset_version\": \"{dataset_version}\",\n  \
             \"built_at\": \"{built_at}\"\n}}\n"
        ),
    );
}

/// The terrain half of [`example_tree`]: two published cells, one all-`NODATA` ocean square, and
/// the declaration that gives them a dataset, a pairing and a revision.
fn write_terrain(tree: &Path, revision: u32, dataset_version: &str) {
    write(&tree.join(TERRAIN_DOC), &terrain_doc_json(revision, dataset_version));
    write_terrain_cell(tree, terrain_nw(), 0x11, "2026-08-01T04:00:00Z", revision, dataset_version);
    write_terrain_cell(tree, terrain_ne(), 0x22, "2026-08-01T04:00:03Z", revision, dataset_version);
    let state = format!(
        "{{\n  \"terrain_revision\": {revision},\n  \"known_empty\": [\n    {{\n      \"start\": \"{}\",\n      \
         \"end\": \"{}\",\n      \"built_at\": \"2026-08-01T04:00:07Z\"\n    }}\n  ]\n}}\n",
        terrain_sea(),
        terrain_sea()
    );
    write(&tree.join(CELLS_DIR).join(TERRAIN_DIR).join(KNOWN_EMPTY_STATE_NAME), &state);
}

// The cells the example publishes. `coarse`/`mid`/`fine` nest exactly (the grid's
// power-of-two nesting), and `network` shares `fine`'s size — which is why cell paths
// are keyed by band and not by `log2`.
fn coarse_cell() -> CellId {
    cell(20, 301, 263)
}
fn mid_cell() -> CellId {
    cell(19, 602, 526)
}
fn fine_west() -> CellId {
    cell(18, 1204, 1052)
}
fn fine_east() -> CellId {
    cell(18, 1204, 1053)
}
fn fine_empty() -> CellId {
    cell(18, 1204, 1055)
}

const DEFAULT_SKIN_BLURB: &str = "The full touring look: warm through-roads, brown trails, indigo cycleways.";
const CONTRAST_SKIN_BLURB: &str = "High contrast: fewer greys, heavier strokes, for bright sun on the panel.";

/// The tree the checked-in examples are generated from.
fn example_tree(tree: &Path) {
    write_schema(tree, EXAMPLE_REVISION);
    write_skin(tree, "default", "Bikepacking", DEFAULT_SKIN_BLURB, 4, "0xF800");
    write_skin(tree, "contrast", "High contrast", CONTRAST_SKIN_BLURB, 1, "0x001F");
    // Preview images are optional at the generic catalog layer. The bakery
    // generates one for every shipped skin; one here exercises both shapes.
    fs::create_dir_all(tree.join(PREVIEWS_DIR)).expect("preview dir");
    fs::write(tree.join(PREVIEWS_DIR).join("default.png"), b"example preview png").expect("preview");

    let ch = [("europe/switzerland", "2026-07-19")];
    write_cell(tree, "coarse", coarse_cell(), 2_048, "2026-07-30T02:10:04Z", &ch, false);
    write_cell(tree, "mid", mid_cell(), 1_024, "2026-07-30T02:11:38Z", &ch, false);
    write_cell(tree, "fine", fine_west(), 512, "2026-07-30T02:12:55Z", &ch, false);
    // A border cell co-baked from both extracts that touch it — the sanctioned way to
    // make an edge cell canonical without a planet source (OBCA_Spec.md §3.7).
    write_cell(
        tree,
        "fine",
        fine_east(),
        384,
        "2026-07-30T02:13:07Z",
        &[("europe/germany/baden-wuerttemberg", "2026-07-18"), ("europe/switzerland", "2026-07-19")],
        false,
    );
    write_known_empty(
        tree,
        "fine",
        vec![KnownEmptyRun {
            start: fine_empty().to_string(),
            end: fine_empty().to_string(),
            built_at: "2026-07-30T02:13:11Z".into(),
            sources: vec![CellSource { extract_id: "planet".into(), snapshot: "2026-07-19".into() }],
        }],
    );
    write_cell(tree, "network", fine_west(), 256, "2026-07-30T02:14:41Z", &ch, false);
    // Baked from one side only: the sources do not cover the square, so it is partial
    // and a consumer must not present it as canonical coverage.
    write_cell(tree, "network", fine_east(), 128, "2026-07-30T02:14:52Z", &ch, true);
    // The other artifact class, at its own revision (§13) — deliberately not `EXAMPLE_REVISION`,
    // so the example itself demonstrates that the two numbers are unrelated.
    write_terrain(tree, TERRAIN_REVISION, TERRAIN_DATASET_VERSION);

    write_region_with_terrain(
        tree,
        "europe/switzerland",
        "Switzerland",
        &[
            ("coarse", vec![coarse_cell()]),
            ("mid", vec![mid_cell()]),
            ("fine", vec![fine_west(), fine_east(), fine_empty()]),
            ("network", vec![fine_west(), fine_east()]),
        ],
        &[terrain_nw(), terrain_ne(), terrain_sea()],
        (5.9, 10.5),
        (45.8, 47.8),
    );
    // A sub-region curated separately from its parent: `parent` comes from the tree's
    // own nesting, so the two cannot disagree.
    write_region_with_terrain(
        tree,
        "europe/switzerland/basel-stadt",
        "Basel-Stadt",
        &[
            ("coarse", vec![coarse_cell()]),
            ("mid", vec![mid_cell()]),
            ("fine", vec![fine_west()]),
            ("network", vec![fine_west()]),
        ],
        &[terrain_nw()],
        (7.5, 7.7),
        (47.5, 47.6),
    );
}

fn opts() -> CatalogOptions {
    CatalogOptions::new("https://maps.example.org/catalog/", "2026-07-30T09:00:00Z")
}

fn generated(tree: &Path) -> GeneratedCatalog {
    generate(tree, &opts()).expect("the example tree generates")
}

fn region<'a>(g: &'a GeneratedCatalog, id: &str) -> &'a RegionEntry {
    g.root.regions.iter().find(|r| r.id == id).expect("region")
}

fn satellite<'a>(g: &'a GeneratedCatalog, rel: &str) -> &'a Satellite {
    g.satellites.iter().find(|s| s.rel_path == rel).expect("satellite")
}

fn cell_index_doc(g: &GeneratedCatalog, band: &str) -> CellIndexDocument {
    serde_json::from_str(&satellite(g, &format!("cells/{band}/index.json")).body).expect("cell index parses")
}

// --- the grid ------------------------------------------------------------------------------
//
// The grid itself — the worked example's squares, the nesting, the padding widths — is
// pinned once in `crate::grid`'s own tests. What is the *catalog's* is the strict reading
// of an id, so that is what is pinned here.

#[test]
fn non_canonical_cell_ids_are_refused() {
    for bad in [
        "18/1204",            // not three parts
        "18/1204/1052/3",     // four
        "9/1204/1052",        // cell size below 2^10
        "29/1204/1052",       // above 2^28
        "18/204/1052",        // truncated padding
        "18/01204/1052",      // over-padded
        "18/2048/1052",       // off the grid (2^29/2^18 = 2048 rows)
        "18/12o4/1052",       // not a number
        "eighteen/1204/1052", // nor is that
    ] {
        assert!(parse_strict_id(bad).is_err(), "`{bad}` must be refused");
    }
}

// --- shape ---------------------------------------------------------------------------------

#[test]
fn walks_a_tree_into_a_root_and_its_satellites() {
    let t = TempTree::new("walk");
    example_tree(t.path());
    let g = generated(t.path());

    assert_eq!(g.root.schema_version, CATALOG_SCHEMA_VERSION);
    assert_eq!(g.root.schema.id, "bikepacking");
    assert_eq!(g.root.schema.revision, 7);
    assert_eq!(g.root.schema.obcm_version, OBCM_VERSION, "read from the cells' own headers");
    assert_eq!(g.root.schema.chunk_size, 4_096);
    assert_eq!(g.root.schema.routing.min_component_edges, 50);
    assert_eq!(g.root.schema.routing.profiles, ["Road", "Gravel"]);
    assert_eq!(
        g.root.schema.grid,
        GridEntry { origin_udeg: GRID_ORIGIN_UDEG, world_side_udeg: WORLD_SIDE_UDEG },
        "the constants are published so no consumer hard-codes them"
    );

    // The ladder, and which band carries each rung.
    assert_eq!(
        g.root.schema.lods.iter().map(|l| (l.index, l.max_mpp, l.band.as_str())).collect::<Vec<_>>(),
        [(0, None, "coarse"), (1, Some(16.0), "mid"), (2, Some(3.0), "fine")]
    );
    // The style-id assignment: 1-based in config document order, and it is schema data
    // because every feature header in every chunk references it.
    assert_eq!(
        g.root.schema.styles.iter().map(|s| (s.id, s.feature_type.as_str())).collect::<Vec<_>>(),
        [(1, "highway.primary"), (2, "highway.track"), (3, "natural.water"), (4, "landuse.forest")]
    );

    assert_eq!(g.root.skins.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["contrast", "default"], "sorted by id");
    assert_eq!(g.root.skins[0].preview, None, "a generic catalog may omit a preview");
    let preview = g.root.skins[1].preview.as_ref().expect("default preview");
    assert_eq!(preview.url, format!("https://maps.example.org/catalog/previews/default.{}.png", preview.sha256));
    assert_eq!(preview.bytes, 19);
    assert_eq!(preview.sha256.len(), 64);
    assert_eq!(
        g.root.regions.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        ["europe/switzerland", "europe/switzerland/basel-stadt"],
        "sorted by id"
    );
    // §11.2: `cell_index` is sorted by cell_log2 descending, with the band id breaking
    // the tie two same-size bands create.
    assert_eq!(
        g.root.cell_index.iter().map(|c| (c.band.as_str(), c.cell_log2, c.cell_count)).collect::<Vec<_>>(),
        [("coarse", 20, 1), ("mid", 19, 1), ("fine", 18, 2), ("network", 18, 2)]
    );
    assert_eq!(
        g.satellites.iter().map(|s| s.rel_path.as_str()).collect::<Vec<_>>(),
        [
            "cells/coarse/index.json",
            "cells/mid/index.json",
            "cells/fine/index.json",
            "cells/network/index.json",
            // The terrain index is a satellite like any other — one document, no band.
            "cells/terrain/index.json",
            "regions/europe/switzerland/cells.json",
            "regions/europe/switzerland/basel-stadt/cells.json",
        ]
    );
    assert!(g.warnings.is_empty(), "the example tree is complete: {:?}", g.warnings);
}

#[test]
fn a_cell_entry_states_the_bake_and_carries_no_bbox() {
    let t = TempTree::new("cells");
    example_tree(t.path());
    let g = generated(t.path());
    let doc = cell_index_doc(&g, "fine");

    assert_eq!(doc.schema_version, CATALOG_SCHEMA_VERSION);
    assert_eq!(doc.schema_revision, 7);
    assert_eq!(doc.band, "fine");
    assert_eq!(doc.cells.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["18/1204/1052", "18/1204/1053"]);

    let west = &doc.cells[0];
    assert_eq!(west.bytes, (HEADER_LEN + 512) as u64);
    assert_eq!(west.sha256.len(), 64);
    assert_eq!(west.url, format!("https://maps.example.org/catalog/cells/fine/1204/1052.{}.obcm", west.sha256));
    assert_eq!(west.built_at, "2026-07-30T02:12:55Z");
    assert!(!west.partial);
    assert_eq!(
        doc.cells[1].sources,
        [
            CellSource { extract_id: "europe/germany/baden-wuerttemberg".into(), snapshot: "2026-07-18".into() },
            CellSource { extract_id: "europe/switzerland".into(), snapshot: "2026-07-19".into() },
        ],
        "sources are published sorted by extract_id"
    );

    // §11.6: the id *is* the coverage statement, so there is no bbox to disagree with
    // it. Asserted on the serialized document, since that is what a consumer sees.
    let body = &satellite(&g, "cells/fine/index.json").body;
    assert!(!body.contains("bbox"), "a cell entry must not carry a bbox:\n{body}");
    assert!(cell_index_doc(&g, "network").cells[1].partial, "the one-sided border cell is partial");
}

#[test]
fn a_region_prices_its_cell_set_per_band() {
    let t = TempTree::new("regions");
    example_tree(t.path());
    let g = generated(t.path());
    let ch = region(&g, "europe/switzerland");

    assert_eq!(ch.name, "Switzerland");
    assert_eq!(ch.parent, None, "`europe` is a directory, not a curated region");
    assert_eq!(
        ch.cell_count,
        BTreeMap::from([
            ("coarse".to_string(), 1),
            ("mid".to_string(), 1),
            ("fine".to_string(), 3),
            ("network".to_string(), 2),
        ])
    );
    // §11.5: `bytes_by_band` must sum to `bytes` — that is what makes §5.7's per-file
    // projection arithmetic rather than estimation, because a volume set's roles
    // partition by band.
    assert_eq!(ch.bytes_by_band.values().sum::<u64>(), ch.bytes);
    let fine = cell_index_doc(&g, "fine");
    assert_eq!(ch.bytes_by_band["fine"], fine.cells.iter().map(|c| c.bytes).sum::<u64>());
    assert_eq!(fine.known_empty.len(), 1);
    assert_eq!(fine.known_empty[0].start, fine_empty().to_string());
    assert_eq!(fine.known_empty[0].end, fine_empty().to_string());
    let fine_ref = g.root.cell_index.iter().find(|entry| entry.band == "fine").expect("fine ref");
    assert_eq!(fine_ref.cell_count, 2, "only downloadable artifacts count here");
    assert_eq!(fine_ref.known_empty_count, 1, "verified-empty coverage is priced separately at zero bytes");
    assert_eq!(
        ch.partial_cell_count_by_band,
        BTreeMap::from([
            ("coarse".to_string(), 0),
            ("mid".to_string(), 0),
            ("fine".to_string(), 0),
            ("network".to_string(), 1),
        ]),
        "the root splits partials by band without fetching the cell list"
    );

    assert_eq!(
        ch.cells_url,
        format!("https://maps.example.org/catalog/regions/europe/switzerland/cells.{}.json", ch.cells_sha256)
    );
    let cells: RegionCellsDocument =
        serde_json::from_str(&satellite(&g, "regions/europe/switzerland/cells.json").body).expect("cells doc");
    assert_eq!(cells.region_id, "europe/switzerland");
    assert_eq!(cells.schema_revision, 7);
    assert_eq!(
        cells.cells["fine"],
        ["18/1204/1052", "18/1204/1053", "18/1204/1055"],
        "real and known-empty ids share the stored sorted selection"
    );

    let basel = region(&g, "europe/switzerland/basel-stadt");
    assert_eq!(basel.parent.as_deref(), Some("europe/switzerland"), "parent is the nearest enclosing region");
    assert!(basel.bytes < ch.bytes, "a sub-selection is cheaper");
    assert!(basel.partial_cell_count_by_band.values().all(|&count| count == 0));

    // Overlap is free: two regions that share ground share the same cells, and
    // the store pays for them once. That is the epic's headline saving.
    assert_eq!(basel.bytes_by_band["coarse"], ch.bytes_by_band["coarse"]);
}

#[test]
fn a_region_boundary_is_a_drawable_outline() {
    let t = TempTree::new("boundary");
    example_tree(t.path());
    let g = generated(t.path());
    let b = &region(&g, "europe/switzerland").boundary;

    assert_eq!(b.tolerance_udeg, boundary::DEFAULT_TOLERANCE_UDEG);
    assert_eq!(b.rings.len(), 1);
    let ring = &b.rings[0];
    assert_eq!(ring.first(), ring.last(), "rings are closed");
    // `[lat, lon]` microdegrees, matching the OBCM header.
    assert!(ring.iter().all(|p| (45_800_000..=47_800_000).contains(&p[0])), "{ring:?}");
    assert!(ring.iter().all(|p| (5_900_000..=10_500_000).contains(&p[1])), "{ring:?}");
    // The outline is presentation only: it is not what the cell set was computed from,
    // and a region with one boundary ring still selects cells in four bands.
    assert_eq!(region(&g, "europe/switzerland").cell_count.len(), 4);
}

#[test]
fn a_skin_is_the_schema_recolored() {
    let t = TempTree::new("skins");
    example_tree(t.path());
    let g = generated(t.path());
    let default = g.root.skins.iter().find(|s| s.id == "default").expect("default skin");

    assert_eq!(default.name, "Bikepacking");
    assert_eq!(default.version, 4);
    assert_eq!(default.marker_color, 0xF800);
    // One entry per schema feature type, in the schema's id order, so `styles[k]`
    // lines up with `schema.styles[k]` without a join.
    assert_eq!(
        default.styles.iter().map(|s| s.feature_type.as_str()).collect::<Vec<_>>(),
        g.root.schema.styles.iter().map(|s| s.feature_type.as_str()).collect::<Vec<_>>()
    );
    let track = default.styles.iter().find(|s| s.feature_type == "highway.track").expect("track");
    assert!(track.dashed, "the dash bit is skin data");
    assert_eq!(track.color2, None);
    let contrast = g.root.skins.iter().find(|s| s.id == "contrast").expect("contrast skin");
    assert_eq!(contrast.marker_color, 0x001F);
    // `preset_version` has no place here: a skin is stamped at assembly time, so
    // no artifact can be a revision behind it (§11.4).
    let body = root_json(&g.root);
    assert!(!body.contains("preset_version"), "the lagging-artifact apparatus must stay absent");
    assert!(!body.contains("\"presets\""), "§11.9: `presets` must not appear in a catalog root");
}

/// A skin document carrying schema keys is **refused**, and the error names them.
///
/// Dropping them quietly is the tempting behaviour and the wrong one. A skin is
/// stamped onto cells already cut at the schema's ladder, tolerances, merge passes and
/// routing table, so a `lods` block in a skin has no effect whatsoever — and an author
/// who wrote one believes something false about the map they are shipping, with
/// nothing anywhere to tell them otherwise. Every offending key is named, not the
/// first, so one edit fixes the document.
///
/// This has to be checked against the JSON rather than the parsed [`Config`]: once
/// parsed, a config that omits `lods` and one that restates the defaults are the same
/// value, so `check_skin` cannot see the difference at all.
#[test]
fn a_skin_carrying_schema_keys_is_refused_by_name() {
    for (key, body, expect) in [
        ("lods", r#""lods": [{"max_mpp": null, "simplify": 200, "min_area_px": 50}],"#, "`lods`"),
        ("routing", r#""routing": {"min_component_edges": 50},"#, "`routing`"),
        ("merge_fills", r#""merge_fills": true,"#, "`merge_fills`"),
        ("merge_lines", r#""merge_lines": true,"#, "`merge_lines`"),
        ("chunk_size", r#""chunk_size": 4096,"#, "`chunk_size`"),
    ] {
        let doc = format!(
            r#"{{
  "_meta": {{ "id": "bad", "name": "Bad", "description": "Carries schema data.", "version": 1 }},
  {body}
  "marker": {{ "color": "0xF800" }},
  {SKIN_FEATURES}
}}
"#
        );
        let err = super::check_skin_document(&doc, "bad.json").expect_err("`{key}` is schema data");
        assert!(err.contains(expect), "the error must name `{key}`: {err}");
        assert!(err.contains("presentation only"), "{err}");
        assert!(err.contains("re-bake"), "and say what the real answer is: {err}");

        // And the generator refuses the whole tree, rather than publishing a skin whose
        // author's intent it silently discarded.
        let t = TempTree::new(&format!("skin-schema-key-{key}"));
        example_tree(t.path());
        write(&t.path().join(SKINS_DIR).join("bad.json"), &doc);
        let err = generate(t.path(), &opts()).expect_err("the tree is unpublishable");
        assert!(err.contains(expect), "{err}");
    }

    // The style-level one: `min_lod` is on nearly every line of the schema a skin gets
    // copied from, and it decides which level a feature is first written at.
    let doc = format!(
        r#"{{
  "_meta": {{ "id": "bad", "name": "Bad", "description": "Carries schema data.", "version": 1 }},
  "marker": {{ "color": "0xF800" }},
  {FEATURES}
}}
"#
    );
    let err = super::check_skin_document(&doc, "bad.json").expect_err("`min_lod` is schema data");
    assert!(err.contains("`features.*.*.min_lod`"), "{err}");

    // #1095's two style-record flag bits are presentation, not schema: they are bits of the 8-byte
    // record a skin stamps, and neither changes which features are cut into which cells — so a skin
    // may carry them, exactly as it may carry `line_style`.
    let doc = r#"{
  "_meta": { "id": "terrain", "name": "Terrain", "description": "Carries the flag bits.", "version": 1 },
  "marker": { "color": "0xF800" },
  "features": {
    "highway": {
      "track": { "color": "0xAA80", "z_index": 30, "weight": 1, "priority": 3, "line_style": "dashed",
                 "fixed_width": true, "terrain_layer": true }
    }
  }
}
"#;
    super::check_skin_document(doc, "terrain.json").expect("`fixed_width`/`terrain_layer` are presentation keys");

    // The shipped documents are the ones this all has to hold for.
    super::check_skin_document(&repo_doc(SHIPPED_SKIN), SHIPPED_SKIN).expect("the shipped skin is presentation only");
    let err = super::check_skin_document(&repo_doc(SHIPPED_SCHEMA), SHIPPED_SCHEMA)
        .expect_err("and the schema is emphatically not a skin");
    assert!(err.contains("`lods`") && err.contains("`routing`"), "{err}");
}

// --- determinism ---------------------------------------------------------------------------

#[test]
fn generation_is_deterministic_for_a_given_tree() {
    let a = TempTree::new("det-a");
    example_tree(a.path());
    let first = generated(a.path());
    let second = generated(a.path());
    assert_eq!(root_json(&first.root), root_json(&second.root), "twice over one tree must be byte-identical");
    assert_eq!(first.satellites, second.satellites);

    // The same content written in a different order, into a different directory: this
    // is what proves the ordering is content-derived rather than `read_dir`-derived.
    let b = TempTree::new("det-b");
    write_region_with_terrain(
        b.path(),
        "europe/switzerland/basel-stadt",
        "Basel-Stadt",
        &[
            ("network", vec![fine_west()]),
            ("fine", vec![fine_west()]),
            ("mid", vec![mid_cell()]),
            ("coarse", vec![coarse_cell()]),
        ],
        &[terrain_nw()],
        (7.5, 7.7),
        (47.5, 47.6),
    );
    let ch = [("europe/switzerland", "2026-07-19")];
    write_cell(b.path(), "network", fine_east(), 128, "2026-07-30T02:14:52Z", &ch, true);
    write_cell(b.path(), "network", fine_west(), 256, "2026-07-30T02:14:41Z", &ch, false);
    write_skin(b.path(), "contrast", "High contrast", CONTRAST_SKIN_BLURB, 1, "0x001F");
    write_cell(
        b.path(),
        "fine",
        fine_east(),
        384,
        "2026-07-30T02:13:07Z",
        // Reversed source order: the published list is sorted by extract_id, so this
        // must not move a byte.
        &[("europe/switzerland", "2026-07-19"), ("europe/germany/baden-wuerttemberg", "2026-07-18")],
        false,
    );
    write_cell(b.path(), "fine", fine_west(), 512, "2026-07-30T02:12:55Z", &ch, false);
    write_known_empty(
        b.path(),
        "fine",
        vec![KnownEmptyRun {
            start: fine_empty().to_string(),
            end: fine_empty().to_string(),
            built_at: "2026-07-30T02:13:11Z".into(),
            sources: vec![CellSource { extract_id: "planet".into(), snapshot: "2026-07-19".into() }],
        }],
    );
    write_cell(b.path(), "mid", mid_cell(), 1_024, "2026-07-30T02:11:38Z", &ch, false);
    write_cell(b.path(), "coarse", coarse_cell(), 2_048, "2026-07-30T02:10:04Z", &ch, false);
    write_terrain(b.path(), TERRAIN_REVISION, TERRAIN_DATASET_VERSION);
    write_region_with_terrain(
        b.path(),
        "europe/switzerland",
        "Switzerland",
        &[
            ("fine", vec![fine_empty(), fine_east(), fine_west()]),
            ("network", vec![fine_east(), fine_west()]),
            ("coarse", vec![coarse_cell()]),
            ("mid", vec![mid_cell()]),
        ],
        // Reversed, like the band lists above: the published list is sorted, so the order the
        // curation happened to be written in must not move a byte.
        &[terrain_sea(), terrain_ne(), terrain_nw()],
        (5.9, 10.5),
        (45.8, 47.8),
    );
    write_skin(b.path(), "default", "Bikepacking", DEFAULT_SKIN_BLURB, 4, "0xF800");
    fs::create_dir_all(b.path().join(PREVIEWS_DIR)).expect("preview dir");
    fs::write(b.path().join(PREVIEWS_DIR).join("default.png"), b"example preview png").expect("preview");
    write_schema(b.path(), EXAMPLE_REVISION);

    let other = generated(b.path());
    assert_eq!(root_json(&other.root), root_json(&first.root), "output must not depend on creation order");
    assert_eq!(other.satellites, first.satellites);
}

#[test]
fn only_generated_at_carries_a_clock() {
    let t = TempTree::new("clock");
    example_tree(t.path());
    let mut o = opts();
    o.generated_at = "2030-01-01T00:00:00Z".into();
    let later = generate(t.path(), &o).expect("generates");
    let base = generated(t.path());
    assert_ne!(base.root.generated_at, later.root.generated_at);
    assert_eq!(base.root.schema, later.root.schema, "nothing but generated_at may move with the clock");
    assert_eq!(base.root.regions, later.root.regions);
    assert_eq!(base.root.cell_index, later.root.cell_index);
    assert_eq!(base.satellites, later.satellites, "a satellite carries no clock at all");
}

// --- the digest pins -----------------------------------------------------------------------

#[test]
fn the_root_pins_every_satellite_by_size_and_digest() {
    let t = TempTree::new("pins");
    example_tree(t.path());
    let g = generated(t.path());

    let pin = |rel: &str| -> (u64, String) { hash_str(&satellite(&g, rel).body) };
    for entry in &g.root.cell_index {
        let (bytes, sha256) = pin(&format!("cells/{}/index.json", entry.band));
        assert_eq!((entry.bytes, entry.sha256.clone()), (bytes, sha256), "band `{}`", entry.band);
        assert!(entry.url.ends_with(&format!("cells/{}/index.{}.json", entry.band, entry.sha256)));
    }
    for r in &g.root.regions {
        let (bytes, sha256) = pin(&format!("regions/{}/cells.json", r.id));
        assert_eq!((r.cells_bytes, r.cells_sha256.clone()), (bytes, sha256), "region `{}`", r.id);
        assert!(r.cells_url.ends_with(&format!("regions/{}/cells.{}.json", r.id, r.cells_sha256)));
    }
    // Known-answer check on the digest encoding itself (NIST's SHA-256 of "abc"), so
    // "the pins agree with each other" cannot be vacuously true.
    assert_eq!(hash_str("abc"), (3, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string()));
}

/// §3.1: the root carries the source declaration, and its human-readable twin
/// `LICENSE.txt` lands beside `catalog.json` with the same facts — the OSM credit,
/// the ODbL, and (because the example publishes terrain) the Copernicus credit
/// from §13.5. Derived from the root, so the two cannot disagree.
#[test]
fn the_source_declaration_and_its_license_txt() {
    let t = TempTree::new("license");
    example_tree(t.path());
    let g = generated(t.path());

    let source = g.root.source.as_ref().expect("a generated root always carries a source block");
    assert_eq!(source.dataset_id, "openstreetmap");
    assert_eq!(source.license, "ODbL-1.0");

    write_all_atomic(t.path(), &g).expect("write");
    let text = fs::read_to_string(t.path().join(LICENSE_NAME)).expect("LICENSE.txt beside the root");
    assert!(text.contains(&source.attribution), "the credit travels verbatim:\n{text}");
    assert!(text.contains(&source.license_url), "the licence is locatable:\n{text}");
    let terrain = g.root.terrain.as_ref().expect("the example publishes terrain");
    assert!(text.contains(&terrain.attribution), "the terrain credit travels too (§13.5):\n{text}");
}

#[test]
fn write_all_atomic_writes_the_satellites_then_the_root() {
    let t = TempTree::new("write");
    example_tree(t.path());
    let g = generated(t.path());
    write_all_atomic(t.path(), &g).expect("write");

    // Every pinned satellite is on disk with exactly the bytes the root's digest
    // claims — the property that makes "root + matching satellite" as strong a
    // all-or-nothing document guarantee (§11.1).
    for s in &g.satellites {
        let path = t.path().join(s.rel_path.replace('/', std::path::MAIN_SEPARATOR_STR));
        assert_eq!(fs::read_to_string(&path).expect("satellite on disk"), s.body, "{}", s.rel_path);
    }
    let root_path = t.path().join(DEFAULT_MANIFEST_NAME);
    assert_eq!(fs::read_to_string(&root_path).unwrap(), root_json(&g.root));

    // Re-running over the tree it just wrote into must be a no-op: the generator's own
    // output is skipped by name, so a re-publish is idempotent.
    let again = generated(t.path());
    assert_eq!(root_json(&again.root), root_json(&g.root));
    assert!(!fs::read_to_string(&root_path).unwrap().is_empty());

    let leftovers: Vec<String> = walk(t.path()).into_iter().filter(|p| p.contains(".tmp")).collect();
    assert!(leftovers.is_empty(), "temp files must be renamed away: {leftovers:?}");
}

fn walk(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for entry in sorted_entries(dir).expect("readable") {
        if entry.is_dir() {
            out.extend(walk(&entry));
        } else {
            out.push(entry.display().to_string());
        }
    }
    out
}

// --- the version law + lockstep ------------------------------------------------------------

#[test]
fn a_cell_from_another_obcm_version_is_refused() {
    let t = TempTree::new("stale-cell");
    example_tree(t.path());
    let id = fine_west();
    write_cell_at(
        t.path(),
        "fine",
        id,
        OBCM_VERSION - 1,
        id.square(),
        512,
        7,
        "2026-07-30T02:12:55Z",
        &[("europe/switzerland", "2026-07-19")],
        false,
    );
    let err = generate(t.path(), &opts()).expect_err("a stale cell must fail the bake");
    assert!(err.contains(&format!("OBCM v{}", OBCM_VERSION - 1)), "{err}");
    assert!(err.contains("re-bake"), "the error must say what to do: {err}");
}

#[test]
fn a_mixed_schema_revision_is_refused() {
    let t = TempTree::new("mixed-revision");
    example_tree(t.path());
    let id = fine_east();
    write_cell_at(
        t.path(),
        "fine",
        id,
        OBCM_VERSION,
        id.square(),
        384,
        6, // the tree says 7
        "2026-07-30T02:13:07Z",
        &[("europe/switzerland", "2026-07-19")],
        false,
    );
    let err = generate(t.path(), &opts()).expect_err("a mixed-revision store must fail");
    assert!(err.contains("schema revision 6"), "{err}");
    assert!(err.contains("invalidates every cell"), "{err}");
}

/// The *identifier* describes the cell, and
/// the bytes are verified against the identifier.
#[test]
fn a_cell_whose_header_is_not_its_square_is_refused() {
    let t = TempTree::new("bad-square");
    example_tree(t.path());
    let id = fine_west();
    let mut square = id.square();
    square.2 -= 1; // max_lon: the content-derived box a normal pack would compute
    write_cell_at(
        t.path(),
        "fine",
        id,
        OBCM_VERSION,
        square,
        512,
        7,
        "2026-07-30T02:12:55Z",
        &[("europe/switzerland", "2026-07-19")],
        false,
    );
    let err = generate(t.path(), &opts()).expect_err("a cell must be its square");
    assert!(err.contains("grid square"), "{err}");
    assert!(err.contains("verbatim"), "the error must say why it matters: {err}");
}

// --- loud failures -------------------------------------------------------------------------

#[test]
fn a_region_naming_an_unpublished_cell_fails() {
    let t = TempTree::new("phantom-cell");
    example_tree(t.path());
    write_region(
        t.path(),
        "europe/switzerland",
        "Switzerland",
        &[
            ("coarse", vec![coarse_cell()]),
            ("mid", vec![mid_cell()]),
            ("fine", vec![fine_west(), cell(18, 1205, 1052)]),
        ],
        (5.9, 10.5),
        (45.8, 47.8),
    );
    let err = generate(t.path(), &opts()).expect_err("a phantom cell must fail");
    assert!(err.contains("18/1205/1052") && err.contains("not published"), "{err}");
}

#[test]
fn a_region_cell_in_the_wrong_band_fails() {
    let t = TempTree::new("wrong-band");
    example_tree(t.path());
    write_region(
        t.path(),
        "europe/switzerland",
        "Switzerland",
        &[("fine", vec![coarse_cell()])],
        (5.9, 10.5),
        (45.8, 47.8),
    );
    let err = generate(t.path(), &opts()).expect_err("a mis-banded cell must fail");
    assert!(err.contains("but band `fine` is"), "{err}");
}

#[test]
fn a_region_missing_a_band_is_a_warning_not_an_error() {
    let t = TempTree::new("thin-region");
    example_tree(t.path());
    write_region(
        t.path(),
        "europe/switzerland",
        "Switzerland",
        &[("coarse", vec![coarse_cell()]), ("mid", vec![mid_cell()]), ("fine", vec![fine_west()])],
        (5.9, 10.5),
        (45.8, 47.8),
    );
    let g = generated(t.path());
    assert!(
        g.warnings.iter().any(|w| w.contains("europe/switzerland") && w.contains("`network`")),
        "a missing band must be reported, not silent: {:?}",
        g.warnings
    );
    assert_eq!(region(&g, "europe/switzerland").cell_count["network"], 0);
}

#[test]
fn a_skin_that_is_not_this_schema_fails() {
    // Missing a feature type: the map would ship with an invisible layer.
    let t = TempTree::new("thin-skin");
    example_tree(t.path());
    write(
        &t.path().join(SKINS_DIR).join("contrast.json"),
        r#"{"_meta": {"id": "contrast", "name": "High contrast", "description": "Fewer greys.", "version": 1},
            "features": {"highway": {"primary": {"color": "0xF800"}, "track": {"color": "0xAA80"}},
                         "natural": {"water": {"color": "0x55DF"}}}}"#,
    );
    let err = generate(t.path(), &opts()).expect_err("a skin missing a style must fail");
    assert!(err.contains("landuse.forest") && err.contains("invisible layer"), "{err}");

    // A feature type the schema does not have: a new type is a new schema revision.
    let u = TempTree::new("fat-skin");
    example_tree(u.path());
    write(
        &u.path().join(SKINS_DIR).join("contrast.json"),
        r#"{"_meta": {"id": "contrast", "name": "High contrast", "description": "Fewer greys.", "version": 1},
            "features": {"highway": {"primary": {"color": "0xF800"}, "track": {"color": "0xAA80"}},
                         "natural": {"water": {"color": "0x55DF"}},
                         "landuse": {"forest": {"color": "0x5B45"}},
                         "aeroway": {"runway": {"color": "0x0000"}}}}"#,
    );
    let err = generate(u.path(), &opts()).expect_err("a skin with an extra style must fail");
    assert!(err.contains("aeroway.runway") && err.contains("new schema revision"), "{err}");

    // Same feature types, different document order — which renumbers the ids every
    // baked chunk already references.
    let v = TempTree::new("renumbered-skin");
    example_tree(v.path());
    write(
        &v.path().join(SKINS_DIR).join("contrast.json"),
        r#"{"_meta": {"id": "contrast", "name": "High contrast", "description": "Fewer greys.", "version": 1},
            "features": {"landuse": {"forest": {"color": "0x5B45"}},
                         "highway": {"primary": {"color": "0xF800"}, "track": {"color": "0xAA80"}},
                         "natural": {"water": {"color": "0x55DF"}}}}"#,
    );
    let err = generate(v.path(), &opts()).expect_err("a reordered skin must fail");
    assert!(err.contains("MUST NOT renumber"), "{err}");
}

#[test]
fn band_table_violations_fail() {
    let cases: [(&str, &str); 7] = [
        // A LOD in the core band: unsplittable bytes in the file whose headroom is the
        // design's hard limit.
        (
            r#"{ "id": "coarse", "cell_log2": 20, "lods": [0], "role": "coarse" },
              { "id": "mid", "cell_log2": 19, "lods": [1], "role": "geometry" },
              { "id": "network", "cell_log2": 18, "lods": [2], "sections": ["nav", "poi"], "role": "core" }"#,
            "cannot be split by bbox",
        ),
        // A LOD in two bands: written twice.
        (
            r#"{ "id": "coarse", "cell_log2": 20, "lods": [0, 1], "role": "coarse" },
              { "id": "mid", "cell_log2": 19, "lods": [1], "role": "geometry" },
              { "id": "fine", "cell_log2": 18, "lods": [2], "role": "geometry" },
              { "id": "network", "cell_log2": 18, "sections": ["nav", "poi"], "role": "core" }"#,
            "would be written twice",
        ),
        // A LOD in no band: blank at that zoom.
        (
            r#"{ "id": "coarse", "cell_log2": 20, "lods": [0], "role": "coarse" },
              { "id": "fine", "cell_log2": 18, "lods": [2], "role": "geometry" },
              { "id": "network", "cell_log2": 18, "sections": ["nav", "poi"], "role": "core" }"#,
            "are in no band",
        ),
        // No core: nowhere for the nav graph and the POIs to go.
        (
            r#"{ "id": "coarse", "cell_log2": 20, "lods": [0], "role": "coarse" },
              { "id": "mid", "cell_log2": 19, "lods": [1], "role": "geometry" },
              { "id": "fine", "cell_log2": 18, "lods": [2], "sections": ["nav", "poi"], "role": "geometry" }"#,
            "only the `core` band may carry a section",
        ),
        // Two coarse bands: two whole-assembly shards.
        (
            r#"{ "id": "coarse", "cell_log2": 20, "lods": [0], "role": "coarse" },
              { "id": "mid", "cell_log2": 19, "lods": [1], "role": "coarse" },
              { "id": "fine", "cell_log2": 18, "lods": [2], "role": "geometry" },
              { "id": "network", "cell_log2": 18, "sections": ["nav", "poi"], "role": "core" }"#,
            "at most one band may have `role: coarse`",
        ),
        // A core band with only one of the two sections.
        (
            r#"{ "id": "coarse", "cell_log2": 20, "lods": [0], "role": "coarse" },
              { "id": "mid", "cell_log2": 19, "lods": [1], "role": "geometry" },
              { "id": "fine", "cell_log2": 18, "lods": [2], "role": "geometry" },
              { "id": "network", "cell_log2": 18, "sections": ["nav"], "role": "core" }"#,
            "must carry both the `nav` and `poi` sections",
        ),
        // A cell size outside the permitted range.
        (
            r#"{ "id": "coarse", "cell_log2": 29, "lods": [0], "role": "coarse" },
              { "id": "mid", "cell_log2": 19, "lods": [1], "role": "geometry" },
              { "id": "fine", "cell_log2": 18, "lods": [2], "role": "geometry" },
              { "id": "network", "cell_log2": 18, "sections": ["nav", "poi"], "role": "core" }"#,
            "outside 2^10",
        ),
    ];
    for (bands, want) in cases {
        let t = TempTree::new("bands");
        example_tree(t.path());
        let doc = schema_doc(EXAMPLE_REVISION).replace(BANDS, &format!("\"bands\": [\n      {bands}\n    ]"));
        assert_ne!(doc, schema_doc(EXAMPLE_REVISION), "the band table must actually have been replaced");
        write(&t.path().join(SCHEMA_DOC), &doc);
        let err = generate(t.path(), &opts()).expect_err(&format!("must fail: {want}"));
        assert!(err.contains(want), "wanted `{want}`, got `{err}`");
    }
}

#[test]
fn a_broken_ladder_fails() {
    for (lods, want) in [
        (r#"{ "max_mpp": 30 }, { "max_mpp": 16 }, { "max_mpp": 3 }"#, "the coarsest level is +inf"),
        (r#"{ "max_mpp": null }, { "max_mpp": 16 }, { "max_mpp": 16 }"#, "strictly decrease"),
    ] {
        let t = TempTree::new("ladder");
        example_tree(t.path());
        let doc = schema_doc(EXAMPLE_REVISION).replace(
            r#"{ "max_mpp": null, "simplify": 200, "min_area_px": 50 },
    { "max_mpp": 16, "simplify": 40, "min_area_px": 30 },
    { "max_mpp": 3, "simplify": 3 }"#,
            lods,
        );
        write(&t.path().join(SCHEMA_DOC), &doc);
        let err = generate(t.path(), &opts()).expect_err(&format!("must fail: {want}"));
        assert!(err.contains(want), "wanted `{want}`, got `{err}`");
    }
}

#[test]
fn a_tree_missing_a_required_part_fails() {
    let t = TempTree::new("no-schema");
    example_tree(t.path());
    fs::remove_file(t.path().join(SCHEMA_DOC)).unwrap();
    assert!(generate(t.path(), &opts()).unwrap_err().contains("schema.json"));

    let u = TempTree::new("no-skins");
    example_tree(u.path());
    fs::remove_dir_all(u.path().join(SKINS_DIR)).unwrap();
    assert!(generate(u.path(), &opts()).unwrap_err().contains("at least one skin"));

    let v = TempTree::new("no-cells");
    example_tree(v.path());
    fs::remove_dir_all(v.path().join(CELLS_DIR)).unwrap();
    assert!(generate(v.path(), &opts()).unwrap_err().contains("no `cells/` directory"));

    let w = TempTree::new("empty-cells");
    example_tree(w.path());
    fs::remove_dir_all(w.path().join(CELLS_DIR)).unwrap();
    fs::create_dir_all(w.path().join(CELLS_DIR)).unwrap();
    assert!(generate(w.path(), &opts()).unwrap_err().contains("empty cell store"));

    let x = TempTree::new("no-poly");
    example_tree(x.path());
    fs::remove_file(x.path().join(REGIONS_DIR).join("europe/switzerland").join(REGION_POLY)).unwrap();
    assert!(generate(x.path(), &opts()).unwrap_err().contains("geofabrik"), "the error must name the source");
}

#[test]
fn a_missing_or_orphaned_cell_sidecar_fails() {
    let t = TempTree::new("no-sidecar");
    example_tree(t.path());
    fs::remove_file(cell_path(t.path(), "fine", fine_west(), CELL_SIDECAR_EXT)).unwrap();
    assert!(generate(t.path(), &opts()).unwrap_err().contains("sidecar"));

    let u = TempTree::new("orphan-sidecar");
    example_tree(u.path());
    fs::remove_file(cell_path(u.path(), "fine", fine_west(), CELL_EXT)).unwrap();
    assert!(generate(u.path(), &opts()).unwrap_err().contains("no cell"));
}

#[test]
fn known_empty_ranges_are_canonical_and_never_overlap_artifacts() {
    let sources = vec![CellSource { extract_id: "planet".into(), snapshot: "2026-07-19".into() }];

    let t = TempTree::new("empty-overlap");
    example_tree(t.path());
    write_known_empty(
        t.path(),
        "fine",
        vec![KnownEmptyRun {
            start: fine_east().to_string(),
            end: fine_empty().to_string(),
            built_at: "2026-07-30T02:13:11Z".into(),
            sources: sources.clone(),
        }],
    );
    let err = generate(t.path(), &opts()).expect_err("a range may not cover an artifact");
    assert!(err.contains("both an OBCM artifact and known empty"), "{err}");

    let u = TempTree::new("empty-cross-row");
    example_tree(u.path());
    write_known_empty(
        u.path(),
        "fine",
        vec![KnownEmptyRun {
            start: fine_empty().to_string(),
            end: cell(18, fine_empty().i + 1, fine_empty().j).to_string(),
            built_at: "2026-07-30T02:13:11Z".into(),
            sources: sources.clone(),
        }],
    );
    let err = generate(u.path(), &opts()).expect_err("a run stays on one row");
    assert!(err.contains("one non-empty inclusive row range"), "{err}");

    let v = TempTree::new("empty-unmerged");
    example_tree(v.path());
    let next = cell(18, fine_empty().i, fine_empty().j + 1);
    write_known_empty(
        v.path(),
        "fine",
        vec![
            KnownEmptyRun {
                start: fine_empty().to_string(),
                end: fine_empty().to_string(),
                built_at: "2026-07-30T02:13:11Z".into(),
                sources: sources.clone(),
            },
            KnownEmptyRun {
                start: next.to_string(),
                end: next.to_string(),
                built_at: "2026-07-30T02:13:11Z".into(),
                sources,
            },
        ],
    );
    let err = generate(v.path(), &opts()).expect_err("identical adjacent provenance has one canonical run");
    assert!(err.contains("merge them"), "{err}");
}

#[test]
fn a_cell_sidecar_typo_fails_rather_than_defaulting() {
    let t = TempTree::new("sidecar-typo");
    example_tree(t.path());
    write(
        &cell_path(t.path(), "fine", fine_west(), CELL_SIDECAR_EXT),
        r#"{"schema_revision": 7, "builtAt": "2026-07-30T02:12:55Z",
            "sources": [{"extract_id": "europe/switzerland", "snapshot": "2026-07-19"}], "partial": false}"#,
    );
    let err = generate(t.path(), &opts()).expect_err("an unknown sidecar key must fail");
    assert!(err.contains("builtAt") || err.contains("unknown field"), "{err}");

    // And the facts inside it are validated, not merely deserialized.
    for (body, want) in [
        (
            r#"{"schema_revision": 7, "built_at": "2026-07-30T02:12:55+02:00",
                "sources": [{"extract_id": "europe/switzerland", "snapshot": "2026-07-19"}], "partial": false}"#,
            "built_at",
        ),
        (
            r#"{"schema_revision": 7, "built_at": "2026-07-30T02:12:55Z",
                "sources": [{"extract_id": "europe/switzerland", "snapshot": "2026-02-30"}], "partial": false}"#,
            "snapshot",
        ),
        (
            r#"{"schema_revision": 7, "built_at": "2026-07-30T02:12:55Z", "sources": [], "partial": false}"#,
            "`sources` is empty",
        ),
        (
            r#"{"schema_revision": 7, "built_at": "2026-07-30T02:12:55Z",
                "sources": [{"extract_id": "Europe/Switzerland", "snapshot": "2026-07-19"}], "partial": false}"#,
            "kebab-case",
        ),
    ] {
        let u = TempTree::new("sidecar-facts");
        example_tree(u.path());
        write(&cell_path(u.path(), "fine", fine_west(), CELL_SIDECAR_EXT), body);
        let err = generate(u.path(), &opts()).expect_err(&format!("must fail: {want}"));
        assert!(err.contains(want), "wanted `{want}`, got `{err}`");
    }
}

#[test]
fn stray_files_and_unknown_bands_fail() {
    let t = TempTree::new("stray-cell");
    example_tree(t.path());
    write(&cell_dir(t.path(), "fine", fine_west()).join("README.txt"), "notes");
    assert!(generate(t.path(), &opts()).unwrap_err().contains("unexpected entry in a cell row"));

    let u = TempTree::new("unknown-band");
    example_tree(u.path());
    write_cell(
        u.path(),
        "ultrafine",
        cell(18, 1204, 1052),
        64,
        "2026-07-30T02:12:55Z",
        &[("europe/switzerland", "2026-07-19")],
        false,
    );
    assert!(generate(u.path(), &opts()).unwrap_err().contains("is not a band"));

    let v = TempTree::new("stray-region");
    example_tree(v.path());
    write(&v.path().join(REGIONS_DIR).join("europe/switzerland/NOTES.md"), "notes");
    assert!(generate(v.path(), &opts()).unwrap_err().contains("unexpected entry in a region tree"));

    let w = TempTree::new("unknown-region-band");
    example_tree(w.path());
    write_region(
        w.path(),
        "europe/switzerland",
        "Switzerland",
        &[("ultrafine", vec![fine_west()])],
        (5.9, 10.5),
        (45.8, 47.8),
    );
    assert!(generate(w.path(), &opts()).unwrap_err().contains("not in `schema.json`'s band table"));
}

/// Dotfiles are the one thing skipped silently — macOS sprinkles `.DS_Store` through
/// any directory a Finder window has visited.
#[test]
fn dotfiles_are_ignored() {
    let t = TempTree::new("dotfiles");
    example_tree(t.path());
    let clean = root_json(&generated(t.path()).root);
    write(&t.path().join(CELLS_DIR).join("fine/.DS_Store"), "junk");
    write(&cell_dir(t.path(), "fine", fine_west()).join(".DS_Store"), "junk");
    write(&t.path().join(SKINS_DIR).join(".gitkeep"), "");
    write(&t.path().join(REGIONS_DIR).join("europe/.DS_Store"), "junk");
    assert_eq!(root_json(&generated(t.path()).root), clean);
}

#[test]
fn a_tree_with_no_regions_still_publishes_its_cells() {
    let t = TempTree::new("no-regions");
    example_tree(t.path());
    fs::remove_dir_all(t.path().join(REGIONS_DIR)).unwrap();
    let g = generated(t.path());
    assert!(g.root.regions.is_empty());
    assert!(g.warnings.iter().any(|w| w.contains("no regions")), "{:?}", g.warnings);
    assert_eq!(g.root.cell_index.len(), 4, "the cell store is still publishable");
}

#[test]
fn a_band_with_no_cells_is_reported() {
    let t = TempTree::new("empty-band");
    example_tree(t.path());
    fs::remove_dir_all(t.path().join(CELLS_DIR).join("mid")).unwrap();
    write_region(
        t.path(),
        "europe/switzerland",
        "Switzerland",
        &[
            ("coarse", vec![coarse_cell()]),
            ("fine", vec![fine_west(), fine_east()]),
            ("network", vec![fine_west(), fine_east()]),
        ],
        (5.9, 10.5),
        (45.8, 47.8),
    );
    write_region(
        t.path(),
        "europe/switzerland/basel-stadt",
        "Basel-Stadt",
        &[("coarse", vec![coarse_cell()]), ("fine", vec![fine_west()]), ("network", vec![fine_west()])],
        (7.5, 7.7),
        (47.5, 47.6),
    );
    let g = generated(t.path());
    assert!(
        g.warnings.iter().any(|w| w.contains("band `mid` has no published or known-empty cells")),
        "{:?}",
        g.warnings
    );
    // The band still gets an index — one entry per band in the schema (§11.6) — so a
    // consumer's band loop does not have to handle a missing document.
    assert_eq!(g.root.cell_index.iter().find(|c| c.band == "mid").expect("mid").cell_count, 0);
}

#[test]
fn base_urls_are_checked_before_a_tree_is_walked() {
    let t = TempTree::new("base-url");
    example_tree(t.path());
    let mut o = opts();
    o.base_url = "maps.example.org".into();
    assert!(generate(t.path(), &o).unwrap_err().contains("must be absolute"));
    let mut o = opts();
    o.generated_at = "2026-07-30T09:00:00+02:00".into();
    assert!(generate(t.path(), &o).unwrap_err().contains("generated_at"));
}

/// JSON is self-delimiting, so no proper prefix of a valid document parses — the
/// format half of "never partially consumed", per document rather than per catalog.
#[test]
fn every_truncation_of_a_document_fails_to_parse() {
    let t = TempTree::new("truncate");
    example_tree(t.path());
    let g = generated(t.path());
    let root = root_json(&g.root);
    assert!(serde_json::from_str::<Catalog>(&root).is_ok());
    for cut in (1..root.len()).step_by(23) {
        assert!(serde_json::from_str::<Catalog>(&root[..cut]).is_err(), "root truncated at {cut} must not parse");
    }
    let index = &satellite(&g, "cells/fine/index.json").body;
    for cut in (1..index.len()).step_by(13) {
        assert!(
            serde_json::from_str::<CellIndexDocument>(&index[..cut]).is_err(),
            "a cell index truncated at {cut} must not parse"
        );
    }
}

// --- schema + examples ---------------------------------------------------------------------

#[test]
fn checked_in_catalog_schema_is_the_current_generated_schema() {
    let checked_in: Value = serde_json::from_str(CATALOG_SCHEMA_JSON).expect("checked-in catalog schema is valid JSON");
    assert_eq!(
        checked_in,
        catalog_schema(),
        "schema/catalog.schema.json is stale; regenerate with `cargo run -p obc-pack --bin obc-pack -- schema \
         --catalog > host/obc-pack/schema/catalog.schema.json`"
    );
}

#[test]
fn the_catalog_schema_pins_the_envelope_version_and_the_field_patterns() {
    let s = catalog_schema();
    assert_eq!(s["properties"]["schema_version"]["const"].as_u64(), Some(u64::from(CATALOG_SCHEMA_VERSION)));
    assert_eq!(s["$defs"]["CellEntry"]["properties"]["id"]["pattern"].as_str(), Some(CELL_ID_PATTERN));
    assert_eq!(s["$defs"]["CellEntry"]["properties"]["sha256"]["pattern"].as_str(), Some(SHA256_PATTERN));
    assert_eq!(s["$defs"]["KnownEmptyRun"]["properties"]["start"]["pattern"].as_str(), Some(CELL_ID_PATTERN));
    assert_eq!(s["$defs"]["KnownEmptyRun"]["properties"]["end"]["pattern"].as_str(), Some(CELL_ID_PATTERN));
    assert_eq!(s["$defs"]["KnownEmptyRun"]["properties"]["built_at"]["pattern"].as_str(), Some(TIMESTAMP_PATTERN));
    assert!(
        s["$defs"]["CellEntry"]["properties"].get("bbox").is_none(),
        "§11.6: a cell entry has no bbox — the id determines the square"
    );
    assert_eq!(
        s["$defs"]["GridEntry"]["properties"]["origin_udeg"]["const"].as_i64(),
        Some(i64::from(GRID_ORIGIN_UDEG))
    );
    assert_eq!(s["$defs"]["SkinStyle"]["properties"]["priority"]["maximum"].as_u64(), Some(4));
    assert_eq!(s["$defs"]["SkinPreview"]["properties"]["url"]["pattern"].as_str(), Some(PINNED_URL_PATTERN));
    for (definition, field) in [("RegionEntry", "cells_url"), ("CellIndexRef", "url"), ("CellEntry", "url")] {
        assert_eq!(
            s["$defs"][definition]["properties"][field]["pattern"].as_str(),
            Some(PINNED_URL_PATTERN),
            "{definition}.{field}"
        );
    }
    let skin_required = s["$defs"]["SkinEntry"]["required"].as_array().expect("skin required");
    assert!(!skin_required.iter().any(|v| v == "preview"), "preview remains optional");
    // Both satellites are in the one checked-in file, so a consumer validates all
    // three documents against a single resource.
    for doc in ["CellIndexDocument", "RegionCellsDocument"] {
        assert_eq!(s["$defs"][doc]["properties"]["schema_version"]["const"].as_u64(), Some(2), "{doc}");
    }
    // `parent` is optional.
    let region_required = s["$defs"]["RegionEntry"]["required"].as_array().expect("region required");
    assert!(!region_required.iter().any(|v| v == "parent"));
    for field in ["boundary", "bytes", "bytes_by_band", "cell_count", "partial_cell_count_by_band", "cells_url"] {
        assert!(region_required.iter().any(|v| v == field), "`{field}` is required on a region");
    }
    let partial_by_band = &s["$defs"]["RegionEntry"]["properties"]["partial_cell_count_by_band"];
    assert_eq!(partial_by_band["propertyNames"]["pattern"].as_str(), Some(ID_PATTERN));
    for field in ["known_empty_count", "url"] {
        let required = s["$defs"]["CellIndexRef"]["required"].as_array().expect("cell ref required");
        assert!(required.iter().any(|v| v == field), "`{field}` is required on a cell index ref");
    }
    let index_required = s["$defs"]["CellIndexDocument"]["required"].as_array().expect("cell index required");
    assert!(index_required.iter().any(|v| v == "known_empty"));
}

/// The documents a real tree produces must validate against the schema consumers will
/// validate with — root *and* both satellites.
#[test]
fn generated_documents_validate_against_the_checked_in_schema() {
    let schema: Value = serde_json::from_str(CATALOG_SCHEMA_JSON).expect("schema JSON");
    let mut schemas = boon::Schemas::new();
    let mut compiler = boon::Compiler::new();
    compiler.add_resource("catalog.schema.json", schema).expect("add schema");
    let root_id = compiler.compile("catalog.schema.json", &mut schemas).expect("compile root");
    let cell_id =
        compiler.compile("catalog.schema.json#/$defs/CellIndexDocument", &mut schemas).expect("compile cell index");
    let region_id =
        compiler.compile("catalog.schema.json#/$defs/RegionCellsDocument", &mut schemas).expect("compile region cells");
    let terrain_id = compiler
        .compile("catalog.schema.json#/$defs/TerrainIndexDocument", &mut schemas)
        .expect("compile terrain index");

    let t = TempTree::new("validate");
    example_tree(t.path());
    let g = generated(t.path());
    let instance: Value = serde_json::from_str(&root_json(&g.root)).unwrap();
    if let Err(e) = schemas.validate(&instance, root_id) {
        panic!("generated root does not validate:\n{e:#}");
    }
    for s in &g.satellites {
        let doc: Value = serde_json::from_str(&s.body).unwrap();
        let sid = match () {
            _ if s.rel_path.starts_with(&format!("{CELLS_DIR}/{TERRAIN_DIR}/")) => terrain_id,
            _ if s.rel_path.starts_with("cells/") => cell_id,
            _ => region_id,
        };
        if let Err(e) = schemas.validate(&doc, sid) {
            panic!("{} does not validate:\n{e:#}", s.rel_path);
        }
    }
    // The checked-in examples are the artifacts a consumer is built against.
    for (example, sid) in [
        (CATALOG_EXAMPLE_JSON, root_id),
        (CELL_INDEX_EXAMPLE_JSON, cell_id),
        (REGION_CELLS_EXAMPLE_JSON, region_id),
        (TERRAIN_INDEX_EXAMPLE_JSON, terrain_id),
    ] {
        let doc: Value = serde_json::from_str(example).expect("example is valid JSON");
        if let Err(e) = schemas.validate(&doc, sid) {
            panic!("a checked-in example does not validate:\n{e:#}");
        }
    }

    // And the schema has teeth.
    let mut broken = instance.clone();
    broken["regions"][0]["cells_sha256"] = Value::from("NOTAHASH");
    assert!(schemas.validate(&broken, root_id).is_err(), "the sha256 pattern must reject a non-hex digest");
    let mut unsupported = instance.clone();
    unsupported["schema_version"] = Value::from(1);
    assert!(schemas.validate(&unsupported, root_id).is_err(), "an unsupported envelope must not validate");
    let mut bad_cell = serde_json::from_str::<Value>(&satellite(&g, "cells/fine/index.json").body).unwrap();
    bad_cell["cells"][0]["id"] = Value::from("18/204/1052");
    assert!(schemas.validate(&bad_cell, cell_id).is_err(), "the cell-id pattern must reject truncated padding");
}

/// The other half of the stale-generation guard: the checked-in examples must have
/// been regenerated through the real generator, so nothing in them can drift from the
/// producer — an OBCM bump included.
#[test]
fn catalog_examples_are_current() {
    let t = TempTree::new("example");
    example_tree(t.path());
    let g = generated(t.path());
    let files = [
        ("catalog.example.json", root_json(&g.root), CATALOG_EXAMPLE_JSON),
        ("cell-index.example.json", satellite(&g, "cells/fine/index.json").body.clone(), CELL_INDEX_EXAMPLE_JSON),
        (
            "region-cells.example.json",
            satellite(&g, "regions/europe/switzerland/cells.json").body.clone(),
            REGION_CELLS_EXAMPLE_JSON,
        ),
        (
            "terrain-index.example.json",
            satellite(&g, "cells/terrain/index.json").body.clone(),
            TERRAIN_INDEX_EXAMPLE_JSON,
        ),
    ];
    // Regeneration is a deliberate act, so it needs a deliberate switch — the same one
    // the checked-in fixture guard uses:
    //   OBC_UPDATE_CATALOG_EXAMPLE=1 cargo test -p obc-pack catalog_examples_are_current
    if std::env::var_os("OBC_UPDATE_CATALOG_EXAMPLE").is_some() {
        for (name, regenerated, _) in &files {
            fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("schema").join(name), regenerated)
                .expect("update the checked-in example");
        }
    }
    for (name, regenerated, checked_in) in &files {
        assert_eq!(
            *checked_in, *regenerated,
            "schema/{name} is stale — regenerate with `OBC_UPDATE_CATALOG_EXAMPLE=1 cargo test -p obc-pack \
             catalog_examples_are_current`. (It is generated from the synthetic tree in this module's `example_tree`, so \
             only a manifest-shape change, an OBCM version bump, or a fixture change should move it.)"
        );
    }

    let parsed: Catalog = serde_json::from_str(CATALOG_EXAMPLE_JSON).expect("the example parses");
    assert_eq!(parsed.schema_version, CATALOG_SCHEMA_VERSION);
    assert_eq!(
        parsed.schema.obcm_version, OBCM_VERSION,
        "the example must report the OBCM version this packer writes (v{OBCM_VERSION})"
    );
}

// --- the shipped schema and skins -----------------------------------------------------------

/// The shipped schema (`builder/presets/schema.json`) or, with a `../` path, one of
/// the retired preset documents kept as a fixture — with a `_meta.revision` and band
/// table injected, so the real file can be used as this producer's `schema.json`.
fn as_schema(rel: &str, bands: &str, revision: u32) -> String {
    let text = repo_doc(rel);
    let mut doc: serde_json::Value = serde_json::from_str(&text).expect("the document is valid JSON");
    let meta = doc.get_mut("_meta").expect("_meta").as_object_mut().expect("object");
    meta.insert("revision".into(), Value::from(revision));
    meta.insert("bands".into(), serde_json::from_str(bands).expect("band table"));
    serde_json::to_string_pretty(&doc).expect("serializes")
}

/// A checked-in document, read relative to this crate.
fn repo_doc(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The shipped schema, as this producer's `schema.json`.
const SHIPPED_SCHEMA: &str = "../../builder/presets/schema.json";
/// The schema's own look, restated as a skin.
const SHIPPED_SKIN: &str = "../../builder/presets/skins/default.json";
/// The night restyle (epic #1016 P5) — the first skin that legitimately differs.
const SHIPPED_DUSK_SKIN: &str = "../../builder/presets/skins/dusk.json";
/// `OBCA_Spec.md` §1.5's recommended band table, against the shipped bikepacking ladder: LOD
/// 0–2 coarse, 3–4 mid, 5–6 fine, nav + POI in `network`.
const RECOMMENDED_BAND_TABLE: &str = r#"[
    { "id": "coarse", "cell_log2": 20, "lods": [0, 1, 2], "role": "coarse" },
    { "id": "mid", "cell_log2": 19, "lods": [3, 4], "role": "geometry" },
    { "id": "fine", "cell_log2": 18, "lods": [5, 6], "role": "geometry" },
    { "id": "network", "cell_log2": 18, "sections": ["nav", "poi"], "role": "core" }
]"#;

/// The shipped documents are what a real bake hands this generator: `schema.json` is
/// the hosted schema, `skins/default.json` is a skin over it, and the band table of
/// `OBCA_Spec.md` §1.5 partitions the schema's ladder exactly. If any of that stops
/// being true, the first real bake fails on data we control.
#[test]
fn the_shipped_schema_and_skin_generate_the_current_catalog() {
    let t = TempTree::new("shipped");
    write(&t.path().join(SCHEMA_DOC), &as_schema(SHIPPED_SCHEMA, RECOMMENDED_BAND_TABLE, 1));
    write(&t.path().join(SKINS_DIR).join("default.json"), &repo_doc(SHIPPED_SKIN));
    write_terrain(t.path(), TERRAIN_REVISION, TERRAIN_DATASET_VERSION);
    let ch = [("europe/switzerland", "2026-07-19")];
    for (band, id) in [("coarse", coarse_cell()), ("mid", mid_cell()), ("fine", fine_west())] {
        write_cell_at(t.path(), band, id, OBCM_VERSION, id.square(), 16, 1, "2026-07-30T02:10:04Z", &ch, false);
    }
    write_cell_at(
        t.path(),
        "network",
        fine_west(),
        OBCM_VERSION,
        fine_west().square(),
        16,
        1,
        "2026-07-30T02:10:04Z",
        &ch,
        false,
    );

    let g = generate(t.path(), &opts()).expect("the shipped documents generate a catalog");
    assert_eq!(g.root.schema.id, "bikepacking", "the shipped schema names itself");
    assert_eq!(g.root.schema.lods.len(), 7, "the shipped ladder is 7 rungs");
    assert_eq!(
        g.root.schema.lods.iter().map(|l| l.band.as_str()).collect::<Vec<_>>(),
        ["coarse", "coarse", "coarse", "mid", "mid", "fine", "fine"],
        "OBCA_Spec.md §1.5's band table partitions the shipped ladder"
    );
    let skin = &g.root.skins[0];
    assert_eq!(skin.id, "default");
    assert_eq!(skin.styles.len(), g.root.schema.styles.len());
    assert!(skin.styles.len() > 30, "the shipped schema carries a real style table: {}", skin.styles.len());
    assert!(skin.styles.iter().any(|s| s.dashed), "and at least one dashed style");

    // The shipped skin is the schema's *own* look, restated. Nothing else in the tree
    // enforces that — a skin is free to differ, which is the entire point of skins —
    // but `default` is the checked-in baseline look, so the two drifting apart
    // would make its name misleading.
    // The `dusk` skin (next test) is where a skin legitimately differs.
    let schema_config = Config::parse(&repo_doc(SHIPPED_SCHEMA)).expect("the schema parses");
    let skin_config = Config::parse(&repo_doc(SHIPPED_SKIN)).expect("the skin parses");
    check_skin(&schema_config, &skin_config).expect("the shipped skin fits the shipped schema");
    assert_eq!(skin.marker_color, schema_config.marker_color, "same marker color");
    let schema_styles =
        skin_styles(&schema_config, &read_schema_doc(&t.path().join(SCHEMA_DOC)).expect("schema"), Path::new("schema"))
            .expect("the schema's own values, in skin shape");
    assert_eq!(skin.styles, schema_styles, "the `default` skin restates the schema's own presentation values");

    // And the swatch with them. It is `_meta`, so nothing above reaches it — but it is
    // the six colours the builder paints a style card with, i.e. the *only* part of
    // either document a user ever sees before downloading a map. A skin whose styles
    // match the schema while its swatch advertises something else is a card that lies,
    // and the two documents restating the same values by hand is exactly the setup in
    // which one of them gets edited alone.
    let swatch = |doc: &str| -> Vec<String> {
        let v: Value = serde_json::from_str(&repo_doc(doc)).expect("valid JSON");
        v["_meta"]["swatch"]
            .as_array()
            .unwrap_or_else(|| panic!("{doc}: `_meta.swatch` is what the builder paints a card with"))
            .iter()
            .map(|c| c.as_str().expect("a swatch entry is a hex string").to_string())
            .collect()
    };
    let schema_swatch = swatch(SHIPPED_SCHEMA);
    assert_eq!(schema_swatch.len(), 6, "six colours, as every card expects");
    assert_eq!(
        swatch(SHIPPED_SKIN),
        schema_swatch,
        "the `default` skin's swatch must be the schema's — the card is the only part of these documents a rider \
         sees before downloading"
    );
}

/// The second shipped skin (epic #1016 P5): `dusk`, the night restyle — the skin that
/// legitimately differs from the schema, which is the entire point of skins. Three
/// properties keep it honest, and each has a way to rot silently without a test:
///
/// 1. **It generates beside `default`** — same feature types, same ids, presentation
///    only. (An edit that drifts it into schema territory should fail here, on data we
///    ship, not on the first real bake.)
/// 2. **It respects the bake's merges.** `merge_fills`/`merge_lines` union features
///    whose *schema* styles render identically and retag the result to one canonical
///    style id (`merge.rs`), so a skin giving two schema-merged feature types
///    different values would style only the canonical id's share of the merged
///    geometry — the other name's entry would be dead weight that looks like a
///    design decision. Every group the schema merges must be restated uniformly.
///    The groups are derived from the `default` skin (proved above to restate the
///    schema's own values) by full render identity — the line-stitch key, which is
///    identical to the line-stitch key, and — for the current schema, where no fill
///    carries a weight or line style — a superset of the fill merge classes (verified
///    empirically: 7 test groups cover all 5 real fill classes). A schema that gives a
///    fill a weight/dash would open a gap here; widen the key to the union then.
/// 3. **It survives the panel.** The LS021B7DD02 shows 64 colors (RGB222 — the top
///    two bits of each channel, `OBCM_Spec.md` §2, `rgb565_to_device64`), so two
///    RGB565 values in one RGB222 bucket are one color on glass. Every *distinct*
///    RGB565 value in the document must land in its own bucket, the ground must
///    quantize dark, and the marker light — a dark-ground skin whose marker
///    quantizes into the ground is unusable at exactly the moment it exists for.
#[test]
fn the_shipped_dusk_skin_is_a_presentation_only_night_restyle() {
    let t = TempTree::new("shipped-dusk");
    write(&t.path().join(SCHEMA_DOC), &as_schema(SHIPPED_SCHEMA, RECOMMENDED_BAND_TABLE, 1));
    write(&t.path().join(SKINS_DIR).join("default.json"), &repo_doc(SHIPPED_SKIN));
    write(&t.path().join(SKINS_DIR).join("dusk.json"), &repo_doc(SHIPPED_DUSK_SKIN));
    write_terrain(t.path(), TERRAIN_REVISION, TERRAIN_DATASET_VERSION);
    let ch = [("europe/switzerland", "2026-07-19")];
    for (band, id) in [("coarse", coarse_cell()), ("mid", mid_cell()), ("fine", fine_west()), ("network", fine_west())]
    {
        write_cell_at(t.path(), band, id, OBCM_VERSION, id.square(), 16, 1, "2026-07-30T02:10:04Z", &ch, false);
    }
    let g = generate(t.path(), &opts()).expect("both shipped skins generate a catalog");
    assert_eq!(g.root.skins.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["default", "dusk"], "sorted by id");
    let [default, dusk] = &g.root.skins[..] else { unreachable!() };
    assert_eq!(dusk.styles.len(), default.styles.len(), "same feature types, schema id order");
    assert_ne!(dusk.styles, default.styles, "a real restyle, not a copy");
    assert_ne!(dusk.marker_color, default.marker_color, "red vanishes on a dark ground; dusk re-picks the marker");
    for (d, s) in dusk.styles.iter().zip(&default.styles) {
        assert_eq!(d.feature_type, s.feature_type);
        // The skin split lets a skin restate every presentation value in the record, but this
        // one deliberately recolors only: geometry-shaping stays visually identical to the
        // day map, so a rider switching skins at dusk sees the same map, re-lit. The #1095
        // flag bits are in the tuple for that reason — a dusk contour that lost `fixed_width`
        // would be a different *drawing*, not a different colour.
        assert_eq!(
            (d.weight, d.z_index, d.priority, d.dashed, d.fixed_width, d.terrain_layer),
            (s.weight, s.z_index, s.priority, s.dashed, s.fixed_width, s.terrain_layer),
            "{}",
            d.feature_type
        );
    }

    // (2) — uniform within every group the schema's own look merges.
    let mut groups: BTreeMap<_, Vec<&SkinStyle>> = BTreeMap::new();
    for s in &default.styles {
        groups.entry((s.z_index, s.color, s.weight, s.priority, s.dashed, s.color2)).or_default().push(s);
    }
    let dusk_by_type: BTreeMap<&str, &SkinStyle> = dusk.styles.iter().map(|s| (s.feature_type.as_str(), s)).collect();
    let mut merged_groups = 0;
    for members in groups.values().filter(|m| m.len() > 1) {
        merged_groups += 1;
        let first = dusk_by_type[members[0].feature_type.as_str()];
        for member in &members[1..] {
            let d = dusk_by_type[member.feature_type.as_str()];
            assert_eq!(
                (d.color, d.color2, d.weight, d.z_index, d.priority, d.dashed),
                (first.color, first.color2, first.weight, first.z_index, first.priority, first.dashed),
                "`{}` and `{}` render identically in the schema, so the bake may have merged their geometry under \
                 one style id — a skin must restate them identically or the distinction is a lie",
                members[0].feature_type,
                member.feature_type,
            );
        }
    }
    assert!(merged_groups >= 5, "the shipped schema really does merge: {merged_groups} groups");

    // (3) — the panel's quantization, through the renderer's OWN policy rather than a
    // restated copy: a change to `obc_reader`'s color pipeline must move this test with it.
    let bucket = |c: u16| obc_reader::rgb565_to_device64(c);
    let colors: BTreeSet<u16> = dusk
        .styles
        .iter()
        .flat_map(|s| [Some(s.color), s.color2].into_iter().flatten())
        .chain([dusk.marker_color])
        .collect();
    let buckets: BTreeSet<_> = colors.iter().map(|&c| bucket(c)).collect();
    assert_eq!(
        buckets.len(),
        colors.len(),
        "two of the skin's RGB565 values share an RGB222 bucket — one color on glass"
    );
    let land = dusk_by_type["natural.land"];
    assert_eq!(bucket(land.color), (0, 0, 0), "the dark ground the whole design stands on");
    assert_eq!(bucket(dusk.marker_color), (255, 255, 0), "and a marker that reads against it");
}

// --- the terrain artifact class (§13) ---------------------------------------------------------

fn terrain_index_doc(g: &GeneratedCatalog) -> TerrainIndexDocument {
    serde_json::from_str(&satellite(g, &format!("{CELLS_DIR}/{TERRAIN_DIR}/{CELL_INDEX_NAME}")).body)
        .expect("terrain index parses")
}

/// Simulate the one event OBCA principle 5 makes a whole-store cutover: a schema-revision bump.
fn bump_schema_revision(tree: &Path, to: u32) {
    write_schema(tree, to);
    let mut stack = vec![tree.join(CELLS_DIR)];
    while let Some(dir) = stack.pop() {
        for entry in sorted_entries(&dir).expect("readable") {
            if entry.is_dir() {
                stack.push(entry);
                continue;
            }
            let name = file_name(&entry).expect("named");
            if !name.ends_with(CELL_SIDECAR_EXT) {
                continue;
            }
            let mut doc: Value = serde_json::from_str(&fs::read_to_string(&entry).expect("sidecar")).expect("JSON");
            doc["schema_revision"] = Value::from(to);
            write(&entry, &format!("{}\n", serde_json::to_string_pretty(&doc).expect("serializes")));
        }
    }
    // The known-empty state is per schema revision too, so it moves with the store.
    let path = tree.join(CELLS_DIR).join("fine").join(KNOWN_EMPTY_STATE_NAME);
    let mut state: Value = serde_json::from_str(&fs::read_to_string(&path).expect("state")).expect("JSON");
    state["schema_revision"] = Value::from(to);
    write(&path, &format!("{}\n", serde_json::to_string_pretty(&state).expect("serializes")));
}

/// Simulate a terrain re-bake: the terrain store moves to a new revision, and the OBCM store — not
/// re-cut in the same breath — still records having sampled the old one.
fn bump_terrain_revision(tree: &Path, to: u32) {
    write(&tree.join(TERRAIN_DOC), &terrain_doc_json(to, TERRAIN_DATASET_VERSION));
    for (id, fill, built_at) in
        [(terrain_nw(), 0x33, "2026-09-01T04:00:00Z"), (terrain_ne(), 0x44, "2026-09-01T04:00:03Z")]
    {
        write_terrain_cell(tree, id, fill, built_at, to, TERRAIN_DATASET_VERSION);
    }
    let state = format!(
        "{{\n  \"terrain_revision\": {to},\n  \"known_empty\": [\n    {{\n      \"start\": \"{}\",\n      \"end\": \
         \"{}\",\n      \"built_at\": \"2026-09-01T04:00:07Z\"\n    }}\n  ]\n}}\n",
        terrain_sea(),
        terrain_sea()
    );
    write(&tree.join(CELLS_DIR).join(TERRAIN_DIR).join(KNOWN_EMPTY_STATE_NAME), &state);
}

/// Everything the root pins, by published path, split into (terrain, everything else).
///
/// The *published* path carries the object's digest, so "the set is unchanged" is literally "there
/// is nothing to re-upload" — which is the claim both independence pins make.
fn pinned(g: &GeneratedCatalog) -> (BTreeSet<String>, BTreeSet<String>) {
    let terrain_prefix = format!("{CELLS_DIR}/{TERRAIN_DIR}/");
    g.pinned_artifacts
        .iter()
        .map(|a| a.published_rel_path.clone())
        .chain(g.satellites.iter().map(|s| s.published_rel_path.clone()))
        .partition(|path| path.starts_with(&terrain_prefix))
}

#[test]
fn the_root_carries_a_terrain_block_with_its_own_revision() {
    let t = TempTree::new("terrain-root");
    example_tree(t.path());
    let g = generated(t.path());
    let terrain = g.root.terrain.as_ref().expect("the example publishes terrain");

    assert_eq!(terrain.dataset_id, "copernicus-glo-30");
    assert_eq!(terrain.dataset_version, TERRAIN_DATASET_VERSION);
    assert_eq!((terrain.posting_log2, terrain.cell_log2), (TERRAIN_POSTING_LOG2, TERRAIN_CELL_LOG2));
    assert_eq!(terrain.terrain_revision, TERRAIN_REVISION);
    assert_ne!(
        terrain.terrain_revision, g.root.schema.revision,
        "the two revisions are unrelated numbers, and the worked example says so"
    );
    // §13.5: the credit is data a consumer reads, not a string a builder hard-codes.
    assert!(terrain.attribution.contains("Copernicus"), "{}", terrain.attribution);
    assert!(terrain.attribution.contains("ESA"), "{}", terrain.attribution);

    // §13.1: one pinned index, digest-addressed like every other pinned object.
    let pin = &terrain.cell_index;
    assert_eq!((pin.cell_count, pin.known_empty_count), (2, 1));
    assert_eq!(pin.url, format!("https://maps.example.org/catalog/cells/terrain/index.{}.json", pin.sha256));
    let index = satellite(&g, "cells/terrain/index.json");
    assert_eq!((pin.bytes, pin.sha256.as_str()), (index.bytes, index.sha256.as_str()));

    // §13.4: the one coupling, recorded — and this example is consistent, so nothing is warned.
    assert_eq!(g.root.network_terrain_revision, Some(TERRAIN_REVISION));
    assert!(g.warnings.is_empty(), "{:?}", g.warnings);
}

#[test]
fn the_terrain_index_lists_cells_and_ocean_runs_and_carries_no_schema_revision() {
    let t = TempTree::new("terrain-index");
    example_tree(t.path());
    let g = generated(t.path());
    let doc = terrain_index_doc(&g);

    assert_eq!(doc.schema_version, CATALOG_SCHEMA_VERSION);
    assert_eq!(doc.terrain_revision, TERRAIN_REVISION);
    assert_eq!(doc.cells.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["13/38528/33664", "13/38528/33665"]);
    let nw = &doc.cells[0];
    // 32-byte header + a one-entry directory + one 512-byte tile: the smallest legal container.
    assert_eq!(nw.bytes, (obct::HEADER_LEN + obct::DIR_ENTRY_LEN + 512) as u64);
    assert_eq!(nw.url, format!("https://maps.example.org/catalog/cells/terrain/38528/33664.{}.obcd", nw.sha256));
    assert_eq!(nw.built_at, "2026-08-01T04:00:00Z");
    // The ocean square: canonical zero-byte coverage, with no object to fetch.
    assert_eq!(doc.known_empty.len(), 1);
    assert_eq!(doc.known_empty[0].start, terrain_sea().to_string());

    // The independence, asserted on the serialized document a consumer actually reads: nothing in
    // it names the OBCM store, so nothing in the OBCM store can invalidate it.
    let body = &satellite(&g, "cells/terrain/index.json").body;
    for forbidden in ["schema_revision", "obcm", "band"] {
        assert!(!body.contains(forbidden), "the terrain index must not name `{forbidden}`:\n{body}");
    }
}

#[test]
fn a_region_lists_and_prices_its_terrain_selection() {
    let t = TempTree::new("terrain-region");
    example_tree(t.path());
    let g = generated(t.path());

    let ch = region(&g, "europe/switzerland");
    let footprint = ch.terrain.expect("the region selects terrain");
    assert_eq!((footprint.cell_count, footprint.known_empty_count), (2, 1));
    let doc = terrain_index_doc(&g);
    assert_eq!(footprint.bytes, doc.cells.iter().map(|c| c.bytes).sum::<u64>());
    // Terrain is priced beside the map, never inside it: a rider may take one without the other,
    // and `bytes_by_band` is the volume set's per-file projection (OBCA_Spec.md §5.7).
    assert_eq!(ch.bytes, ch.bytes_by_band.values().sum::<u64>());
    assert!(footprint.bytes > 0 && ch.bytes < footprint.bytes + ch.bytes);

    let satellite_doc: RegionCellsDocument =
        serde_json::from_str(&satellite(&g, "regions/europe/switzerland/cells.json").body).expect("parses");
    assert_eq!(
        satellite_doc.terrain,
        [terrain_nw().to_string(), terrain_ne().to_string(), terrain_sea().to_string()],
        "sorted, and a known-empty square is part of the selection"
    );
    // A terrain id is not a band key: `cells` stays exactly the schema's bands.
    assert!(!satellite_doc.cells.contains_key(TERRAIN_DIR));

    let basel = region(&g, "europe/switzerland/basel-stadt");
    assert_eq!(basel.terrain.expect("selects terrain").cell_count, 1);
}

/// **Independence pin (a)**: a schema-revision bump is a complete OBCM cutover — and touches not
/// one terrain byte.
///
/// This is the property the whole issue exists for. Terrain derives from a DEM that changes on a
/// years cadence; if it joined OBCA principle 5's lockstep, every schema bump would re-publish
/// hundreds of MiB of identical raster.
#[test]
fn a_schema_revision_bump_republishes_no_terrain_object() {
    let t = TempTree::new("indep-schema");
    example_tree(t.path());
    let before = generated(t.path());
    let (terrain_before, obcm_before) = pinned(&before);

    bump_schema_revision(t.path(), EXAMPLE_REVISION + 1);
    let after = generated(t.path());
    let (terrain_after, obcm_after) = pinned(&after);

    assert_eq!(after.root.schema.revision, EXAMPLE_REVISION + 1, "the OBCM store really did move");
    assert_ne!(obcm_after, obcm_before, "…and its satellites really were re-published");
    assert_eq!(terrain_after, terrain_before, "not one terrain object may be re-published by a schema bump");
    assert_eq!(
        satellite(&after, "cells/terrain/index.json").body,
        satellite(&before, "cells/terrain/index.json").body,
        "the terrain index is byte-identical across a schema-revision bump"
    );
    assert_eq!(after.root.terrain, before.root.terrain, "and so is the root's terrain block");
    assert!(after.warnings.is_empty(), "{:?}", after.warnings);
}

/// **Independence pin (b)**: a terrain re-bake is a complete terrain cutover — and touches not one
/// OBCM byte. The reverse of (a), and it needs its own test because the two directions are
/// different code.
///
/// It also pins the one real coupling: the network band's cells recorded which terrain revision
/// their ascents were integrated from, so a terrain bump leaves them **stale** — said out loud,
/// naming both revisions, rather than quietly serving a router whose numbers disagree with the
/// raster the device draws.
#[test]
fn a_terrain_rebake_republishes_no_obcm_object_and_flags_the_network_band() {
    let t = TempTree::new("indep-terrain");
    example_tree(t.path());
    let before = generated(t.path());
    let (terrain_before, obcm_before) = pinned(&before);

    bump_terrain_revision(t.path(), TERRAIN_REVISION + 1);
    let after = generated(t.path());
    let (terrain_after, obcm_after) = pinned(&after);

    assert_eq!(after.root.terrain.as_ref().expect("terrain").terrain_revision, TERRAIN_REVISION + 1);
    assert_ne!(terrain_after, terrain_before, "the terrain store really did move");
    assert_eq!(obcm_after, obcm_before, "not one OBCM object may be re-published by a terrain re-bake");
    for band in ["coarse", "mid", "fine", "network"] {
        let rel = format!("cells/{band}/index.json");
        assert_eq!(
            satellite(&after, &rel).body,
            satellite(&before, &rel).body,
            "band `{band}`'s index is byte-identical across a terrain re-bake"
        );
    }
    assert_eq!(after.root.schema, before.root.schema, "and the schema entry is untouched");

    // The coupling, stated: the cells still record the previous revision.
    assert_eq!(after.root.network_terrain_revision, Some(TERRAIN_REVISION));
    let warning = after.warnings.iter().find(|w| w.contains("network band")).expect("a stale network band is loud");
    assert!(warning.contains(&format!("terrain revision {TERRAIN_REVISION},")), "{warning}");
    assert!(warning.contains(&format!("terrain revision {}", TERRAIN_REVISION + 1)), "{warning}");
    assert!(warning.contains("§13.4"), "the warning must name the rule: {warning}");
}

#[test]
fn a_mixed_terrain_store_is_refused() {
    // A cell from another terrain revision: the terrain track's own lockstep (§13.2).
    let t = TempTree::new("mixed-terrain");
    example_tree(t.path());
    write_terrain_cell(t.path(), terrain_ne(), 0x22, "2026-08-01T04:00:03Z", TERRAIN_REVISION - 1, "2021-1");
    let err = generate(t.path(), &opts()).expect_err("a mixed-revision terrain store must fail");
    assert!(err.contains(&format!("terrain revision {}", TERRAIN_REVISION - 1)), "{err}");
    assert!(err.contains("lockstep within its own track"), "{err}");

    // A cell from another dataset version: the other half of the same key.
    let u = TempTree::new("mixed-dataset");
    example_tree(u.path());
    write_terrain_cell(u.path(), terrain_ne(), 0x22, "2026-08-01T04:00:03Z", TERRAIN_REVISION, "2023-1");
    let err = generate(u.path(), &opts()).expect_err("a mixed-dataset terrain store must fail");
    assert!(err.contains("dataset version `2023-1`"), "{err}");

    // And a cell store that sampled two different rasters (§13.4).
    let v = TempTree::new("mixed-network-terrain");
    example_tree(v.path());
    let path = cell_path(v.path(), "network", fine_east(), CELL_SIDECAR_EXT);
    let mut doc: Value = serde_json::from_str(&fs::read_to_string(&path).expect("sidecar")).expect("JSON");
    doc["terrain_revision"] = Value::from(TERRAIN_REVISION + 5);
    write(&path, &format!("{}\n", serde_json::to_string_pretty(&doc).expect("serializes")));
    let err = generate(v.path(), &opts()).expect_err("a store that sampled two rasters must fail");
    assert!(err.contains("§13.4"), "{err}");
}

#[test]
fn a_terrain_container_must_be_the_one_by_one_cell_its_id_names() {
    // A container covering a different square than its path says.
    let t = TempTree::new("terrain-square");
    example_tree(t.path());
    let path = terrain_path(t.path(), terrain_nw(), TERRAIN_EXT);
    fs::write(&path, obct_bytes(TERRAIN_POSTING_LOG2, terrain_cell(38_529, 33_664), 0x11)).expect("cell");
    let err = generate(t.path(), &opts()).expect_err("a terrain cell must be its square");
    assert!(err.contains("disagrees with its id"), "{err}");

    // A shard (a wider rectangle) published as a cell.
    let u = TempTree::new("terrain-shard");
    example_tree(u.path());
    let path = terrain_path(u.path(), terrain_nw(), TERRAIN_EXT);
    let mut bytes = obct_bytes(TERRAIN_POSTING_LOG2, terrain_nw(), 0x11);
    bytes[obct::HDR_CELL_COLS..obct::HDR_CELL_COLS + 2].copy_from_slice(&2u16.to_le_bytes());
    fs::write(&path, bytes).expect("cell");
    let err = generate(u.path(), &opts()).expect_err("a shard is not a cell");
    assert!(err.contains("1 × 1"), "{err}");

    // A container at another pairing than the one the store declares.
    let v = TempTree::new("terrain-pairing");
    example_tree(v.path());
    let path = terrain_path(v.path(), terrain_nw(), TERRAIN_EXT);
    let mut bytes = obct_bytes(TERRAIN_POSTING_LOG2, terrain_nw(), 0x11);
    bytes[obct::HDR_POSTING_LOG2] = TERRAIN_POSTING_LOG2 - 1;
    fs::write(&path, bytes).expect("cell");
    let err = generate(v.path(), &opts()).expect_err("one lattice per terrain revision");
    assert!(err.contains("one lattice per terrain revision"), "{err}");
}

#[test]
fn terrain_is_reserved_and_a_terrainless_catalog_is_complete() {
    // `terrain` is not a band id, because `cells/terrain/` is the other artifact class.
    let t = TempTree::new("terrain-band");
    example_tree(t.path());
    let bands = BANDS.replace("\"fine\"", "\"terrain\"");
    write(&t.path().join(SCHEMA_DOC), &schema_doc(EXAMPLE_REVISION).replace(BANDS, &bands));
    let err = generate(t.path(), &opts()).expect_err("`terrain` is reserved");
    assert!(err.contains("reserved"), "{err}");

    // And a catalog with no terrain at all is complete: everything degrades to "no elevation
    // here", which is what every map had before epic #1068.
    let u = TempTree::new("terrainless");
    example_tree(u.path());
    fs::remove_file(u.path().join(TERRAIN_DOC)).expect("remove the declaration");
    fs::remove_dir_all(u.path().join(CELLS_DIR).join(TERRAIN_DIR)).expect("remove the cells");
    for id in ["europe/switzerland", "europe/switzerland/basel-stadt"] {
        let path = u.path().join(REGIONS_DIR).join(id).join(REGION_DOC);
        let mut doc: Value = serde_json::from_str(&fs::read_to_string(&path).expect("region")).expect("JSON");
        doc.as_object_mut().expect("object").remove("terrain");
        write(&path, &format!("{}\n", serde_json::to_string_pretty(&doc).expect("serializes")));
    }
    let g = generate(u.path(), &opts()).expect("a terrain-less catalog generates");
    assert_eq!(g.root.terrain, None);
    assert!(g.root.regions.iter().all(|r| r.terrain.is_none()));
    assert!(!root_json(&g.root).contains("\"terrain\""), "an absent artifact class writes no keys");
    // The cells still record having sampled one, and that is itself worth saying out loud.
    assert!(g.warnings.iter().any(|w| w.contains("publishes no terrain")), "{:?}", g.warnings);
}
