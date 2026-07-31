//! The cell bake's acceptance criteria (#1020), all of them offline.
//!
//! Nothing here touches the network: extracts and `.poly` files come from a
//! [`LocalExtracts`] root, and the cutter is driven over a **synthetic ingest** rather
//! than a PBF — the real `obc_pack::cut::cut_ingested`, so every cell these tests
//! inspect is a genuine OBCM file with a genuine header, but with a fixture that can
//! be placed exactly on the grid lines the assertions are about.
//!
//! The geography is chosen so the ownership rule has something to own. Two regions,
//! `west` and `east`, meet **inside** cell column `j = 1053`:
//!
//! ```text
//!   j:      1051     1052     1053     1054     1055
//!        ┌────────┬────────┬────┬───┬────────┬────────┐
//!  west  │░░░░░░░░│████████│████│   │        │        │   ░ touched, not covered
//!  east  │        │        │    │███│████████│░░░░░░░░│   █ fully covered
//!        └────────┴────────┴────┴───┴────────┴────────┘
//!                                ↑ the co-baked seam
//! ```
//!
//! so `18/1204/1052` is canonical from `west` alone, `18/1204/1054` from `east` alone,
//! and `18/1204/1053` **only when both are baked together** — which is exactly the D3
//! property the whole plan-grouping machinery exists to produce.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use obc_bake::cells::{CellBakeOptions, CellBakery, CellCutter, CellRunSummary, CellStatus};
use obc_bake::presets::StyleDoc;
use obc_bake::regions::Region;
use obc_bake::source::LocalExtracts;
use obc_pack::config::Config;
use obc_pack::cut::{CutOptions, CutSummary};
use obc_pack::geom::Geom;
use obc_pack::grid::{BandTable, CellId};
use obc_pack::ingest::{IngestFeature, Ingested};
use obc_pack::nav::RoutableWay;
use obc_pack::poi::Poi;
use obc_pack::progress::Progress;

const SNAPSHOT: &str = "2026-07-28";
const BASE_URL: &str = "https://maps.example/cells";
const GENERATED_AT: &str = "2026-07-30T00:00:00Z";

/// A three-level ladder with no simplification, and a `_meta` block so the bakery can
/// load it as the schema: the config every cell in these tests is cut with.
const SCHEMA_JSON: &str = r#"{
    "_meta": {
        "id": "testschema",
        "name": "Test schema",
        "description": "A three-level ladder for the cell bake tests.",
        "version": 1
    },
    "lods": [
        {"max_mpp": null, "simplify": 0},
        {"max_mpp": 20, "simplify": 0},
        {"max_mpp": 4, "simplify": 0}
    ],
    "features": {
        "highway": { "residential": {"color": "0xF800", "weight": 2, "min_lod": 1} },
        "natural": { "water": {"color": "0x001F", "weight": 1, "min_lod": 0} }
    },
    "marker": {"color": "0xF800"},
    "chunk_size": 4096,
    "routing": {"min_component_edges": 4}
}"#;

/// A **skin** over that schema, shaped the way the shipped ones are: the same feature
/// types in the same document order (which is what fixes the style ids), carrying only
/// presentation. No ladder, no `min_lod`, no routing — those are schema data, and a
/// skin that restated them would be claiming to change bytes it is stamped on top of.
const SKIN_JSON: &str = r#"{
    "_meta": {
        "id": "testskin",
        "name": "Test skin",
        "description": "The test schema's own look, restated as a skin.",
        "version": 1
    },
    "features": {
        "highway": { "residential": {"color": "0xF800", "weight": 2} },
        "natural": { "water": {"color": "0x001F", "weight": 1} }
    },
    "marker": {"color": "0xF800"}
}"#;

/// One geometry band and one core band, both `2^18` — the smallest table that still
/// satisfies `OBCA_Spec.md` §1.2's partition rule and §5.1's role rules, and it keeps
/// two bands on one cell size, which is where the path-collision trap lives.
const BANDS_JSON: &str = r#"{"bands": [
    {"id": "coarse",  "cell_log2": 18, "lods": [0, 1, 2], "role": "coarse"},
    {"id": "network", "cell_log2": 18, "lods": [], "sections": ["nav", "poi"], "role": "core"}
]}"#;

// --- the fixture geography --------------------------------------------------------

/// `west`'s polygon: from inside column 1051 to the middle of column 1053, and from
/// inside row 1203 to inside row 1205.
const WEST_POLY: &str = "west\n1\n   7.240032   47.085920\n   7.733248   47.085920\n   7.733248   47.548064\n   \
                         7.240032   47.548064\n   7.240032   47.085920\nEND\nEND\n";
/// `east`'s polygon: from that same middle of column 1053 to inside column 1055.
const EAST_POLY: &str = "east\n1\n   7.733248   47.085920\n   8.226464   47.085920\n   8.226464   47.548064\n   \
                         7.733248   47.548064\n   7.733248   47.085920\nEND\nEND\n";

/// The cell each region alone covers completely, and the one only a co-bake covers.
const WEST_CORE: &str = "18/1204/1052";
const EAST_CORE: &str = "18/1204/1054";
const SEAM_CELL: &str = "18/1204/1053";

fn regions_toml() -> &'static str {
    "regions = [\n  { id = \"europe/west\", name = \"West\" },\n  { id = \"europe/east\", name = \"East\" },\n]\n"
}

// --- the cutter -------------------------------------------------------------------

/// Cuts the real cutter over a synthetic ingest.
///
/// Real, because everything downstream of the cut — the header bbox check, the reader
/// round-trip, the catalog generator's own `bbox == id` law — is only meaningful
/// against bytes a cutter actually produced. Synthetic, because a PBF cannot be placed
/// on a grid line by hand.
struct FixtureCutter {
    calls: AtomicUsize,
    /// Every `(sorted source ids, sorted cell ids)` this cutter was asked for — the
    /// plan grouping, observed from the outside.
    plans: Mutex<Vec<(Vec<String>, Vec<String>)>>,
    /// Whether each run was handed a crop box.
    cropped: Mutex<Vec<bool>>,
}

impl FixtureCutter {
    fn new() -> Self {
        Self { calls: AtomicUsize::new(0), plans: Mutex::new(Vec::new()), cropped: Mutex::new(Vec::new()) }
    }

    fn plans(&self) -> Vec<(Vec<String>, Vec<String>)> {
        self.plans.lock().expect("plans").clone()
    }
}

impl CellCutter for FixtureCutter {
    fn recipe(&self) -> String {
        "fixture-cut".into()
    }

    fn cut(
        &self,
        _pbfs: &[String],
        config: &Config,
        out_dir: &Path,
        opts: &CutOptions,
        progress: &Progress,
    ) -> Result<CutSummary, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut sources: Vec<String> = opts.sources.iter().map(|s| s.id.clone()).collect();
        sources.sort();
        let mut cells: Vec<String> = opts.select.iter().map(ToString::to_string).collect();
        cells.sort();
        self.plans.lock().expect("plans").push((sources, cells));
        self.cropped.lock().expect("cropped").push(opts.bbox.is_some());
        let (ing, ways) = fixture(config);
        obc_pack::cut::cut_ingested(&ing, &ways, config, out_dir, opts, progress)
    }
}

/// Degrees from microdegrees.
fn deg(udeg: i64) -> f64 {
    udeg as f64 / 1e6
}

/// An extract spanning every cell the fixture regions touch: one lake over the whole
/// area, one road across row 1204, and a POI per column.
fn fixture(cfg: &Config) -> (Ingested, Vec<RoutableWay>) {
    let style = |key: &str, value: &str| {
        cfg.get_style(&std::collections::HashMap::from([(key, value)])).expect("styled feature type").id
    };
    let (road, water) = (style("highway", "residential"), style("natural", "water"));
    let (lat0, lat1) = (47_000_000i64, 47_600_000i64);
    let (lon0, lon1) = (7_100_000i64, 8_300_000i64);
    let ring = vec![
        (deg(lon0), deg(lat0)),
        (deg(lon1), deg(lat0)),
        (deg(lon1), deg(lat1)),
        (deg(lon0), deg(lat1)),
        (deg(lon0), deg(lat0)),
    ];
    let features = vec![
        IngestFeature { style_id: water, min_lod: 0, geom: Geom::Polygon { exterior: ring, interiors: Vec::new() } },
        IngestFeature {
            style_id: road,
            min_lod: 1,
            geom: Geom::Line((0..=24).map(|k| (deg(lon0 + k * (lon1 - lon0) / 24), deg(47_300_000))).collect()),
        },
    ];
    // One long road across the whole area: every cell of row 1204 gets nav content,
    // and every column boundary gets a deterministic boundary junction.
    let ways = vec![RoutableWay {
        node_ids: (0..=24).collect(),
        coords: (0..=24).map(|k| ((lon0 + k * (lon1 - lon0) / 24) as i32, 47_300_000i32)).collect(),
        kind: 7,
    }];
    let pois = (0..6)
        .map(|k| Poi {
            subtype: 1,
            lon_udeg: (lon0 + k * 200_000) as i32,
            lat_udeg: 47_310_000,
            name: Some(format!("POI {k}")),
            from_node: true,
            hours: None,
        })
        .collect();
    (Ingested { features, coastlines: Vec::new(), pois, nav_graph: Default::default() }, ways)
}

// --- the harness ------------------------------------------------------------------

struct Fixture {
    dir: PathBuf,
    regions: Vec<Region>,
    schema: StyleDoc,
    skins: Vec<StyleDoc>,
    tree: PathBuf,
    extracts: PathBuf,
}

fn fixture_dirs(name: &str) -> Fixture {
    let dir = std::env::temp_dir().join(format!("obc-cellbake-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let extracts = dir.join("extracts");
    std::fs::create_dir_all(extracts.join("europe")).expect("extract root");
    for (id, poly) in [("west", WEST_POLY), ("east", EAST_POLY)] {
        std::fs::write(extracts.join(format!("europe/{id}-latest.osm.pbf")), b"not a real pbf").unwrap();
        std::fs::write(extracts.join(format!("europe/{id}.poly")), poly).unwrap();
    }
    let presets_dir = dir.join("presets");
    std::fs::create_dir_all(presets_dir.join(obc_bake::presets::SKINS_DIR)).unwrap();
    std::fs::write(presets_dir.join(obc_bake::presets::SCHEMA_DOC), SCHEMA_JSON).unwrap();
    std::fs::write(presets_dir.join("skins/testskin.json"), SKIN_JSON).unwrap();
    let schema = obc_bake::presets::load_schema(&presets_dir).expect("the test schema loads");
    let skins = obc_bake::presets::load_skins(&presets_dir, None).expect("the test skin loads");
    let regions = obc_bake::regions::parse(regions_toml()).expect("region list parses");
    Fixture { tree: dir.join("tree"), dir, regions, schema, skins, extracts }
}

impl Fixture {
    fn bake(&self, cutter: &dyn CellCutter, only: &[&str], snapshot: &str, force: bool) -> CellRunSummary {
        let regions: Vec<Region> =
            self.regions.iter().filter(|r| only.is_empty() || only.contains(&r.id.as_str())).cloned().collect();
        let skins: Vec<&StyleDoc> = self.skins.iter().collect();
        CellBakery {
            regions: &regions,
            schema: &self.schema,
            skins: &skins,
            source: &LocalExtracts::new(&self.extracts).with_snapshot(snapshot),
            cutter,
            opts: CellBakeOptions {
                out: self.tree.clone(),
                force,
                fail_fast: false,
                bands: BandTable::parse(BANDS_JSON).expect("band table"),
                schema_id: "testschema".into(),
                schema_revision: 1,
            },
        }
        .run(&Progress::silent())
        .expect("the run itself completes; per-plan failures are in the summary")
    }

    /// Generate the v2 catalog into the tree, as the CLI does after a bake.
    fn catalog(&self) -> obc_pack::catalog::v2::GeneratedCatalogV2 {
        let opts = obc_pack::catalog::v2::CatalogV2Options::new(BASE_URL, GENERATED_AT);
        let generated = obc_pack::catalog::v2::generate(&self.tree, &opts).expect("the cell tree generates a catalog");
        obc_pack::catalog::v2::write_all_atomic(&self.tree, &generated).expect("write");
        generated
    }

    /// A published cell's sidecar, as JSON.
    fn sidecar(&self, band: &str, id: &str) -> serde_json::Value {
        let mut parts = id.split('/');
        let (_, i, j) = (parts.next(), parts.next().unwrap(), parts.next().unwrap());
        let path = self.tree.join("cells").join(band).join(i).join(format!("{j}.obcm.json"));
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())))
            .expect("sidecar parses")
    }

    fn partial(&self, band: &str, id: &str) -> bool {
        self.sidecar(band, id)["partial"].as_bool().expect("partial is a bool")
    }

    fn cell_ids(&self, band: &str) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        let dir = self.tree.join("cells").join(band);
        let Ok(rows) = std::fs::read_dir(&dir) else { return out };
        for row in rows.flatten() {
            let i = row.file_name().to_string_lossy().into_owned();
            for f in std::fs::read_dir(row.path()).into_iter().flatten().flatten() {
                let name = f.file_name().to_string_lossy().into_owned();
                if let Some(j) = name.strip_suffix(".obcm") {
                    out.insert(format!("18/{i}/{j}"));
                }
            }
        }
        out
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn statuses(summary: &CellRunSummary) -> BTreeMap<String, CellStatus> {
    summary.plans.iter().flat_map(|p| p.cells.iter().map(|c| (format!("{} [{}]", c.id, c.band), c.status))).collect()
}

// --- the tests --------------------------------------------------------------------

/// The ownership rule, observed from outside the bakery: three plans, keyed by source
/// set, with the seam column owned by the pair.
#[test]
fn co_baked_neighbours_share_one_plan_for_the_cells_they_straddle() {
    let f = fixture_dirs("plans");
    let cutter = FixtureCutter::new();
    let summary = f.bake(&cutter, &[], SNAPSHOT, false);
    assert!(summary.ok(), "{}", summary.render());

    let plans = cutter.plans();
    assert_eq!(plans.len(), 3, "west-only, east-only, and the pair: {plans:?}");
    let by_sources: BTreeMap<Vec<String>, Vec<String>> = plans.into_iter().collect();
    let west = by_sources.get(&vec!["europe/west".to_string()]).expect("a west-only plan");
    let east = by_sources.get(&vec!["europe/east".to_string()]).expect("an east-only plan");
    let both = by_sources
        .get(&vec!["europe/east".to_string(), "europe/west".to_string()])
        .expect("a plan cut from BOTH extracts");

    assert_eq!(both.len(), 3, "the seam column, three rows: {both:?}");
    assert!(both.iter().all(|id| id.starts_with("18/12") && id.ends_with("/1053")), "{both:?}");
    assert!(west.iter().all(|id| !id.ends_with("/1053")), "no cell is in two plans: {west:?}");
    assert!(east.iter().all(|id| !id.ends_with("/1053")), "{east:?}");
    assert_eq!(west.len() + east.len() + both.len(), 15, "every touched cell is planned exactly once");

    // The pair's run is cropped to its own cells; a single-extract run is not.
    let cropped = cutter.cropped.lock().unwrap().clone();
    assert_eq!(cropped.iter().filter(|c| **c).count(), 1, "only the multi-source plan crops: {cropped:?}");
}

/// D3, the property the plan grouping exists for: a border cell is canonical only when
/// its whole square is covered by the sources it was cut from.
#[test]
fn a_seam_cell_is_partial_alone_and_canonical_co_baked() {
    let alone = fixture_dirs("partial-alone");
    let summary = alone.bake(&FixtureCutter::new(), &["europe/west"], SNAPSHOT, false);
    assert!(summary.ok(), "{}", summary.render());
    assert!(!alone.partial("coarse", WEST_CORE), "the cell west covers entirely is canonical");
    assert!(alone.partial("coarse", SEAM_CELL), "west alone leaves the seam cell's eastern sliver uncovered");
    assert!(alone.partial("network", SEAM_CELL), "…in every band");

    let both = fixture_dirs("partial-both");
    let summary = both.bake(&FixtureCutter::new(), &[], SNAPSHOT, false);
    assert!(summary.ok(), "{}", summary.render());
    assert!(!both.partial("coarse", SEAM_CELL), "co-baked, the union covers the square (OBCA §3.7)");
    assert!(!both.partial("coarse", WEST_CORE));
    assert!(!both.partial("coarse", EAST_CORE));
    // A cell the coverage's northern edge crosses is still partial, co-bake or not.
    assert!(both.partial("coarse", "18/1205/1053"), "the row the border crosses is not covered");

    // The sidecar names every source, sorted, with its snapshot — §11.6's provenance.
    let sources = both.sidecar("coarse", SEAM_CELL)["sources"].clone();
    assert_eq!(
        sources,
        serde_json::json!([
            {"extract_id": "europe/east", "snapshot": SNAPSHOT},
            {"extract_id": "europe/west", "snapshot": SNAPSHOT}
        ])
    );
    assert_eq!(both.sidecar("coarse", WEST_CORE)["sources"].as_array().map(Vec::len), Some(1));
}

/// The incremental property, at plan granularity: nothing is ingested when nothing
/// changed.
#[test]
fn an_unchanged_rerun_cuts_nothing_and_a_force_cuts_everything() {
    let f = fixture_dirs("idempotent");
    let cutter = FixtureCutter::new();
    let first = f.bake(&cutter, &[], SNAPSHOT, false);
    assert!(first.ok(), "{}", first.render());
    assert_eq!(cutter.calls.load(Ordering::SeqCst), 3, "one cut per plan");
    let before: BTreeMap<String, Vec<u8>> = f
        .cell_ids("coarse")
        .iter()
        .map(|id| {
            let mut p = id.split('/');
            let (_, i, j) = (p.next(), p.next().unwrap(), p.next().unwrap());
            let path = f.tree.join("cells/coarse").join(i).join(format!("{j}.obcm"));
            (id.clone(), std::fs::read(path).unwrap())
        })
        .collect();

    let second = f.bake(&cutter, &[], SNAPSHOT, false);
    assert!(second.ok(), "{}", second.render());
    assert_eq!(cutter.calls.load(Ordering::SeqCst), 3, "no plan was ingested a second time");
    assert!(statuses(&second).values().all(|s| *s == CellStatus::Unchanged), "{:?}", statuses(&second));
    for (id, bytes) in &before {
        let mut p = id.split('/');
        let (_, i, j) = (p.next(), p.next().unwrap(), p.next().unwrap());
        assert_eq!(&std::fs::read(f.tree.join("cells/coarse").join(i).join(format!("{j}.obcm"))).unwrap(), bytes);
    }

    let forced = f.bake(&cutter, &[], SNAPSHOT, true);
    assert_eq!(cutter.calls.load(Ordering::SeqCst), 6, "--force re-cuts every plan");
    assert!(statuses(&forced).values().all(|s| *s == CellStatus::Cut), "{:?}", statuses(&forced));
    // Determinism (OBCA §3.2): the same sources cut the same cell to the same bytes.
    for (id, bytes) in &before {
        let mut p = id.split('/');
        let (_, i, j) = (p.next(), p.next().unwrap(), p.next().unwrap());
        assert_eq!(
            &std::fs::read(f.tree.join("cells/coarse").join(i).join(format!("{j}.obcm"))).unwrap(),
            bytes,
            "{id} is byte-identical across a forced re-cut"
        );
    }
}

/// A re-dated but byte-identical extract publishes the new date and re-cuts nothing —
/// the same two-key design [`obc_bake::bake`] uses, at cell granularity.
#[test]
fn a_redated_but_identical_extract_refreshes_the_sidecar_and_cuts_nothing() {
    let f = fixture_dirs("redate");
    let cutter = FixtureCutter::new();
    f.bake(&cutter, &[], SNAPSHOT, false);
    let calls = cutter.calls.load(Ordering::SeqCst);
    let built_at = f.sidecar("coarse", WEST_CORE)["built_at"].clone();

    let summary = f.bake(&cutter, &[], "2026-07-29", false);
    assert!(summary.ok(), "{}", summary.render());
    assert_eq!(cutter.calls.load(Ordering::SeqCst), calls, "a re-dated identical extract must not re-cut");
    assert!(statuses(&summary).values().all(|s| *s == CellStatus::SidecarRefreshed), "{:?}", statuses(&summary));
    let sidecar = f.sidecar("coarse", WEST_CORE);
    assert_eq!(sidecar["sources"][0]["snapshot"], "2026-07-29", "the published date follows the extract");
    assert_eq!(sidecar["built_at"], built_at, "built_at describes when the bytes were cut, and they were not");
}

/// D3's other half: a narrower bake must never take coverage away.
#[test]
fn a_canonical_cell_is_never_replaced_by_a_partial_one() {
    let f = fixture_dirs("no-downgrade");
    let cutter = FixtureCutter::new();
    f.bake(&cutter, &[], SNAPSHOT, false);
    assert!(!f.partial("coarse", SEAM_CELL));
    let canonical = std::fs::read(f.tree.join("cells/coarse/1204/1053.obcm")).unwrap();
    let calls = cutter.calls.load(Ordering::SeqCst);

    // Now bake west alone. Its plan owns the seam cell, its sources no longer cover
    // it, and the cell it would write is thinner than the one already published.
    let summary = f.bake(&cutter, &["europe/west"], SNAPSHOT, false);
    assert!(summary.ok(), "{}", summary.render());
    let seam = statuses(&summary);
    assert_eq!(seam.get(&format!("{SEAM_CELL} [coarse]")), Some(&CellStatus::KeptCanonical));
    assert_eq!(seam.get(&format!("{SEAM_CELL} [network]")), Some(&CellStatus::KeptCanonical));
    assert!(!f.partial("coarse", SEAM_CELL), "the published cell is still the canonical one");
    assert_eq!(std::fs::read(f.tree.join("cells/coarse/1204/1053.obcm")).unwrap(), canonical, "byte-for-byte");
    // A real run: the seam column was ingested again (its source set shrank, so its
    // recipe changed) and refused at the install gate rather than never attempted.
    assert_eq!(cutter.calls.load(Ordering::SeqCst), calls + 1, "one plan re-cut — the seam column's");
    // The cells west already owned alone are untouched: their recipe did not change,
    // which is the same skip the incremental test pins.
    assert_eq!(seam.get(&format!("{WEST_CORE} [coarse]")), Some(&CellStatus::Unchanged));
}

/// A skin that is not a skin over the schema stops the run **before** the first
/// extract is fetched (#1036).
///
/// The generator would refuse the finished tree anyway, so what this buys is the
/// moment of failure: without it a DACH bake spends hours cutting cells and then ends
/// with a tree that has no catalog. The mismatch below is the shape epic #1016 D2
/// retired `high-detail` for — a document that styles a feature type the schema does
/// not have is a different *schema*, and the answer is a revision and a re-bake.
#[test]
fn a_skin_that_does_not_fit_the_schema_refuses_the_bake_before_any_cutting() {
    let f = fixture_dirs("skin-mismatch");
    let stray = r#"{
        "_meta": {"id": "stray", "name": "Stray", "description": "Styles a type the schema lacks.", "version": 1},
        "features": {
            "highway": { "residential": {"color": "0xF800", "weight": 2} },
            "natural": { "water": {"color": "0x001F", "weight": 1} },
            "aeroway": { "runway": {"color": "0x0000", "weight": 2} }
        },
        "marker": {"color": "0xF800"}
    }"#;
    let dir = f.dir.join("presets");
    std::fs::write(dir.join("skins/stray.json"), stray).unwrap();
    let skins = obc_bake::presets::load_skins(&dir, None).expect("both skins load as configs");
    let refs: Vec<&StyleDoc> = skins.iter().collect();
    let cutter = FixtureCutter::new();
    let err = CellBakery {
        regions: &f.regions,
        schema: &f.schema,
        skins: &refs,
        source: &LocalExtracts::new(&f.extracts).with_snapshot(SNAPSHOT),
        cutter: &cutter,
        opts: CellBakeOptions {
            out: f.tree.clone(),
            force: false,
            fail_fast: false,
            bands: BandTable::parse(BANDS_JSON).expect("band table"),
            schema_id: "testschema".into(),
            schema_revision: 1,
        },
    }
    .run(&Progress::silent())
    .expect_err("a skin the generator would reject must not be baked against");
    assert!(err.contains("stray") && err.contains("aeroway.runway"), "the error names the skin and the type: {err}");
    assert_eq!(cutter.calls.load(Ordering::SeqCst), 0, "and nothing was cut");
    assert!(!f.tree.exists(), "not even a tree");
}

/// A skin carrying **schema** keys is refused too, before any cutting, naming them.
///
/// `check_skin` cannot catch this: a parsed config that omits `lods` and one that
/// restates the defaults are the same value. So the document's text is checked, and
/// the alternative — quietly dropping the keys — is the failure worth avoiding: the
/// skin's author would go on believing their document changes a ladder that was fixed
/// when the cells were cut.
#[test]
fn a_skin_carrying_schema_data_refuses_the_bake_before_any_cutting() {
    let f = fixture_dirs("skin-schema-keys");
    let bossy = r#"{
        "_meta": {"id": "bossy", "name": "Bossy", "description": "Thinks it sets the ladder.", "version": 1},
        "lods": [{"max_mpp": null, "simplify": 0}, {"max_mpp": 20, "simplify": 0}, {"max_mpp": 4, "simplify": 0}],
        "routing": {"min_component_edges": 4},
        "features": {
            "highway": { "residential": {"color": "0xF800", "weight": 2, "min_lod": 1} },
            "natural": { "water": {"color": "0x001F", "weight": 1, "min_lod": 0} }
        },
        "marker": {"color": "0xF800"}
    }"#;
    let dir = f.dir.join("presets");
    std::fs::write(dir.join("skins/bossy.json"), bossy).unwrap();
    let skins = obc_bake::presets::load_skins(&dir, None).expect("both skins load as configs");
    let refs: Vec<&StyleDoc> = skins.iter().collect();
    let cutter = FixtureCutter::new();
    let err = CellBakery {
        regions: &f.regions,
        schema: &f.schema,
        skins: &refs,
        source: &LocalExtracts::new(&f.extracts).with_snapshot(SNAPSHOT),
        cutter: &cutter,
        opts: CellBakeOptions {
            out: f.tree.clone(),
            force: false,
            fail_fast: false,
            bands: BandTable::parse(BANDS_JSON).expect("band table"),
            schema_id: "testschema".into(),
            schema_revision: 1,
        },
    }
    .run(&Progress::silent())
    .expect_err("a skin that states schema data must not be baked against");
    assert!(err.contains("bossy.json"), "the error names the document: {err}");
    for key in ["`lods`", "`routing`", "`features.*.*.min_lod`"] {
        assert!(err.contains(key), "and every offending key, not just the first — missing {key}: {err}");
    }
    assert_eq!(cutter.calls.load(Ordering::SeqCst), 0, "and nothing was cut");
}

/// `--schema-id` and the document's `_meta.id` are one fact stated twice, so they are
/// **checked against each other** rather than one overwriting the other.
///
/// The old behaviour stamped the flag into the tree's copy of the document, which made
/// the disagreement unobservable: a typo in `--schema-id` published the bikepacking
/// schema's cells under some other name, and a store's id is the identity a rider's
/// already-downloaded cells get matched against.
#[test]
fn a_schema_id_that_disagrees_with_the_document_is_refused_not_overwritten() {
    let f = fixture_dirs("schema-id");
    let skins: Vec<&StyleDoc> = f.skins.iter().collect();
    let cutter = FixtureCutter::new();
    let err = CellBakery {
        regions: &f.regions,
        schema: &f.schema,
        skins: &skins,
        source: &LocalExtracts::new(&f.extracts).with_snapshot(SNAPSHOT),
        cutter: &cutter,
        opts: CellBakeOptions {
            out: f.tree.clone(),
            force: false,
            fail_fast: false,
            bands: BandTable::parse(BANDS_JSON).expect("band table"),
            schema_id: "typoschema".into(),
            schema_revision: 1,
        },
    }
    .run(&Progress::silent())
    .expect_err("the run must not publish this schema under another name");
    assert!(err.contains("testschema") && err.contains("typoschema"), "the error names both: {err}");
    assert!(
        !f.tree.join(obc_bake::presets::SCHEMA_DOC).exists(),
        "and no document was written claiming the wrong id"
    );
}

/// A skin dropped from the run is **pruned** from a pre-existing tree.
///
/// The generator publishes whatever it finds in `skins/`, so a leftover from an
/// earlier bake would be offered to riders as a current look over a store it may no
/// longer fit — and narrowing `--skin` is exactly how an operator says "not that one
/// any more". The directory and the catalog must not be able to disagree.
#[test]
fn a_skin_the_run_no_longer_publishes_is_pruned_from_the_tree() {
    let f = fixture_dirs("skin-prune");
    let second = r#"{
        "_meta": {"id": "retiring", "name": "Retiring", "description": "Shipped once, then dropped.", "version": 1},
        "features": {
            "highway": { "residential": {"color": "0x001F", "weight": 2} },
            "natural": { "water": {"color": "0xF800", "weight": 1} }
        },
        "marker": {"color": "0x001F"}
    }"#;
    let dir = f.dir.join("presets");
    std::fs::write(dir.join("skins/retiring.json"), second).unwrap();
    let both = obc_bake::presets::load_skins(&dir, None).expect("both skins load");
    let bake = |skins: &[&StyleDoc]| {
        CellBakery {
            regions: &f.regions,
            schema: &f.schema,
            skins,
            source: &LocalExtracts::new(&f.extracts).with_snapshot(SNAPSHOT),
            cutter: &FixtureCutter::new(),
            opts: CellBakeOptions {
                out: f.tree.clone(),
                force: false,
                fail_fast: false,
                bands: BandTable::parse(BANDS_JSON).expect("band table"),
                schema_id: "testschema".into(),
                schema_revision: 1,
            },
        }
        .run(&Progress::silent())
        .expect("the run completes")
    };

    bake(&both.iter().collect::<Vec<_>>());
    let published = f.tree.join(obc_bake::presets::SKINS_DIR);
    assert!(published.join("retiring.json").is_file(), "both skins land the first time");

    // The second run publishes one — as `--skin testskin` would.
    let kept: Vec<&StyleDoc> = both.iter().filter(|s| s.id == "testskin").collect();
    bake(&kept);
    assert!(published.join("testskin.json").is_file());
    assert!(!published.join("retiring.json").exists(), "the dropped skin must not survive in the tree");
    assert_eq!(f.catalog().root.skins.len(), 1, "and the catalog offers exactly what the directory holds");
}

/// The tree a bake leaves is one the v2 generator accepts and the verifier passes.
#[test]
fn the_tree_generates_a_v2_catalog_that_verifies() {
    let f = fixture_dirs("catalog");
    let summary = f.bake(&FixtureCutter::new(), &[], SNAPSHOT, false);
    assert!(summary.ok(), "{}", summary.render());

    let generated = f.catalog();
    let root = &generated.root;
    assert_eq!(root.schema.id, "testschema", "the published id is the schema document's own");
    assert_eq!(root.schema.revision, 1);
    assert_eq!(root.schema.obcm_version, obc_formats::obcm::VERSION, "read out of the cells' own headers");
    assert_eq!(root.cell_index.len(), 2, "one index per band");
    assert!(root.cell_index.iter().all(|b| b.cell_count == 15), "{:?}", root.cell_index);
    assert_eq!(root.skins.len(), 1);
    // The published skin is the presentation-only document the bake copied in, and its
    // styles line up one-for-one with the schema's id assignment (§11.4) — which is
    // what lets an assembler stamp `skins[k].styles` straight into the style table.
    let skin = &root.skins[0];
    assert_eq!(skin.id, "testskin");
    assert_eq!(
        skin.styles.iter().map(|s| s.feature_type.as_str()).collect::<Vec<_>>(),
        root.schema.styles.iter().map(|s| s.feature_type.as_str()).collect::<Vec<_>>(),
        "a skin covers exactly the schema's feature types, in the schema's own id order"
    );

    let ids: Vec<&str> = root.regions.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["europe/east", "europe/west"], "sorted, and both are selections now");
    let west = root.regions.iter().find(|r| r.id == "europe/west").expect("west");
    assert_eq!(west.name, "West");
    assert_eq!(west.cell_count.values().copied().collect::<Vec<u32>>(), vec![9, 9], "nine cells per band");
    assert_eq!(west.bytes_by_band.values().sum::<u64>(), west.bytes, "the per-file projection adds up (§5.7)");
    // Two of West's nine cells per band are canonical: the one it covers alone, and the
    // seam cell the co-bake completed — which is the epic's saving made visible, since
    // that cell is the *same* cell East's selection names.
    assert_eq!(west.partial_cell_count, 14, "18 cells, 4 of them canonical");
    let east = root.regions.iter().find(|r| r.id == "europe/east").expect("east");
    assert_eq!(east.partial_cell_count, 14);
    assert!(!west.boundary.rings.is_empty(), "and it carries a drawable outline (§11.8)");

    let guard = obc_bake::guard::check_cell_store(&f.tree).expect("guard runs");
    assert!(guard.ok(), "{}", guard.render());
    assert_eq!(guard.cells, 30, "fifteen cells in each of two bands");
    assert_eq!(guard.revision, 1);

    let report = obc_bake::verify::verify_cell_tree(&f.tree, obc_bake::verify::CellTreeVerifyOptions { sample: 1 })
        .expect("verify runs");
    assert!(report.ok(), "{}", report.render());
    assert_eq!(report.cells, 30);
    assert_eq!(report.sampled, 30, "sample = 1 opens every cell with the real reader");
    assert_eq!(report.regions, 2);

    // Every cell's header bbox is its own grid square — the law §11.6 stores no bbox
    // for, checked here through the file the catalog published.
    for band in ["coarse", "network"] {
        for id in f.cell_ids(band) {
            let cell = CellId::parse(&id).expect("canonical id");
            let mut p = id.split('/');
            let (_, i, j) = (p.next(), p.next().unwrap(), p.next().unwrap());
            let path = f.tree.join("cells").join(band).join(i).join(format!("{j}.obcm"));
            let (_, bbox) = obc_bake::verify::header_of(&path).expect("header");
            let sq = cell.square();
            assert_eq!(
                (bbox.min_lon as i64, bbox.min_lat as i64, bbox.max_lon as i64, bbox.max_lat as i64),
                sq,
                "{id} [{band}]"
            );
        }
    }
}

/// The lockstep guard: a store that mixes schema revisions is not partly stale, it is
/// unassemblable — so it fails, loudly, with no override.
#[test]
fn a_mixed_revision_store_fails_the_guard() {
    let f = fixture_dirs("lockstep");
    f.bake(&FixtureCutter::new(), &[], SNAPSHOT, false);
    assert!(obc_bake::guard::check_cell_store(&f.tree).expect("guard").ok());

    let path = f.tree.join("cells/coarse/1204/1052.obcm.json");
    let mut sidecar: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    sidecar["schema_revision"] = serde_json::json!(2);
    std::fs::write(&path, serde_json::to_string_pretty(&sidecar).unwrap()).unwrap();

    let guard = obc_bake::guard::check_cell_store(&f.tree).expect("guard runs");
    assert!(!guard.ok());
    let text = guard.render();
    assert!(text.contains("FAILED"), "{text}");
    assert!(text.contains("schema revision 2"), "{text}");
    assert!(text.contains("obc-bake bake --cells"), "the failure must say what to do: {text}");
    // And the generator refuses the same tree, for the same reason.
    let opts = obc_pack::catalog::v2::CatalogV2Options::new(BASE_URL, GENERATED_AT);
    let err = obc_pack::catalog::v2::generate(&f.tree, &opts).expect_err("a mixed-revision tree is unpublishable");
    assert!(err.contains("schema revision"), "{err}");
}

/// A satellite that does not match the digest the root pinned MUST be rejected —
/// `OBCC_Spec.md` §11.1's all-or-nothing guarantee, per document.
#[test]
fn a_tampered_satellite_fails_verification() {
    let f = fixture_dirs("satellite");
    f.bake(&FixtureCutter::new(), &[], SNAPSHOT, false);
    f.catalog();
    assert!(obc_bake::verify::verify_cell_tree(&f.tree, Default::default()).unwrap().ok());

    let index = f.tree.join("cells/coarse/index.json");
    let mut doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&index).unwrap()).unwrap();
    doc["cells"][0]["bytes"] = serde_json::json!(1);
    std::fs::write(&index, serde_json::to_string_pretty(&doc).unwrap()).unwrap();

    let report = obc_bake::verify::verify_cell_tree(&f.tree, Default::default()).expect("verify runs");
    assert!(!report.ok());
    assert!(report.problems.iter().any(|p| p.contains("cells/coarse/index.json")), "{:?}", report.problems);
}

/// A cell tree publishes root-last, exactly as a v1 tree does — the satellites are
/// ordinary objects and must all be fetchable before the document naming them is.
#[test]
fn a_cell_tree_publishes_its_root_last() {
    let f = fixture_dirs("publish");
    f.bake(&FixtureCutter::new(), &[], SNAPSHOT, false);
    f.catalog();

    let objects = obc_bake::publish::plan_v2(&f.tree).expect("plan");
    let keys: Vec<&str> = objects.iter().map(|o| o.key.as_str()).collect();
    assert_eq!(*keys.last().unwrap(), "catalog.json", "the root is last by construction");
    assert!(keys.contains(&"schema.json"), "the schema document is published too: {keys:?}");
    assert!(keys.contains(&"cells/coarse/index.json"), "and every satellite: {keys:?}");
    assert!(keys.contains(&"regions/europe/west/cells.json"), "{keys:?}");
    assert!(keys.contains(&"regions/europe/west/boundary.poly"), "{keys:?}");
    assert!(keys.iter().all(|k| !k.contains("/.")), "no bake-state dotfile is published: {keys:?}");

    let dest = f.dir.join("published");
    let store = obc_bake::publish::DirStore::new(&dest);
    let opts = obc_pack::catalog::v2::CatalogV2Options::new(BASE_URL, GENERATED_AT);
    let report = obc_bake::publish::publish_v2(
        &f.tree,
        &store,
        &opts,
        obc_bake::publish::PublishOptions { dry_run: false, allow_shrink: false },
    )
    .expect("publish");
    assert_eq!(report.cells, 30);
    assert!(dest.join("catalog.json").is_file());
    assert!(dest.join("cells/coarse/1204/1052.obcm").is_file());

    // Publishing a west-only tree over it would un-offer East: refused by default.
    let narrowed = fixture_dirs("publish-narrow");
    narrowed.bake(&FixtureCutter::new(), &["europe/west"], SNAPSHOT, false);
    narrowed.catalog();
    let err = obc_bake::publish::publish_v2(
        &narrowed.tree,
        &store,
        &opts,
        obc_bake::publish::PublishOptions { dry_run: false, allow_shrink: false },
    )
    .expect_err("the shrink guard refuses it");
    assert!(err.contains("europe/east"), "{err}");
}
