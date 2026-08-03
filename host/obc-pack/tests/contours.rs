//! Contours through the **whole** pipeline (EL10a, #1094).
//!
//! `src/contour.rs`'s own tests own the marching squares; these own the wiring, which is where the
//! issue's acceptance actually lives: that an OBCT container on disk turns into ordinary features in
//! a real `.obcm`, that a class with no style rule costs nothing, and — the one that has to keep
//! being true for every map that is not asking for contours — that the whole feature is invisible
//! when it is off.
//!
//! Land generation is skipped throughout: it needs the ~950 MB global land-polygon dataset, which is
//! a network download and not a fixture. Nothing here is about land.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use obc_map_scene::BBox;
use obc_pack::config::{Config, ContourClass};
use obc_pack::cut::{cut, CutOptions};
use obc_pack::grid::BandTable;
use obc_pack::pipeline::{pack, PackOptions};
use obc_pack::progress::{Phase, Progress};
use obc_reader::{MapCache, MapTables, Reader, SliceSource, MAX_FEAT_PTS, MAX_FEAT_RINGS};

/// The synthetic terrain's posting and cell size — both legal OBCT v1 header values, both small, so
/// the rectangle covering the fixture is tens of KB instead of the tens of MB a production 2^19 cell
/// would be. The sampler cannot tell the difference (`OBCT_Spec.md` §1.3).
const POSTING_LOG2: u8 = 9;
const CELL_LOG2: u8 = 14;
/// Metres of rise per lattice row. At a 512 µdeg posting (~57 m) the fixture's ~40 rows climb far
/// enough to cross a few dozen 100 m levels, index contours among them.
const RISE_PER_ROW: i32 = 40;

/// `builder/tests/corpus/data/tiny.osm.pbf` covers lat 47.98..48.00, lon 7.800..7.855. The terrain
/// rectangle rounds outward to whole cells around that, which is more than the box needs.
const MIN_LAT_UDEG: i64 = 47_970_000;
const MAX_LAT_UDEG: i64 = 48_010_000;
const MIN_LON_UDEG: i64 = 7_790_000;
const MAX_LON_UDEG: i64 = 7_860_000;

/// Everything a run said, in order, as a test can inspect it.
type Reported = Arc<Mutex<Vec<(Option<Phase>, String)>>>;

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(rel)
}

fn fixture_pbf() -> String {
    repo("builder/tests/corpus/data/tiny.osm.pbf").to_string_lossy().into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-pack-contours-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

/// The OBCA cell index of a µdeg coordinate at [`CELL_LOG2`].
fn cell_of(udeg: i64) -> u32 {
    ((udeg + (1 << 28)) >> CELL_LOG2) as u32
}

/// A `.obcd` covering the fixture box with a north-facing ramp, written by the spec-fixture crate
/// rather than by anything in this repository that also *reads* OBCT.
fn write_terrain(dir: &Path) -> PathBuf {
    let (min_i, min_j) = (cell_of(MIN_LAT_UDEG), cell_of(MIN_LON_UDEG));
    let rows = (cell_of(MAX_LAT_UDEG) - min_i + 1) as u16;
    let cols = (cell_of(MAX_LON_UDEG) - min_j + 1) as u16;
    // A ramp with a small lateral jitter on it. The ramp is what makes levels exist; the jitter is
    // what makes the clamp have anything to do — it wanders each crossing by a few metres, which is
    // under the 15 m clamp and well over the finest tier's 0.5 m.
    let bytes =
        obc_vectors::terrain_container(POSTING_LOG2, CELL_LOG2, min_i, min_j, rows, cols, &|_, _| true, &|di, dj| {
            (100 + RISE_PER_ROW * di as i32 + ((dj * 7 + di * 13) % 13) as i32) as i16
        });
    let path = dir.join("ramp.obcd");
    std::fs::write(&path, bytes).expect("write terrain");
    path
}

/// A complete little config: one styled way class, a two-tier ladder, and whatever contour styles
/// the caller asks for. `contours` is spliced in verbatim so a test can state exactly the block it
/// is about.
fn config(contours: &str, contour_styles: &str) -> Config {
    let text = format!(
        r#"{{
            "lods": [{{"max_mpp": null, "simplify": 40}}, {{"max_mpp": 10, "simplify": 0.5}}],
            "features": {{
                "highway": {{"*": {{"color": "0x0001", "weight": 1}}}}
                {contour_styles}
            }},
            "contours": {contours}
        }}"#
    );
    Config::parse(&text).unwrap_or_else(|e| panic!("test config parses: {e}"))
}

const BOTH_CLASSES: &str = r#", "contour": {
    "major": {"color": "0xAD55", "weight": 1, "min_lod": 0, "priority": 4, "line_style": "dashed"},
    "index": {"color": "0xAD55", "weight": 1, "min_lod": 0, "priority": 4}
}"#;
const INDEX_ONLY: &str = r#", "contour": {"index": {"color": "0xAD55", "weight": 1, "min_lod": 0, "priority": 4}}"#;

const ON: &str = r#"{"enabled": true, "interval": 100, "index_every": 5, "simplify": 15}"#;
const OFF: &str = r#"{"enabled": false, "interval": 100, "index_every": 5, "simplify": 15}"#;

/// Pack the fixture and return `(bytes on disk, every line the run reported)`.
fn run(dir: &Path, name: &str, config: &Config, terrain: Option<PathBuf>) -> (Vec<u8>, Vec<(Option<Phase>, String)>) {
    let out = dir.join(format!("{name}.obcm"));
    let log: Reported = Arc::default();
    let sink = Arc::clone(&log);
    let progress = Progress::new(Default::default(), move |phase, line| {
        sink.lock().expect("log").push((phase, line.to_string()));
    });
    let opts = PackOptions { no_land: true, terrain, ..PackOptions::default() };
    pack(&[fixture_pbf()], config, &out, &opts, &progress).expect("pack succeeds");
    let bytes = std::fs::read(&out).expect("output is readable");
    let lines = log.lock().expect("log").clone();
    (bytes, lines)
}

fn logged(lines: &[(Option<Phase>, String)], needle: &str) -> bool {
    lines.iter().any(|(_, line)| line.contains(needle))
}

/// Traced contours reach the output as ordinary geometry, and the run says so — including the
/// Copernicus credit, which is a licence obligation that travels with the data.
#[test]
fn tracing_adds_geometry_and_credits_the_dem() {
    let dir = scratch("adds");
    let terrain = write_terrain(&dir);

    let (off, _) = run(&dir, "off", &config(OFF, BOTH_CLASSES), Some(terrain.clone()));
    let (on, lines) = run(&dir, "on", &config(ON, BOTH_CLASSES), Some(terrain));

    assert!(on.len() > off.len(), "contours must add geometry: {} vs {} bytes", on.len(), off.len());
    assert!(lines.iter().any(|(phase, _)| *phase == Some(Phase::Contours)), "the tracing pass reports its own phase");
    assert!(logged(&lines, "contour line(s)"), "the run states what it traced: {lines:#?}");
    assert!(logged(&lines, "15 m clamp"), "and that the clamp ran: {lines:#?}");
    assert!(
        logged(&lines, "Copernicus WorldDEM-30"),
        "the GLO-30 credit travels with anything derived from it: {lines:#?}"
    );
}

/// The whole feature is invisible unless it is asked for: off, or on with no terrain, or on with no
/// style rule, all pack the identical bytes a build without contours would.
#[test]
fn nothing_is_traced_unless_the_run_asks_for_all_three() {
    let dir = scratch("silent");
    let terrain = write_terrain(&dir);

    // No terrain at all: the comparison has to be against a run that also had none, because
    // `--terrain` is what fills the §8.3 per-edge ascent as well.
    let (blind, _) = run(&dir, "blind", &config(OFF, BOTH_CLASSES), None);
    let (no_terrain, lines) = run(&dir, "no-terrain", &config(ON, BOTH_CLASSES), None);
    assert_eq!(no_terrain, blind, "contours on but no terrain must change nothing");
    assert!(logged(&lines, "no terrain"), "and must say so rather than silently drawing nothing");

    // The unstyled comparison has to be against a config with the *same* style table — two style
    // records is a real byte difference and not the one this is about.
    let (unstyled_off, _) = run(&dir, "unstyled-off", &config(OFF, ""), Some(terrain.clone()));
    let (unstyled, lines) = run(&dir, "unstyled", &config(ON, ""), Some(terrain.clone()));
    assert_eq!(unstyled, unstyled_off, "a class with no style rule is not packed");
    assert!(logged(&lines, "no `features.contour.major`"), "and the operator is told: {lines:#?}");

    // And the same inputs twice are the same bytes — the trace is deterministic, not merely correct.
    let (again, _) = run(&dir, "again", &config(ON, BOTH_CLASSES), Some(terrain));
    let (once_more, _) = run(&dir, "once-more", &config(ON, BOTH_CLASSES), Some(terrain2(&dir)));
    assert_eq!(again, once_more, "two identical runs must produce identical maps");
}

/// A second copy of the same container, to prove determinism is a property of the data rather than
/// of one file handle.
fn terrain2(dir: &Path) -> PathBuf {
    let src = dir.join("ramp.obcd");
    let dst = dir.join("ramp-copy.obcd");
    std::fs::copy(src, &dst).expect("copy terrain");
    dst
}

/// The two classes are independent: styling only the index contours packs strictly less than styling
/// both, and strictly more than styling neither.
#[test]
fn each_class_costs_only_what_it_is_asked_for() {
    let dir = scratch("classes");
    let terrain = write_terrain(&dir);

    let (none, _) = run(&dir, "none", &config(ON, ""), Some(terrain.clone()));
    let (index, _) = run(&dir, "index", &config(ON, INDEX_ONLY), Some(terrain.clone()));
    let (both, _) = run(&dir, "both", &config(ON, BOTH_CLASSES), Some(terrain));

    assert!(none.len() < index.len(), "the index ladder alone still costs bytes");
    assert!(
        index.len() < both.len(),
        "every 5th level must be a small fraction of every level: {} vs {}",
        index.len(),
        both.len()
    );
}

// --- the ladder reach, through the cutter (#1104) ----------------------------------------------

/// The shipped ladder's shape (7 tiers) with the shipped split reach: `major` from LOD 3, `index`
/// one tier coarser at LOD 2. Only the numbers this test is about are the shipped ones — the styles
/// are the minimum a contour class needs to be packed at all.
const SPLIT_REACH: &str = r#", "contour": {
    "major": {"color": "0xAD55", "weight": 1, "min_lod": 3, "priority": 4, "line_style": "dashed"},
    "index": {"color": "0xAD55", "weight": 1, "min_lod": 2, "priority": 4}
}"#;

/// A 7-tier config the recommended band table partitions (`coarse` = LOD 0–2, `mid` = 3–4,
/// `fine` = 5–6), so the cut this test drives is the one the bakery drives.
fn ladder_config(contour_styles: &str) -> Config {
    let text = format!(
        r#"{{
            "lods": [
                {{"max_mpp": null, "simplify": 200}},
                {{"max_mpp": 30, "simplify": 100}},
                {{"max_mpp": 16, "simplify": 40}},
                {{"max_mpp": 10, "simplify": 15}},
                {{"max_mpp": 5, "simplify": 8}},
                {{"max_mpp": 3, "simplify": 3}},
                {{"max_mpp": 1.2, "simplify": 0.5}}
            ],
            "features": {{
                "highway": {{"*": {{"color": "0x0001", "weight": 1}}}}
                {contour_styles}
            }},
            "contours": {ON}
        }}"#
    );
    Config::parse(&text).unwrap_or_else(|e| panic!("test config parses: {e}"))
}

/// `LOD → style id → feature count`, decoded from one cell with the real reader.
fn features_per_lod(bytes: &[u8]) -> BTreeMap<usize, BTreeMap<u8, usize>> {
    let src = SliceSource(bytes);
    let tables = MapTables::parse(&src).expect("a cell parses");
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    let mut out: BTreeMap<usize, BTreeMap<u8, usize>> = BTreeMap::new();
    let mut points = heapless::Vec::<_, MAX_FEAT_PTS>::new();
    let mut ring_lens = heapless::Vec::<_, MAX_FEAT_RINGS>::new();
    for lod in 0..r.lods().len() {
        let slot = out.entry(lod).or_default();
        let mut chunks: Vec<(u32, BBox)> = Vec::new();
        r.for_each_chunk(lod, &r.bbox, |cid, node| chunks.push((cid, node))).expect("walk the index");
        for (cid, node) in chunks {
            r.for_each_feature(lod, cid, &node, &mut points, &mut ring_lens, |f| {
                *slot.entry(f.style_id).or_default() += 1;
            })
            .expect("decode");
        }
    }
    out
}

/// The reach is a per-class property all the way through the **cutter**, not only the whole-extract
/// pipeline — and in particular the `coarse` band traces contours at all.
///
/// #1103 named this as a risk: #1094 wired contours in as "the mid/fine cells", and if the cutter
/// had hard-wired a band set the coarse cells would be silently contour-free no matter what the
/// preset said. It does not — contours are traced once over the extract and then filtered by the
/// ordinary `min_lod <= lod` ladder rule — and this is the test that keeps it that way:
///
/// - LOD 2 (coarse band) carries index contours and **zero** major ones — the sparse 500 m rhythm.
/// - LODs 0–1 carry neither: the two coarsest tiers stay terrain-free.
/// - LOD 3 (mid band) carries both — `major` starts exactly where #1095 put it.
#[test]
fn the_coarse_band_carries_index_contours_only() {
    let dir = scratch("reach");
    let terrain = write_terrain(&dir);
    let cfg = ladder_config(SPLIT_REACH);
    let major = cfg.contour_style(ContourClass::Major).expect("major is styled").id;
    let index = cfg.contour_style(ContourClass::Index).expect("index is styled").id;

    let out = dir.join("cells");
    let opts =
        CutOptions { bands: BandTable::recommended(), no_land: true, terrain: Some(terrain), ..CutOptions::default() };
    let summary = cut(&[fixture_pbf()], &cfg, &out, &opts, &Progress::silent()).expect("the cut succeeds");

    let mut coarse_cells = 0;
    let mut mid_cells = 0;
    for artifact in &summary.cells {
        let counts = features_per_lod(&std::fs::read(out.join(&artifact.path)).expect("cell is readable"));
        let n = |lod: usize, style: u8| counts.get(&lod).and_then(|c| c.get(&style)).copied().unwrap_or(0);
        match artifact.band.as_str() {
            "coarse" => {
                coarse_cells += 1;
                assert_eq!(n(2, major), 0, "{}: LOD 2 is the index rhythm alone — no major contours", artifact.path);
                for lod in [0, 1] {
                    assert_eq!(n(lod, index), 0, "{}: LOD {lod} is below every contour's reach", artifact.path);
                    assert_eq!(n(lod, major), 0, "{}: LOD {lod} is below every contour's reach", artifact.path);
                }
            }
            "mid" => mid_cells += 1,
            _ => {}
        }
    }
    assert!(coarse_cells > 0, "the fixture produced coarse cells at all");
    assert!(mid_cells > 0, "…and mid cells to compare them against");

    // The positive halves, summed over the band: one cell of the box may hold no contour at all, but
    // the band as a whole must — otherwise every assertion above passes vacuously.
    let total = |band: &str, lod: usize, style: u8| -> usize {
        summary
            .cells
            .iter()
            .filter(|a| a.band == band)
            .map(|a| {
                let counts = features_per_lod(&std::fs::read(out.join(&a.path)).expect("cell is readable"));
                counts.get(&lod).and_then(|c| c.get(&style)).copied().unwrap_or(0)
            })
            .sum()
    };
    assert!(total("coarse", 2, index) > 0, "the coarse band must actually trace the index contours");
    assert!(total("mid", 3, index) > 0, "and LOD 3 keeps them");
    assert!(total("mid", 3, major) > 0, "where the major ones join in");
}

/// The clamp is what keeps the fine tiers from storing interpolation noise: relaxing it to zero has
/// to make the map bigger, or it is not doing anything.
#[test]
fn the_clamp_is_load_bearing() {
    let dir = scratch("clamp");
    let terrain = write_terrain(&dir);

    let (clamped, _) = run(&dir, "clamped", &config(ON, BOTH_CLASSES), Some(terrain.clone()));
    let unclamped = r#"{"enabled": true, "interval": 100, "index_every": 5, "simplify": 0}"#;
    let (raw, _) = run(&dir, "raw", &config(unclamped, BOTH_CLASSES), Some(terrain));

    assert!(
        clamped.len() < raw.len(),
        "the 15 m clamp must actually remove vertices: {} clamped vs {} raw",
        clamped.len(),
        raw.len()
    );
}
