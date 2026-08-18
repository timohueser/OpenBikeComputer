//! **The determinism pin**: the bridge's output is the native CLI's output, byte for byte, and it
//! is the same on every run.
//!
//! These are not "does the wrapper work" tests. They exist so that a change to the assembly engine —
//! the renumber tie-break, the shard planner, a hash-map iteration order that leaks into the
//! output — cannot ship a browser build that quietly disagrees with the command line. The inputs are
//! the checked-in cell tree in `tests/fixture/` and the expected outputs are what
//! `cargo run -p obcm-assemble` wrote from them; `tests/fixture.rs` documents the provenance of
//! both, executably.
//!
//! What that proves, precisely: the *fixture* and this crate's output share a source (the engine),
//! so this is a **drift and non-determinism guard**, not an independent correctness check — a bug in
//! the engine would move both together. That is the right scope: the engine's own correctness is
//! tested where it lives, in `obcm-assemble`'s differential oracle against the real packer. The
//! *second* host — the wasm build — is held to the same checked-in bytes from Node, in
//! `builder/app/src/lib/assemble/bridge.test.ts`. Between them the claim is complete: the browser
//! produces what the command line produces.

use std::path::{Path, PathBuf};

use obc_web_assemble::{
    assemble, assemble_cells, assemble_cells_with_known_empty, assemble_everything, BridgeOptions, CellBytes,
    CellReads, ErrorCode, Hooks, KnownEmptyCell, NoHooks, OutputFile, Phase, SealedShard, ShardWrites, SourceCell,
    TerrainCellBytes, TerrainLattice, Wiring,
};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

/// The cutter's provenance sidecar, verbatim. It doubles as the **schema document**: `Schema::parse`
/// accepts an OBCC v2 root, and `cells.json` is `{"schema": {…}, "cells": […]}`, so the same text
/// serves both roles — which is exactly what a hosted catalog hands the builder.
fn sidecar() -> String {
    std::fs::read_to_string(fixture_dir().join("cells.json")).expect("tests/fixture/cells.json — see tests/fixture.rs")
}

fn skin() -> String {
    std::fs::read_to_string(fixture_dir().join("skin.json")).expect("tests/fixture/skin.json — see tests/fixture.rs")
}

/// Load every cell the sidecar lists, in the order it lists them — the order the builder's downloads
/// finish in is not this one, but the engine sorts what it needs to sort and the output must not
/// depend on either.
fn cells() -> Vec<CellBytes> {
    let doc: serde_json::Value = serde_json::from_str(&sidecar()).expect("the sidecar is JSON");
    let dir = fixture_dir();
    doc["cells"]
        .as_array()
        .expect("the sidecar lists cells")
        .iter()
        .map(|c| CellBytes {
            id: c["id"].as_str().expect("cell id").to_string(),
            band: c["band"].as_str().expect("cell band").to_string(),
            partial: c["partial"].as_bool().unwrap_or(false),
            bytes: std::fs::read(dir.join(c["path"].as_str().expect("cell path"))).expect("a cell artifact"),
        })
        .collect()
}

/// The terrain sidecar, in the shape the CLI's `--terrain` and a catalog's §13.1 block share.
fn terrain_sidecar() -> serde_json::Value {
    let text = std::fs::read_to_string(fixture_dir().join("terrain.json"))
        .expect("tests/fixture/terrain.json — see tests/fixture.rs");
    serde_json::from_str(&text).expect("the terrain sidecar is JSON")
}

/// The store's lattice, as the catalog would state it.
fn terrain_lattice() -> TerrainLattice {
    let doc = terrain_sidecar();
    TerrainLattice {
        posting_log2: doc["posting_log2"].as_u64().expect("posting_log2") as u8,
        cell_log2: doc["cell_log2"].as_u64().expect("cell_log2") as u8,
    }
}

/// Every published terrain cell, with the digest the sidecar pins it with — exactly what the builder
/// hands over after `fetchVerified`. The fixture's fourth square is deliberately **not** here: it is
/// canonically void, so it has no object (`OBCC_Spec.md` §13.6) and must reach the shard as a `0`
/// directory slot.
fn terrain_cells() -> Vec<TerrainCellBytes> {
    let doc = terrain_sidecar();
    let dir = fixture_dir();
    doc["cells"]
        .as_array()
        .expect("the terrain sidecar lists cells")
        .iter()
        .map(|c| TerrainCellBytes {
            id: c["id"].as_str().expect("terrain cell id").to_string(),
            sha256: c["sha256"].as_str().expect("terrain cell sha256").to_string(),
            bytes: std::fs::read(dir.join(c["path"].as_str().expect("terrain cell path"))).expect("a terrain cell"),
        })
        .collect()
}

/// The whole fixture assembly — cells **and** raster — which is what the CLI wrote `expected/` from.
fn assemble_fixture(opts: &BridgeOptions, hooks: &mut dyn Hooks) -> obc_web_assemble::Outcome {
    assemble_everything(cells(), Vec::new(), Some(terrain_lattice()), terrain_cells(), &sidecar(), &skin(), opts, hooks)
        .expect("the assembly runs")
}

/// The options the fixture's `expected/` was produced with (`--name "Bridge Fixture"
/// --accept-partial`). The coarse `2^20` cell is necessarily partial at this extract's size, which
/// is why the flag is on rather than the refusal being papered over.
fn options() -> BridgeOptions {
    BridgeOptions { name: "Bridge Fixture".into(), accept_partial: true, ..BridgeOptions::default() }
}

/// What the native CLI left in `tests/fixture/<dir>/`, as `(filename, bytes)` in the order the
/// bridge must hand them on: shards ascending, manifest last.
fn expected(dir: &str) -> Vec<(String, Vec<u8>)> {
    let root = fixture_dir().join(dir);
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&root)
        .unwrap_or_else(|e| panic!("{} — see tests/fixture.rs: {e}", root.display()))
        .map(|e| {
            let e = e.expect("a directory entry");
            (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).expect("an expected file"))
        })
        .collect();
    // The order the bridge hands them on: OBCM shards ascending, then the terrain shard, then the
    // manifest **last** (§5.4). None of that is the alphabet's order — `MS1.OBD` sorts *before*
    // `MS1S00.OBM` — so the key is spelled out rather than relied on.
    let rank = |name: &str| {
        if name.ends_with(".OBS") {
            2
        } else if name.ends_with(".OBD") {
            1
        } else {
            0
        }
    };
    files.sort_by(|a, b| rank(&a.0).cmp(&rank(&b.0)).then(a.0.cmp(&b.0)));
    assert!(!files.is_empty(), "{} is empty", root.display());
    files
}

/// Fail on the first differing byte with its index and both values, instead of dumping two
/// multi-KB arrays. A byte-identity failure is usually one field, and the index says which.
fn assert_same_bytes(actual: &[u8], want: &[u8], what: &str) {
    for (i, (a, b)) in actual.iter().zip(want).enumerate() {
        assert!(
            a == b,
            "{what}: first difference at byte {i} — the bridge produced 0x{a:02x}, the native CLI has 0x{b:02x} \
             (lengths {} vs {})",
            actual.len(),
            want.len()
        );
    }
    assert_eq!(actual.len(), want.len(), "{what}: length");
}

/// The headline: a single-file assembly through the bridge **is** the file the CLI wrote.
#[test]
fn the_bridge_reproduces_the_native_clis_bytes() {
    let out = assemble_fixture(&options(), &mut NoHooks);
    let want = expected("expected");
    assert_eq!(out.files.len(), want.len(), "file count: {:?} vs {:?}", out.files, want);
    for (got, (name, bytes)) in out.files.iter().zip(&want) {
        assert_eq!(&got.name, name, "the derived filenames must match the CLI's (OBCA §5.2)");
        assert_same_bytes(&got.bytes, bytes, name);
    }
    // One OBCM file, its raster, and the manifest last — §5.5's fast path with terrain beside it,
    // which is the shape a rider's map actually has.
    assert_eq!(out.files.iter().map(|f| f.role).collect::<Vec<_>>(), vec!["core", "terrain", "manifest"]);
}

/// **The terrain round trip** (EL4): the shard the assembler wrote is a legal OBCT container whose
/// directory places every downloaded cell's block verbatim, and whose one unpublished square is the
/// `0` sentinel — read back with the real reader, from the checked-in bytes.
#[test]
fn the_terrain_shard_places_every_published_cell_and_leaves_the_void_absent() {
    let out = assemble_fixture(&options(), &mut NoHooks);
    let shard = &out.files.iter().find(|f| f.role == "terrain").expect("a terrain shard").bytes;

    // OBCT §4.2's header, over the fixture's 2 × 2 rectangle at the catalog's lattice.
    assert_eq!(&shard[..4], b"OBCT");
    assert_eq!(shard[5], terrain_lattice().posting_log2);
    assert_eq!(shard[6], terrain_lattice().cell_log2);
    assert_eq!(u16::from_le_bytes(shard[16..18].try_into().unwrap()), 2, "rows");
    assert_eq!(u16::from_le_bytes(shard[18..20].try_into().unwrap()), 2, "cols");
    let dir: Vec<u32> = shard[32..48].chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    assert_eq!(dir.iter().filter(|&&e| e == 0).count(), 1, "exactly one canonically void square (OBCC §13.6)");
    assert_eq!(dir[3], 0, "…and it is the rectangle's last slot");

    // Every present block is byte-for-byte the block of the published cell it came from — placement,
    // not grafting. The published objects' own blocks start after their 32-byte header and single
    // directory entry.
    for cell in terrain_cells() {
        let (i, j) = {
            let mut parts = cell.id.split('/').skip(1);
            (parts.next().unwrap().parse::<u32>().unwrap(), parts.next().unwrap().parse::<u32>().unwrap())
        };
        let slot = (i - 602) as usize * 2 + (j - 526) as usize;
        let at = dir[slot] as usize;
        assert!(at != 0, "cell {} was published and must be present", cell.id);
        assert_eq!(&shard[at..at + 2048], &cell.bytes[36..36 + 2048], "cell {}'s block moved", cell.id);
    }

    // §5.7's pin, on the raster: 32-byte header + 4 × 4-byte directory + 3 × 2048-byte blocks.
    assert_eq!(shard.len(), 32 + 16 + 3 * 2048);
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    assert_eq!(s["terrain"]["file"], "MS1.OBD");
    assert_eq!(s["terrain"]["bytes"], shard.len());
    assert_eq!(s["terrain"]["cells"], 3);
    assert_eq!(s["terrain"]["slots"], 4);
}

/// A selection with no raster assembles exactly as it did before terrain existed: no `.OBD`, no
/// `terrain` role, and the summary says so. `OBCC_Spec.md` §13's degrade-to-flat rule, at the seam
/// where it is easiest to get wrong.
#[test]
fn a_selection_with_no_terrain_writes_no_terrain_shard() {
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    assert!(!out.files.iter().any(|f| f.role == "terrain"));
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("summary");
    assert!(s["terrain"].is_null());
    // …and the manifest is back to one record.
    let manifest = &out.files.last().expect("a manifest").bytes;
    assert_eq!(manifest[6], 1, "Shard Count");
    assert_eq!(manifest.len(), obc_formats::obcs::manifest_len(1));
}

/// A digest the catalog does not confirm is refused, and the whole set is refused with it — the
/// §4.8 posture, on the raster: nothing self-made reaches a device unverified.
#[test]
fn a_terrain_cell_that_fails_its_catalog_digest_aborts_the_assembly() {
    let mut cells = terrain_cells();
    cells[0].bytes[64] ^= 0xFF; // one sample, deep inside the block
    let e = assemble_everything(
        self::cells(),
        Vec::new(),
        Some(terrain_lattice()),
        cells,
        &sidecar(),
        &skin(),
        &options(),
        &mut NoHooks,
    )
    .expect_err("the catalog pins these bytes");
    assert_eq!(e.code, ErrorCode::Format);
    assert!(e.message.contains("digest mismatch"), "{}", e.message);
}

#[test]
fn known_empty_cells_expand_coverage_without_payloads() {
    let base = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("base assembly");
    let base_json: serde_json::Value = serde_json::from_str(&base.summary_json).expect("base summary");
    let known_empty = vec![
        KnownEmptyCell { id: "20/0301/0264".into(), band: "coarse".into() },
        KnownEmptyCell { id: "18/1204/1056".into(), band: "fine".into() },
        KnownEmptyCell { id: "18/1204/1056".into(), band: "network".into() },
    ];
    let out = assemble_cells_with_known_empty(cells(), known_empty, &sidecar(), &skin(), &options(), &mut NoHooks)
        .expect("known-empty coverage assembles");
    let json: serde_json::Value = serde_json::from_str(&out.summary_json).expect("summary");
    assert!(json["assembly_bbox_udeg"]["span_log2"].as_u64() > base_json["assembly_bbox_udeg"]["span_log2"].as_u64());
    assert_eq!(json["cells"].as_u64(), base_json["cells"].as_u64().map(|n| n + 3));
}

/// …and the multi-file shape, which is what a resumable upload actually hands on: shards in index
/// order, the OBCS manifest **last** (OBCA §5.4 — a set with no manifest is invisible as a map,
/// which is the property an interrupted transfer wants).
#[test]
fn the_bridge_reproduces_the_native_clis_volume_set() {
    let opts = BridgeOptions { force_split: true, ..options() };
    let out = assemble_fixture(&opts, &mut NoHooks);
    let want = expected("expected-split");
    assert_eq!(out.files.len(), want.len(), "file count: {:?} vs {:?}", out.files, want);
    for (got, (name, bytes)) in out.files.iter().zip(&want) {
        assert_eq!(&got.name, name);
        assert_same_bytes(&got.bytes, bytes, name);
    }
    let roles: Vec<&str> = out.files.iter().map(|f| f.role).collect();
    assert_eq!(roles, vec!["core", "coarse", "geometry", "terrain", "manifest"], "§5.1's roles, manifest last");
    // The raster does not split with the geometry: one terrain shard per set, spanning the whole
    // assembly, however many OBCM files the map needs.
    assert_eq!(out.files.iter().filter(|f| f.role == "terrain").count(), 1);
}

/// A caller that takes every shard as it is verified (#1116 B1). Records what it was handed, in the
/// order it was handed it, so the delivery *stream* can be held to the same checked-in bytes as the
/// end-of-run set.
#[derive(Default)]
struct Evicting {
    /// `(name, role, sha256, bytes)`, in delivery order.
    taken: Vec<(String, String, String, Vec<u8>)>,
    /// Refuse the `n`-th hand-off, as a sink that ran out of disk would.
    fail_at: Option<usize>,
    /// Hand everything straight back instead of taking it — the `Ok(Some(_))` half of the contract.
    keep: bool,
    /// Ask to stop at the first progress report after this many shards have been handed over — a
    /// cancel button pressed by someone watching the files arrive.
    abort_after_taken: Option<usize>,
}

impl Hooks for Evicting {
    fn now_us(&mut self) -> u64 {
        0
    }
    fn progress(&mut self, _phase: Phase, _fraction: f64) -> bool {
        self.abort_after_taken.is_some_and(|n| self.taken.len() >= n)
    }
    fn wants_shards(&self) -> bool {
        true
    }
    fn take_shard(&mut self, shard: OutputFile) -> Result<Option<OutputFile>, String> {
        if self.fail_at == Some(self.taken.len()) {
            return Err(format!("the card is full and {} could not be written", shard.name));
        }
        self.taken.push((shard.name.clone(), shard.role.to_string(), shard.sha256.clone(), shard.bytes.clone()));
        if self.keep {
            return Ok(Some(shard));
        }
        Ok(None)
    }
}

/// **The eviction pin**: a set delivered shard-by-shard *during* the run, then the remainder at the
/// end, is the same set — the same files, in the same order, byte for byte — as the one the native
/// CLI wrote in one piece.
///
/// This is the claim B1 rests on. The output no longer stays resident until `takeFile`, so the
/// question "did we lose or reorder anything by handing it over early" is exactly the question the
/// determinism fixture already answers for the whole-set path, asked of the streamed one.
#[test]
fn the_evicted_stream_plus_the_remainder_is_the_native_clis_volume_set() {
    let opts = BridgeOptions { force_split: true, ..options() };
    let mut hooks = Evicting::default();
    let out = assemble_fixture(&opts, &mut hooks);
    let want = expected("expected-split");

    // Every OBCM shard left during the run, in index order…
    assert_eq!(
        hooks.taken.iter().map(|(_, role, _, _)| role.as_str()).collect::<Vec<_>>(),
        vec!["core", "coarse", "geometry"],
        "the shards must arrive in the order the engine wrote them"
    );
    // …and only the raster and the manifest were still in the store at the end. That is the whole
    // point: the set's residency over the run is one shard, not four.
    assert_eq!(out.files.iter().map(|f| f.role).collect::<Vec<_>>(), vec!["terrain", "manifest"]);

    // The stream, then the remainder, is the set — name by name and byte by byte.
    let delivered: Vec<(String, Vec<u8>)> = hooks
        .taken
        .iter()
        .map(|(name, _, _, bytes)| (name.clone(), bytes.clone()))
        .chain(out.files.iter().map(|f| (f.name.clone(), f.bytes.clone())))
        .collect();
    assert_eq!(delivered.len(), want.len(), "file count: {:?} vs {:?}", delivered, want);
    for ((name, bytes), (want_name, want_bytes)) in delivered.iter().zip(&want) {
        assert_eq!(name, want_name, "the derived filenames must match the CLI's (OBCA §5.2)");
        assert_same_bytes(bytes, want_bytes, name);
    }

    // The digest each shard was handed over with is the engine's own — the identity a caller records
    // against a file it has already saved. (`assemble_everything` refuses the run outright if these
    // ever disagree; this is the same equality, stated where a reader can see it.)
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    for (i, (name, role, sha256, bytes)) in hooks.taken.iter().enumerate() {
        assert_eq!(s["shards"][i]["file"], name.as_str());
        assert_eq!(s["shards"][i]["role"], role.as_str());
        assert_eq!(s["shards"][i]["sha256"], sha256.as_str());
        assert_eq!(s["shards"][i]["bytes"].as_u64(), Some(bytes.len() as u64));
    }
}

/// …and the single-file fast path evicts too: one shard out during the run, terrain and the manifest
/// after it. The shape where "one shard" and "the whole set" are the same thing is the one where an
/// off-by-one in the hand-off would hide.
#[test]
fn a_single_file_assembly_hands_its_one_shard_over_too() {
    let mut hooks = Evicting::default();
    let out = assemble_fixture(&options(), &mut hooks);
    let want = expected("expected");
    assert_eq!(hooks.taken.len(), 1);
    assert_eq!(hooks.taken[0].0, want[0].0);
    assert_same_bytes(&hooks.taken[0].3, &want[0].1, &want[0].0);
    assert_eq!(out.files.iter().map(|f| f.role).collect::<Vec<_>>(), vec!["terrain", "manifest"]);
    for (got, (name, bytes)) in out.files.iter().zip(want.iter().skip(1)) {
        assert_eq!(&got.name, name);
        assert_same_bytes(&got.bytes, bytes, name);
    }
}

/// Handing a shard **back** (`Ok(Some(_))`, the trait's default) is not a half-measure: the set comes
/// out exactly as it does with no hand-off at all. This is what makes the seam safe to add to a
/// caller that only wants to *watch* the files go by.
#[test]
fn a_shard_handed_back_stays_in_the_set() {
    let opts = BridgeOptions { force_split: true, ..options() };
    let mut hooks = Evicting { keep: true, ..Default::default() };
    let out = assemble_fixture(&opts, &mut hooks);
    let want = expected("expected-split");
    assert_eq!(hooks.taken.len(), 3, "it still saw every shard");
    assert_eq!(out.files.len(), want.len(), "…and kept every one of them");
    for (got, (name, bytes)) in out.files.iter().zip(&want) {
        assert_eq!(&got.name, name);
        assert_same_bytes(&got.bytes, bytes, name);
    }
}

/// A caller that never asks for shards is never handed one, however the rest of its hooks look. The
/// two halves are one decision, and the default is the old behaviour — which is what keeps every
/// other test in this file, and every native caller, on the path they were written for.
#[test]
fn a_caller_that_does_not_ask_is_handed_nothing() {
    struct Silent(std::rc::Rc<std::cell::Cell<usize>>);
    impl Hooks for Silent {
        fn now_us(&mut self) -> u64 {
            0
        }
        fn progress(&mut self, _phase: Phase, _fraction: f64) -> bool {
            false
        }
        // Deliberately overridden *without* `wants_shards`: the driver must still never call it.
        fn take_shard(&mut self, shard: OutputFile) -> Result<Option<OutputFile>, String> {
            self.0.set(self.0.get() + 1);
            Ok(Some(shard))
        }
    }
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let opts = BridgeOptions { force_split: true, ..options() };
    let out = assemble_fixture(&opts, &mut Silent(calls.clone()));
    assert_eq!(calls.get(), 0, "take_shard was called without wants_shards");
    assert_eq!(out.files.len(), expected("expected-split").len(), "the whole set is still here");
}

/// A sink that refuses a shard fails the **run**, as `io` and with its own words. It must not be
/// swallowed: the bytes are already gone by then, so continuing would finish a set with a hole in it
/// and report it as a map.
#[test]
fn a_sink_that_refuses_a_shard_fails_the_run_as_io() {
    let opts = BridgeOptions { force_split: true, ..options() };
    let mut hooks = Evicting { fail_at: Some(1), ..Default::default() };
    let e = assemble_everything(
        cells(),
        Vec::new(),
        Some(terrain_lattice()),
        terrain_cells(),
        &sidecar(),
        &skin(),
        &opts,
        &mut hooks,
    )
    .expect_err("the sink refused the second shard");
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
    assert!(e.message.contains("the card is full"), "the sink's own message must survive: {}", e.message);
    // The first shard was taken before the refusal — which is the caller's to clean up, and is why
    // the message says so rather than claiming nothing was written.
    assert_eq!(hooks.taken.len(), 1);
}

/// A cancelled run may already have handed shards out — the property a UI's cleanup path is written
/// against. It is safe because of §5.4, not because nothing was written: the OBCS manifest is last,
/// so however many `.OBM` files a cancelled run left behind, none of them is a map to a device.
#[test]
fn a_cancelled_run_may_already_have_handed_shards_out() {
    let opts = BridgeOptions { force_split: true, ..options() };
    let mut hooks = Evicting { abort_after_taken: Some(1), ..Default::default() };
    let e = assemble_everything(
        cells(),
        Vec::new(),
        Some(terrain_lattice()),
        terrain_cells(),
        &sidecar(),
        &skin(),
        &opts,
        &mut hooks,
    )
    .expect_err("cancelled");
    assert_eq!(e.code, ErrorCode::Aborted, "{}", e.message);
    assert!(e.message.contains("manifest is written last"), "{}", e.message);
    assert!(!hooks.taken.is_empty(), "this fixture no longer cancels after a shard has been handed over");
    // Nothing was handed over *after* the cancellation: every store entry point reads the abort flag
    // before it hands anything out.
    assert!(hooks.taken.len() < 3);
}

/// Same inputs, same bytes — twice, in one process. The engine renumbers nav nodes and re-bins POIs
/// through hash maps; a run-order dependency in either would show up here as two different files
/// from one fixture.
#[test]
fn two_runs_produce_identical_bytes() {
    let a = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("run one");
    let b = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("run two");
    for (x, y) in a.files.iter().zip(&b.files) {
        assert_same_bytes(&x.bytes, &y.bytes, &format!("{} across two runs", x.name));
    }
    assert_eq!(a.files[0].sha256, b.files[0].sha256);
}

// --- the §1.2 gaps ------------------------------------------------------------------------------
//
// Everything above compares the bridge's bytes against the CLI's, which is what a *drift* guard is
// for — and it is exactly why it cannot see a filler mistake. Both sides are the same engine, so a
// run that wrote its gaps as zeros, or left one out and slid every structure behind it down, would
// agree with itself and with a freshly regenerated fixture, and every offset in the file would still
// resolve. `OBCM_Spec.md` §1.2 says the gaps are part of the file and two bakes agree on them or
// they do not agree at all, so they get a pin of their own: an independent walk of the finished
// shard that names each gap the layout implies and reads its bytes.

/// The unit every offset in a shard this engine writes counts (§1.1), and the fill byte (§1.2).
const UNIT: usize = 16;
const FILLER: u8 = 0xFF;

/// `align_up(x, U)`.
fn align_up(x: usize) -> usize {
    x.next_multiple_of(UNIT)
}

fn le32(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes")) as usize
}

/// A scaled offset field, resolved (§1.1: widen, then multiply).
fn offset(bytes: &[u8], at: usize) -> usize {
    le32(bytes, at) * UNIT
}

/// Assert `bytes[from..to]` is `0xFF`, and say which gap it was when it is not.
#[track_caller]
fn assert_filler(bytes: &[u8], from: usize, to: usize, what: &str) {
    assert!(from <= to && to <= bytes.len(), "{what}: the gap {from}..{to} is not inside a {}-byte file", bytes.len());
    for (i, &b) in bytes[from..to].iter().enumerate() {
        assert_eq!(b, FILLER, "{what}: byte {} of the gap is 0x{b:02x}, not §1.2's 0xFF", from + i);
    }
}

/// Every gap `OBCM_Spec.md` §1.2 puts in one shard, walked from the file's own directories.
///
/// Two kinds of mistake this catches that a byte-for-byte comparison against the CLI cannot: a gap
/// written with the wrong fill (the reader never looks at these bytes, so nothing else notices), and
/// a gap that is not there at all (every offset behind it simply moves, and the file stays
/// self-consistent). The counters at the end are what stop the walk being vacuous — a fixture whose
/// structures happened to land on unit boundaries would exercise nothing.
fn assert_section_gaps(shard: &[u8]) -> (usize, usize) {
    let mut gaps = 0usize; // region and section boundaries actually filled
    let mut padded_chunks = 0usize; // §5.1 chunks whose content ended mid-unit
    let mut gap = |from: usize, to: usize, what: &str| {
        assert_filler(shard, from, to, what);
        gaps += (to > from) as usize;
    };

    // §1: the 49-byte header, then the run to the style table's boundary.
    assert_eq!(shard[4], 14, "the version byte this walk is written against");
    assert_eq!(shard[40], 4, "`Offset Scale`, so U = 16");
    assert_eq!(&shard[41..49], &[0u8; 8], "a set's raster is its own file, so §1.3's pair is (0, 0)");
    let style_at = offset(shard, 21);
    assert_eq!(style_at, 64, "the style table starts at align_up(49)");
    gap(49, style_at, "header → style table");

    // §2 → §3: the style table's own tail, and the LOD table's.
    let lod_table_at = offset(shard, 26);
    let style_end = style_at + 1 + shard[style_at] as usize * 8;
    gap(style_end, lod_table_at, "style table → LOD table");
    let lod_count = shard[25] as usize;
    let lod_table_end = lod_table_at + lod_count * 18;

    // §3/§5.1: per LOD, the rounding step between the offset table and `data_start`, and the run
    // behind every chunk's `0xFF` sentinel.
    let mut previous_end = lod_table_end;
    for i in 0..lod_count {
        let entry = lod_table_at + i * 18;
        let index_at = offset(shard, entry + 4);
        assert_eq!(index_at % UNIT, 0, "LOD {i}: a scaled `Index Offset` cannot name a non-boundary");
        gap(previous_end, index_at, &format!("→ LOD {i}'s index"));
        let (node_count, chunk_count) = (le32(shard, entry + 8), le32(shard, entry + 14));
        let chunk_size = u16::from_le_bytes(shard[entry + 12..entry + 14].try_into().unwrap()) as usize;
        let table_at = index_at + node_count * 4;
        let table_end = table_at + (chunk_count + 1) * 4;
        let data_start = align_up(table_end);
        gap(table_end, data_start, &format!("LOD {i}: offset table → data_start"));
        for k in 0..chunk_count {
            let (from, to) = (offset(shard, table_at + k * 4), offset(shard, table_at + (k + 1) * 4));
            assert!(to - from <= align_up(chunk_size), "LOD {i} chunk {k}: span past §5.1's bound");
            // The chunk's content ends at its one sentinel; from there to the unit boundary is
            // filler, so the run of `0xFF` at the end is `1 + (0..U-1)`. A writer that padded with
            // zeros leaves a run of exactly one, and the last byte of the span is not `0xFF`.
            let end = data_start + to;
            let run = shard[data_start + from..end].iter().rev().take_while(|&&b| b == FILLER).count();
            assert!(run >= 1, "LOD {i} chunk {k}: no `0xFF` sentinel at the end of the span");
            assert!(run <= UNIT, "LOD {i} chunk {k}: {run} trailing 0xFF, more than a sentinel plus one unit");
            padded_chunks += (run > 1) as usize;
        }
        previous_end = data_start + offset(shard, table_at + chunk_count * 4);
    }

    // §7.1: the directory's tail, each category's index → chunks step, and the section's own end.
    let poi_at = offset(shard, 32);
    gap(previous_end, poi_at, "last LOD → POI section");
    let categories = shard[poi_at] as usize;
    let poi_chunk_size = u16::from_le_bytes(shard[poi_at + 1..poi_at + 3].try_into().unwrap()) as usize;
    let dir_end = poi_at + 1 + 2 + categories * 13 + 4 + 2;
    gap(dir_end, align_up(dir_end), "POI directory → first category");
    for c in 0..categories {
        let entry = poi_at + 3 + c * 13;
        let index_at = offset(shard, entry + 1);
        let (node_count, chunk_count) = (le32(shard, entry + 5), le32(shard, entry + 9));
        let index_end = index_at + node_count * 4;
        gap(index_end, align_up(index_end), &format!("POI category {}: index → chunks", c + 1));
        // 512 is a multiple of `U` at every legal scale, so a category's chunk run carries none.
        assert_eq!(align_up(index_end) % UNIT, 0);
        assert_eq!(poi_chunk_size % UNIT, 0, "the fixed POI stride needs no filler inside the run");
        let _ = chunk_count;
    }
    let pool_at = offset(shard, dir_end - 6);
    let pool_end = pool_at + 2 + u16::from_le_bytes(shard[dir_end - 2..dir_end].try_into().unwrap()) as usize * 29;
    let nav_at = offset(shard, 36);
    gap(pool_end, nav_at, "hours pool → nav section");

    // §8.1: the eight bytes behind the 40-byte directory, the alignment run that lands the node
    // chunks on a sector, and the rounding step between the index and those chunks. **All of it is
    // `0xFF` since v14** — v13 wrote zeros for the alignment runs, which is precisely the change no
    // offset in this file can see.
    gap(nav_at + 40, nav_at + 48, "nav directory → profile table");
    let profile_end = nav_at + 48 + shard[nav_at + 26] as usize * 56;
    let index_at = offset(shard, nav_at);
    let node_count = le32(shard, nav_at + 4);
    gap(profile_end, index_at, "profile table → node index (§8.1's alignment run)");
    let index_end = index_at + node_count * 4;
    let chunks_at = align_up(index_end);
    gap(index_end, chunks_at, "node index → node chunks");
    if node_count > 0 {
        // §8.1's sector landing is a producer guarantee for a **populated** graph; the empty
        // section a non-core shard carries has no chunks to land, and its three zero-length regions
        // simply point at the first unit boundary past the profile table.
        assert_eq!(chunks_at % 512, 0, "…and the run put them on a sector, which is the point of it");
    } else {
        assert_eq!(index_at, align_up(profile_end), "an empty graph's regions are still nameable");
    }
    let pool_at = offset(shard, nav_at + 12);
    assert_eq!(pool_at, chunks_at + le32(shard, nav_at + 8) * 512);
    let snap_at = offset(shard, nav_at + 28);
    let snap_nodes = le32(shard, nav_at + 32);
    gap(pool_at + le32(shard, nav_at + 16) * 512, snap_at, "edge pool → snap index");
    gap(snap_at + snap_nodes * 4, align_up(snap_at + snap_nodes * 4), "snap index → snap chunks");
    (gaps, padded_chunks)
}

/// The gap walk over the fixture's own core shard.
#[test]
fn every_section_boundary_of_the_assembled_shard_is_filler() {
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    let core = &out.files.iter().find(|f| f.role == "core").expect("a core shard").bytes;
    let (gaps, padded_chunks) = assert_section_gaps(core);
    // The fixture must actually *have* gaps, or the walk above asserts nothing. Both counters are
    // the two costs §1.2 quantifies separately: the per-region ones, and the per-chunk ones.
    assert!(gaps >= 8, "only {gaps} non-empty region gaps — this fixture no longer exercises §1.2");
    assert!(padded_chunks > 0, "no §5.1 chunk ended mid-unit, so the per-chunk filler is untested");
}

/// …and over every shard of the volume set, where a non-core shard's *empty* POI and nav sections
/// are the shape whose filler is easiest to forget: the regions are zero-length, so nothing but the
/// gaps is there at all.
#[test]
fn every_shard_of_a_volume_set_is_filled_the_same_way() {
    let opts = BridgeOptions { force_split: true, ..options() };
    let out = assemble_fixture(&opts, &mut NoHooks);
    for file in out.files.iter().filter(|f| f.name.ends_with(".OBM")) {
        let (gaps, _) = assert_section_gaps(&file.bytes);
        assert!(gaps >= 4, "{}: only {gaps} non-empty region gaps", file.name);
    }
}

/// **`Edge Id` is a `(chunk, ordinal)` pair, not a byte offset** (§8.4). The two agree on the first
/// record of the first chunk and nowhere else, so a fixture with several edges in one chunk is what
/// tells them apart: under v14 the ids of a nine-edge single-chunk pool are `0..=8`, where the v13
/// byte offsets ran to the hundreds.
#[test]
fn the_edge_ids_the_merge_mints_are_chunks_and_ordinals() {
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    let core = &out.files.iter().find(|f| f.role == "core").expect("a core shard").bytes;
    let nav_at = offset(core, 36);
    assert_eq!(le32(core, nav_at + 16), 1, "the fixture's whole edge pool is one 512-byte chunk");

    // Every `Edge Id` in the §8.3 adjacency, read off the node chunks.
    let chunks_at = align_up(offset(core, nav_at) + le32(core, nav_at + 4) * 4);
    let mut ids: Vec<u32> = Vec::new();
    for k in 0..le32(core, nav_at + 8) {
        let chunk = &core[chunks_at + k * 512..][..512];
        let mut at = 0usize;
        while at + 13 <= 512 && chunk[at + 12] != 0xFF {
            let degree = chunk[at + 12] as usize;
            for n in 0..degree {
                ids.push(u32::from_le_bytes(chunk[at + 13 + n * 17 + 8..][..4].try_into().unwrap()));
            }
            at += 13 + degree * 17;
        }
    }
    assert!(!ids.is_empty(), "the fixture's graph has adjacency to read");
    ids.sort_unstable();
    ids.dedup();
    // Chunk 0, so the id *is* the ordinal — dense from zero, one per record. A byte-offset id would
    // start at 0 and then jump by the record widths (19 bytes and up).
    assert_eq!(ids, (0..ids.len() as u32).collect::<Vec<u32>>(), "the ids are ordinals, not byte offsets");
    assert!(ids.len() > 1, "one edge would not tell an ordinal from an offset");
    for id in &ids {
        assert_eq!(id >> 5, 0, "chunk half");
        assert!((id & 0x1F) < 31, "§8.4 caps a chunk at 31 records so 0xFFFFFFFF stays impossible");
    }
}

/// …and the cells' *arrival order* must not reach the bytes either. The builder downloads cells
/// concurrently, so the order they are handed over is whatever the network decided.
#[test]
fn the_order_cells_arrive_in_does_not_reach_the_output() {
    let want = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("in sidecar order");
    let mut shuffled = cells();
    shuffled.reverse();
    let got = assemble_cells(shuffled, &sidecar(), &skin(), &options(), &mut NoHooks).expect("in reverse order");
    for (a, b) in got.files.iter().zip(&want.files) {
        assert_same_bytes(&a.bytes, &b.bytes, &format!("{} with the cells reversed", a.name));
    }
}

/// The recorded (phase, fraction) stream, plus a clock that ticks **once per call** — so a
/// `phases_us` figure in the summary is literally the number of times the engine read the clock
/// during that phase, which is what `the_clock_is_read_exactly_once_per_phase_boundary` pins.
#[derive(Default)]
struct Recorder {
    ticks: u64,
    seen: Vec<(Phase, f64)>,
    /// Abort on the `n`-th report of this phase (1 = the phase boundary itself).
    abort_at: Option<(Phase, usize)>,
}

impl Recorder {
    /// Abort the moment `phase` is first reported — its boundary callback.
    fn aborting_at(phase: Phase) -> Recorder {
        Recorder { abort_at: Some((phase, 1)), ..Default::default() }
    }

    /// The fractions reported for `phase`, in order.
    fn fractions(&self, phase: Phase) -> Vec<f64> {
        self.seen.iter().filter(|(p, _)| *p == phase).map(|(_, f)| *f).collect()
    }
}

impl Hooks for Recorder {
    fn now_us(&mut self) -> u64 {
        self.ticks += 1;
        self.ticks
    }
    fn progress(&mut self, phase: Phase, fraction: f64) -> bool {
        self.seen.push((phase, fraction));
        match self.abort_at {
            Some((p, n)) => p == phase && self.seen.iter().filter(|(q, _)| *q == p).count() == n,
            None => false,
        }
    }
}

/// **The phase-seam pin.** The bridge names phases by counting the engine's own clock calls (see
/// `driver`'s module header), which is a contract with `obcm_assemble::assemble`'s internals that
/// nothing else enforces. If the engine gains or loses a phase, this test fails — instead of a
/// progress bar that says "nav" while the verify pass runs.
#[test]
fn the_phase_sequence_is_the_one_the_engine_calls() {
    let mut rec = Recorder::default();
    assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut rec).expect("the assembly runs");

    let mut order: Vec<Phase> = Vec::new();
    for (p, _) in &rec.seen {
        if order.last() != Some(p) {
            order.push(*p);
        }
    }
    assert_eq!(
        order,
        vec![
            Phase::Open,
            Phase::Poi,
            Phase::Nav,
            Phase::Plan,
            Phase::Write,
            Phase::Verify,
            Phase::Manifest,
            Phase::Done
        ],
        "the engine's clock + store call sequence no longer maps onto these phases"
    );
    // A progress bar may never go backwards, and it must end at 1.0.
    let mut last = -1.0;
    for (p, f) in &rec.seen {
        assert!(*f >= last, "{p:?} reported {f}, behind {last}");
        assert!((0.0..=1.0).contains(f), "{p:?} reported {f}");
        last = *f;
    }
    assert_eq!(rec.seen.last().expect("progress was reported").1, 1.0);
}

/// **The tick-count pin**, and the reason the sequence test above is not enough on its own.
///
/// The phase names are read off a *count* of clock calls, so an innocent extra `clock.now_us()`
/// anywhere in the engine shifts every mapping by one boundary — `Poi` would be announced while
/// `open` is still running — while leaving the deduplicated order this file already asserts exactly
/// as it was. That mutation is invisible to the sequence test and fails here, because the counting
/// clock makes each `phases_us` figure the number of clock reads that phase contains.
///
/// One read per boundary, four per shard (write start/end, verify start/end), one final total:
/// `5 + 4·shards + 1`. A single-file assembly is 10.
#[test]
fn the_clock_is_read_exactly_once_per_phase_boundary() {
    let mut rec = Recorder::default();
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut rec).expect("the assembly runs");
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    let us = &s["phases_us"];
    // Each of these is `t_next − t_this` over a clock that advances by exactly 1 per read: a 2
    // anywhere means the engine now reads the clock twice in that phase, and every phase name this
    // bridge reports after it is off by one.
    for phase in ["open", "poi", "nav", "plan", "write", "verify"] {
        assert_eq!(us[phase], 1, "{phase} spans {} clock reads, not 1 — the phase mapping has shifted", us[phase]);
    }
    assert_eq!(us["total"], 9, "the whole run spans 9 clock reads (10 reads, first to last)");
    assert_eq!(rec.ticks, 10, "the engine read the clock {} times, not the 10 the phase mapping assumes", rec.ticks);
}

/// **The verify-progress pin.** §4.8 is 43 % of a measured region-scale run (11.4 s of
/// baden-württemberg's 26.2 s, #1116's phase-D harness), and the engine makes exactly *one* store call for
/// the whole pass — so a bar driven by store calls alone reaches its write-phase maximum and then
/// freezes for two fifths of the wait. `VerifySource::read_at` is what stops that, and this is the
/// test that says so: the pass reports many times, strictly forward, over a wide span of the bar.
///
/// The inverted form is the shipped-then-fixed defect: before the wrapper, the write's own emits ran
/// to a fraction of **1.0** and every one after it repeated it. Both halves are asserted.
#[test]
fn the_verify_pass_reports_its_own_progress_instead_of_freezing_the_bar() {
    let mut rec = Recorder::default();
    assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut rec).expect("the assembly runs");

    // Nothing before the read-back may claim the run is nearly done. The write phase runs from 0.167
    // to at most 0.203 + 0.363 = 0.566 of the bar by construction (`Phase::weight`); the defect this
    // pins had it arriving at 1.0.
    for (p, f) in rec.seen.iter().take_while(|(p, _)| *p != Phase::Verify) {
        assert!(*f <= 0.57, "{p:?} reported {f} before verify even started — the bar is spending verify's budget");
    }

    let verify = rec.fractions(Phase::Verify);
    assert!(verify.len() >= 8, "the §4.8 pass reported {} times; a bar needs more than that", verify.len());
    for pair in verify.windows(2) {
        assert!(pair[1] > pair[0], "verify reported {} after {} — the bar stalled or went backwards", pair[1], pair[0]);
    }
    let (first, last) = (verify[0], verify[verify.len() - 1]);
    // Verify's constructed sweep is its 0.434 weight times the bytes the real reader pulls back
    // over the input-byte projection. OBCM v13's sparse snap-anchor chunks make this tiny fixture
    // unusually input-heavy, so the observed sweep is ≈0.17 rather than v12's >0.20. Requiring
    // 0.15 still pins a visibly moving bar without pretending every padded byte is read.
    assert!(
        last - first > 0.15,
        "the §4.8 pass moved the bar from {first} to {last} — less than 15% of the whole run, for the phase that \
         is two fifths of the run"
    );
    // …and the boundaries still land where the phases say: verify opens where the write left the bar
    // (0.203 + the write term) and the manifest is the 1.0. This fixture's output is 0.61× its input
    // bytes — the projection both terms are measured against — so neither term reaches its full span
    // and the manifest closes the gap. At the scale the 1.00 ratio was measured on (#1116's harness
    // regions), output ≈ input and they do.
    assert!((0.39..=0.46).contains(&first), "verify started at {first}, not where the write phase ends");
    assert_eq!(rec.seen.last().expect("progress was reported"), &(Phase::Done, 1.0));
}

/// A truthy return from the progress callback stops the assembly, and it stops it as a
/// *cancellation* rather than as an I/O failure — the distinction a UI needs to decide between
/// "you cancelled" and "something broke".
#[test]
fn a_progress_callback_can_abort_the_run() {
    let mut rec = Recorder::aborting_at(Phase::Write);
    let e = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut rec).expect_err("aborted");
    assert_eq!(e.code, ErrorCode::Aborted);
    assert!(e.message.contains("manifest is written last"), "{}", e.message);
    // Nothing past the write was reported: the abort is honoured at the next store call.
    assert!(!rec.seen.iter().any(|(p, _)| *p == Phase::Done));
}

/// **The verify-abort pin**, and the sharper half of the same defect: with §4.8 making one store
/// call and reporting nothing, an abort armed anywhere inside it was a **no-op** — the run went on
/// to produce the whole set, so a cancel button pressed during the longest phase of the run did
/// nothing at all.
///
/// Both moments are checked: the boundary callback (`n = 1`, the review's own probe) and one from
/// inside the read loop (`n = 4`), which only the `read_at` poll can honour. And in both, the
/// failure must read as `aborted` — `verify_shard` turns any read refusal into `Error::Verify`, so
/// the naive mapping would tell the rider the assembler is broken because they pressed cancel.
#[test]
fn an_abort_armed_inside_the_verify_pass_stops_the_run() {
    for n in [1, 4] {
        let mut rec = Recorder { abort_at: Some((Phase::Verify, n)), ..Default::default() };
        let e = match assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut rec) {
            // The pre-fix behaviour, exactly: cancel during §4.8 and the full set is produced anyway.
            Ok(out) => panic!("cancelled at verify callback {n}, and it still produced {:?}", out.files),
            Err(e) => e,
        };
        assert_eq!(e.code, ErrorCode::Aborted, "cancelled at verify callback {n}: {}", e.message);
        assert!(e.message.contains("cancelled"), "{}", e.message);
        // The abort is honoured on the very next read: no manifest was ever asked for, so §5.4's
        // "a set with no manifest is not a map" holds and nothing is left half-usable.
        assert!(!rec.seen.iter().any(|(p, _)| matches!(p, Phase::Manifest | Phase::Done)));
        assert_eq!(rec.fractions(Phase::Verify).len(), n, "the pass kept reporting after it was told to stop");
    }
}

/// A `partial` cell is an OBCA §3.7 refusal unless the caller accepted it — an **input** problem,
/// which is the class that means "fix the selection", not "the assembler is broken".
#[test]
fn an_unaccepted_partial_cell_is_an_input_refusal() {
    let opts = BridgeOptions { accept_partial: false, ..options() };
    let e = assemble_cells(cells(), &sidecar(), &skin(), &opts, &mut NoHooks).expect_err("the coarse cell is partial");
    assert_eq!(e.code, ErrorCode::Input);
    assert!(e.message.contains("partial"), "{}", e.message);
}

/// A corrupt cell is a **format** refusal naming the cell — the download is broken, or the catalog
/// is serving something that is not a cell. Distinct from both of the above.
#[test]
fn a_corrupt_cell_is_a_format_refusal() {
    let mut cells = cells();
    cells[0].bytes[0] ^= 0xFF; // the OBCM magic
    let e = assemble_cells(cells, &sidecar(), &skin(), &options(), &mut NoHooks).expect_err("bad magic");
    assert_eq!(e.code, ErrorCode::Format);
    assert!(e.message.contains("readable OBCM"), "{}", e.message);
}

/// The summary crosses as the same document `obcm-assemble --json` prints, so a builder and an
/// operator read one thing. The fields P4c needs are pinned here.
#[test]
fn the_summary_is_the_clis_json() {
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    assert_eq!(s["cells"], 5);
    assert_eq!(s["manifest"], "MS1.OBS");
    assert_eq!(s["shards"][0]["role"], "core");
    assert_eq!(s["shards"][0]["file"], "MS1S00.OBM");
    // The §4.8 verify report is present and non-vacuous — the whole point of running verify in the
    // tab is that the caller can see it did.
    assert!(s["shards"][0]["verified"]["chunks"].as_u64().expect("a chunk count") > 0);
    assert!(s["shards"][0]["verified"]["features"].as_u64().expect("a feature count") > 0);
    // The fixture's seam is real: nav nodes were unified across it and an islet was pruned.
    assert!(s["nav"]["unified"].as_u64().expect("a unified count") > 0, "the fixture's seam must unify junctions");
    assert!(s["nav"]["pruned_nodes"].as_u64().expect("a prune count") > 0, "the fixture's islet must be pruned");
    assert_eq!(s["poi"]["records"], 4);
    assert_eq!(out.warnings, Vec::<String>::new(), "this fixture is clean; a warning here is a real finding");
}

/// The write and verify phases **interleave** — the engine writes shard *i*, verifies shard *i*,
/// then writes shard *i+1* — so a set is where a single write/verify counter would have run
/// backwards at every shard boundary. Two independent terms over one span is what stops it, and this
/// is the shape that would catch a regression to one.
#[test]
fn the_bar_stays_monotone_across_an_interleaved_volume_set() {
    let opts = BridgeOptions { force_split: true, ..options() };
    let mut rec = Recorder::default();
    assemble_cells(cells(), &sidecar(), &skin(), &opts, &mut rec).expect("the assembly runs");

    // The set really does interleave: write appears again after verify has already been reported.
    let phases: Vec<Phase> = rec.seen.iter().map(|(p, _)| *p).collect();
    let first_verify = phases.iter().position(|p| *p == Phase::Verify).expect("a verify phase");
    assert!(phases[first_verify..].contains(&Phase::Write), "this fixture no longer exercises the interleaving");

    let mut last = -1.0;
    for (p, f) in &rec.seen {
        assert!(*f >= last, "{p:?} reported {f} after {last} — the bar went backwards at a shard boundary");
        last = *f;
    }
    assert_eq!(rec.seen.last().expect("progress was reported"), &(Phase::Done, 1.0));
}

// --- the input cells, from outside wasm memory (#1116 B2) --------------------------------------

/// The fixture's cells as a browser has them once they live in OPFS: identities and lengths on this
/// side, bytes on the other side of a read callback.
///
/// It also *counts*, because the read seam's affordability is the whole argument for the block cache
/// and an assertion is the only thing that keeps that argument honest.
#[derive(Default)]
struct Stored {
    blobs: Vec<Vec<u8>>,
    /// One entry per host read: `(slot, offset, length)`.
    reads: std::cell::RefCell<Vec<(usize, u32, usize)>>,
    /// Refuse every read of this slot that *reaches* this byte, as a closed storage handle does.
    /// Stated as a byte rather than as a request offset because the block cache decides how the
    /// engine's reads are grouped, and the test is about the byte the host cannot serve.
    fail_at: Option<(usize, u32)>,
}

impl Stored {
    fn new() -> Stored {
        Stored { blobs: cells().into_iter().map(|c| c.bytes).collect(), ..Default::default() }
    }

    fn failing_at(slot: usize, offset: u32) -> Stored {
        Stored { fail_at: Some((slot, offset)), ..Stored::new() }
    }

    /// The identities, in the sidecar's order — slot `i` is `blobs[i]`.
    fn source_cells(&self) -> Vec<SourceCell> {
        cells()
            .iter()
            .zip(&self.blobs)
            .map(|(c, bytes)| SourceCell {
                id: c.id.clone(),
                band: c.band.clone(),
                partial: c.partial,
                byte_length: bytes.len() as u32,
                key: format!("{:02x}{:02x}-key", bytes[0], bytes[1]),
            })
            .collect()
    }

    /// This store's cells, with the raster beside them — the same `Wiring` every test here builds.
    fn wiring(&self) -> Wiring<'_> {
        Wiring {
            source_cells: self.source_cells(),
            reads: Some(self),
            terrain: Some(terrain_lattice()),
            terrain_cells: terrain_cells(),
            ..Wiring::default()
        }
    }
}

impl CellReads for Stored {
    fn read(&self, slot: usize, offset: u64, buf: &mut [u8]) -> Result<(), String> {
        self.reads.borrow_mut().push((slot, offset as u32, buf.len()));
        if let Some((s, at)) = self.fail_at {
            if slot == s && offset as usize + buf.len() > at as usize {
                return Err("the storage handle is closed".into());
            }
        }
        let blob = self.blobs.get(slot).ok_or_else(|| format!("no cell in slot {slot}"))?;
        let start = offset as usize;
        let want = blob.get(start..start + buf.len()).ok_or_else(|| format!("slot {slot} has no byte {start}"))?;
        buf.copy_from_slice(want);
        Ok(())
    }
}

/// **The B2 pin**: an assembly whose cells were never copied into wasm memory produces the same
/// bytes as one whose cells were. The read seam and its block cache are plumbing, and plumbing that
/// changes the output is a defect rather than a trade-off.
#[test]
fn cells_read_through_the_host_produce_the_native_clis_bytes() {
    let store = Stored::new();
    let out = assemble(store.wiring(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    let want = expected("expected");
    assert_eq!(out.files.len(), want.len(), "file count: {:?} vs {:?}", out.files, want);
    for (got, (name, bytes)) in out.files.iter().zip(&want) {
        assert_eq!(&got.name, name);
        assert_same_bytes(&got.bytes, bytes, name);
    }
    // Every cell really did come through the seam — a path that quietly found the bytes elsewhere
    // would pass the comparison above and prove nothing.
    let slots: std::collections::HashSet<usize> = store.reads.borrow().iter().map(|(s, _, _)| *s).collect();
    assert_eq!(slots.len(), store.blobs.len(), "every source cell must have been read through the seam");
    // …and never past its declared length, which is the cache's own bounds check rather than the
    // host's: a catalog byte count is what the engine reads as the cell's size.
    for (slot, offset, len) in store.reads.borrow().iter() {
        assert!(*offset as usize + *len <= store.blobs[*slot].len(), "read {offset}+{len} past the end of slot {slot}");
    }
}

/// …and the same for a volume set, where §2.3's 256 KiB verbatim geometry copies (which go around
/// the cache) run beside §4.6.6's per-record nav emission (which is the reason it exists).
#[test]
fn a_volume_set_assembles_the_same_from_host_reads() {
    let store = Stored::new();
    let opts = BridgeOptions { force_split: true, ..options() };
    let out = assemble(store.wiring(), &sidecar(), &skin(), &opts, &mut NoHooks).expect("the assembly runs");
    let want = expected("expected-split");
    assert_eq!(out.files.len(), want.len(), "file count: {:?} vs {:?}", out.files, want);
    for (got, (name, bytes)) in out.files.iter().zip(&want) {
        assert_eq!(&got.name, name);
        assert_same_bytes(&got.bytes, bytes, name);
    }
}

/// The block cache is **transparent and worth having**: the same bytes with it on or off, and two
/// orders of magnitude fewer host reads with it on.
///
/// A block size of `1` is the cache switched off — every read takes the bypass — so the first count
/// is literally the number of reads the engine makes, and the second is what the host is actually
/// asked for. That ratio is the number the browser path lives or dies on: every host read is a JS
/// crossing *and* an OPFS syscall, and §4.6.6 makes one engine read per nav record — 17.5 M of them
/// at country scale (#1116 C3's measurement), which is not a thing to cross a language boundary for.
#[test]
fn the_read_block_size_changes_the_call_count_and_not_the_bytes() {
    let run = |block: usize| {
        let store = Stored::new();
        let opts = BridgeOptions { read_block_bytes: block, ..options() };
        let out = assemble(store.wiring(), &sidecar(), &skin(), &opts, &mut NoHooks).expect("the assembly runs");
        let reads = store.reads.borrow().len();
        (out.files.into_iter().map(|f| (f.name, f.bytes)).collect::<Vec<_>>(), reads)
    };
    let (uncached, engine_reads) = run(1);
    let (cached, host_reads) = run(64 * 1024);
    assert_eq!(uncached.len(), cached.len());
    for ((name, a), (_, b)) in uncached.iter().zip(&cached) {
        assert_same_bytes(a, b, &format!("{name} with the read cache off"));
    }
    eprintln!("host reads: {engine_reads} with the cache off, {host_reads} at 64 KiB blocks");
    // 30× on a 20 KB fixture, where most regions already fit one read; the ratio a country's cells
    // see is the one in the module header, because it is the per-record walks that grow with size.
    assert!(host_reads * 10 < engine_reads, "the block cache saved only {engine_reads} → {host_reads} host reads");
}

/// A cell that cannot be read is [`ErrorCode::Io`] **naming the cell**, not a §4.8 verify defect and
/// not a panic across the FFI boundary. The engine's own report would blame the format — a failed
/// read inside `Cell::open` comes back as "not a readable OBCM", a true sentence about the wrong
/// thing, because the browser's storage failed and the catalog did not.
///
/// Both cases matter. With the whole cell in one block the failure lands in `Cell::open`, where the
/// engine's own class is `Format`; with 512-byte blocks the cell opens and the failure lands
/// somewhere in the middle of a later phase, where it is `Io` or `Verify` depending on who was
/// reading. All three must reach the caller as `io`, with the same sentence.
#[test]
fn a_cell_the_host_cannot_read_fails_as_io_naming_the_cell() {
    for (block, at) in [(64 * 1024usize, 0u32), (512, 1024)] {
        let store = Stored::failing_at(1, at);
        let named = store.source_cells()[1].id.clone();
        let opts = BridgeOptions { read_block_bytes: block, ..options() };
        let e = assemble(store.wiring(), &sidecar(), &skin(), &opts, &mut NoHooks).expect_err("slot 1 is unreadable");
        assert_eq!(e.code, ErrorCode::Io, "blocks of {block}, failing at {at}: {}", e.message);
        assert!(e.message.contains(&named), "the message must name the cell: {}", e.message);
        assert!(e.message.contains("the storage handle is closed"), "the host's own words: {}", e.message);
    }
}

/// A key with no way to resolve it is a half-wired host, and it is refused before a byte is read —
/// as `internal`, because it is a defect in the caller rather than anything about the selection.
#[test]
fn cells_handed_over_by_key_without_a_reader_are_refused() {
    let store = Stored::new();
    let e = assemble(
        Wiring { source_cells: store.source_cells(), reads: None, ..Wiring::default() },
        &sidecar(),
        &skin(),
        &options(),
        &mut NoHooks,
    )
    .expect_err("there is no way to fetch these");
    assert_eq!(e.code, ErrorCode::Internal, "{}", e.message);
    assert!(e.message.contains("no read callback"), "{}", e.message);
}

// --- the output shards, outside wasm memory (#1116 D1) ------------------------------------------

/// One slot's file on the host's own storage.
#[derive(Default, Clone)]
struct DiskFile {
    /// The derived §5.2 name the driver announced at `create`. Recorded rather than used: the
    /// browser writes into a pre-opened scratch file and saves it under this name afterwards, so a
    /// sink that quietly disagreed with the manifest would be invisible without checking it here.
    name: String,
    bytes: Vec<u8>,
    sealed: bool,
}

/// The host's own storage, as this crate sees it through [`ShardWrites`]: one file per slot, plus
/// the three things a real disk does that a `Vec<u8>` in the same process never would — refuse a
/// write, refuse a read, and hand back bytes that are not the ones it was given.
#[derive(Default)]
struct Disk {
    files: std::cell::RefCell<Vec<DiskFile>>,
    /// Refuse `write` once this many bytes have been accepted for that slot — a disk filling up
    /// mid-shard.
    refuse_write_after: Option<(usize, usize)>,
    /// Refuse every `read_at` of this slot: a handle closed under the §4.8 read-back's feet.
    refuse_reads: Option<usize>,
    /// Flip one byte of a slot at `seal`, **behind the driver's back**. Nothing in this process ever
    /// sees the change: this crate's digest was taken from the bytes on the way in, and the engine's
    /// from the same bytes by its own path. Only a §4.8 pass that genuinely re-reads the file can
    /// notice.
    corrupt: Option<(usize, usize)>,
    /// How many times the host was asked for bytes — the read-back's own crossing count.
    reads: std::cell::Cell<usize>,
}

impl Disk {
    fn slot(&self, slot: usize) -> Result<std::cell::RefMut<'_, DiskFile>, String> {
        let files = self.files.borrow_mut();
        if slot >= files.len() {
            return Err(format!("there is no slot {slot}"));
        }
        Ok(std::cell::RefMut::map(files, |f| &mut f[slot]))
    }

    /// What is on the host's storage, in slot order — the set as a rider would find it.
    fn written(&self) -> Vec<(String, Vec<u8>)> {
        self.files.borrow().iter().map(|f| (f.name.clone(), f.bytes.clone())).collect()
    }
}

impl ShardWrites for Disk {
    fn create(&self, slot: usize, name: &str) -> Result<(), String> {
        let mut files = self.files.borrow_mut();
        if files.len() <= slot {
            files.resize(slot + 1, DiskFile::default());
        }
        files[slot] = DiskFile { name: name.to_string(), bytes: Vec::new(), sealed: false };
        Ok(())
    }

    fn write(&self, slot: usize, bytes: &[u8]) -> Result<(), String> {
        let mut file = self.slot(slot)?;
        if let Some((s, after)) = self.refuse_write_after {
            if slot == s && file.bytes.len() + bytes.len() > after {
                return Err("the disk is full".into());
            }
        }
        assert!(!file.sealed, "slot {slot} was written after it was sealed");
        file.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn read_at(&self, slot: usize, offset: u64, into: &mut [u8]) -> Result<(), String> {
        self.reads.set(self.reads.get() + 1);
        if self.refuse_reads == Some(slot) {
            return Err("the storage handle is closed".into());
        }
        let file = self.slot(slot)?;
        assert!(file.sealed, "slot {slot} was read back before it was sealed");
        let at = offset as usize;
        let want = file.bytes.get(at..at + into.len()).ok_or_else(|| format!("slot {slot} has no byte {at}"))?;
        into.copy_from_slice(want);
        Ok(())
    }

    fn seal(&self, slot: usize) -> Result<(), String> {
        let mut file = self.slot(slot)?;
        file.sealed = true;
        if let Some((s, at)) = self.corrupt {
            if slot == s {
                file.bytes[at] ^= 0xff;
            }
        }
        Ok(())
    }
}

/// A caller whose shards are written by the host, recording what it was told it now has.
#[derive(Default)]
struct Sinking {
    sealed: Vec<SealedShard>,
    /// Refuse the `n`-th report, as a caller whose own bookkeeping failed.
    fail_at: Option<usize>,
}

impl Hooks for Sinking {
    fn now_us(&mut self) -> u64 {
        0
    }
    fn progress(&mut self, _phase: Phase, _fraction: f64) -> bool {
        false
    }
    fn shard_sealed(&mut self, shard: SealedShard) -> Result<(), String> {
        if self.fail_at == Some(self.sealed.len()) {
            return Err(format!("{} could not be recorded", shard.name));
        }
        self.sealed.push(shard);
        Ok(())
    }
}

/// The fixture assembled with its shards written through `disk` instead of into this address space.
fn assemble_to_disk(
    disk: &Disk,
    opts: &BridgeOptions,
    hooks: &mut dyn Hooks,
) -> Result<obc_web_assemble::Outcome, obc_web_assemble::AssembleFailure> {
    assemble(
        Wiring {
            cells: cells(),
            terrain: Some(terrain_lattice()),
            terrain_cells: terrain_cells(),
            sink: Some(disk),
            ..Wiring::default()
        },
        &sidecar(),
        &skin(),
        opts,
        hooks,
    )
}

/// **The D1 pin**: a volume set whose shards were never in this address space is the same set — the
/// same files, the same names, the same digests, byte for byte — as the one the native CLI wrote in
/// one piece.
///
/// This is the claim the whole phase rests on. The core shard cannot be split (one nav graph, one
/// file), so at DACH scale it is a ~3 GiB allocation in a 4 GiB address space and the *only* answer
/// is that it is not an allocation at all. The seam that makes that true must contribute no format
/// knowledge, and this is where that is checked.
#[test]
fn shards_written_through_the_sink_are_the_native_clis_volume_set() {
    let disk = Disk::default();
    let mut hooks = Sinking::default();
    let opts = BridgeOptions { force_split: true, ..options() };
    let out = assemble_to_disk(&disk, &opts, &mut hooks).expect("the assembly runs");
    let want = expected("expected-split");

    // Nothing shard-sized came back: the raster (which the engine writes through its own sink) and
    // the manifest are all that is left in wasm memory.
    assert_eq!(out.files.iter().map(|f| f.role).collect::<Vec<_>>(), vec!["terrain", "manifest"]);
    assert_eq!(hooks.sealed.iter().map(|s| s.role).collect::<Vec<_>>(), vec!["core", "coarse", "geometry"]);

    // What the host has, then what is left here, is the set — in order, name by name, byte by byte.
    let delivered: Vec<(String, Vec<u8>)> =
        disk.written().into_iter().chain(out.files.iter().map(|f| (f.name.clone(), f.bytes.clone()))).collect();
    assert_eq!(delivered.len(), want.len(), "file count: {:?} vs {}", delivered.iter().map(|f| &f.0), want.len());
    for ((name, bytes), (want_name, want_bytes)) in delivered.iter().zip(&want) {
        assert_eq!(name, want_name, "the derived filenames must match the CLI's (OBCA §5.2)");
        assert_same_bytes(bytes, want_bytes, name);
    }

    // …and what the caller was *told* it has matches what the engine says it wrote. The host saved
    // these bytes without ever seeing them, so this equality is the only thing between a mislabelled
    // file and a card.
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    for (i, sealed) in hooks.sealed.iter().enumerate() {
        assert_eq!(sealed.slot, i, "a shard's slot is its index in the set");
        assert_eq!(s["shards"][i]["file"], sealed.name.as_str());
        assert_eq!(s["shards"][i]["sha256"], sealed.sha256.as_str());
        assert_eq!(s["shards"][i]["bytes"].as_u64(), Some(sealed.byte_length));
        assert_eq!(sealed.byte_length as usize, want[i].1.len());
    }
}

/// …and the single-file fast path, where "one shard" and "the whole set" are the same thing — the
/// shape a country actually has, and the one an off-by-one in the sink would hide.
#[test]
fn a_single_file_assembly_writes_its_one_shard_through_the_sink() {
    let disk = Disk::default();
    let mut hooks = Sinking::default();
    let out = assemble_to_disk(&disk, &options(), &mut hooks).expect("the assembly runs");
    let want = expected("expected");
    let on_disk = disk.written();
    assert_eq!(on_disk.len(), 1);
    assert_eq!(on_disk[0].0, want[0].0);
    assert_same_bytes(&on_disk[0].1, &want[0].1, &want[0].0);
    assert_eq!(hooks.sealed.len(), 1);
    assert_eq!(out.files.iter().map(|f| f.role).collect::<Vec<_>>(), vec!["terrain", "manifest"]);
    for (got, (name, bytes)) in out.files.iter().zip(want.iter().skip(1)) {
        assert_eq!(&got.name, name);
        assert_same_bytes(&got.bytes, bytes, name);
    }
}

/// **The proof that §4.8 reads the file.** Flip one byte of a sealed shard behind the driver's
/// back — the sink's own storage changed, nothing in this process did — and the verify pass must
/// reject the set.
///
/// It is the test the buffered store could never pass. With the bytes in a `Vec`, "read the shard
/// back" and "look at the shard" are the same operation, so §4.8 proves the *encoder* agrees with
/// the *decoder* and nothing about the medium. With a sink the medium is the thing that can lie, and
/// a read-back that quietly answered out of an in-memory copy would ship a corrupt map with a clean
/// verdict. Byte 0 is the OBCM magic, so what fails is unmistakably the reader.
#[test]
fn a_shard_the_sink_corrupts_on_disk_fails_verify() {
    let disk = Disk { corrupt: Some((0, 0)), ..Disk::default() };
    let mut hooks = Sinking::default();
    let e = assemble_to_disk(&disk, &options(), &mut hooks).expect_err("the shard on disk is not the one written");
    assert_eq!(e.code, ErrorCode::Verify, "{}", e.message);
    // Nothing was reported as sealed and nothing was written after it: §4.8 is a precondition of the
    // manifest, so the corrupt shard never became part of a map (OBCA §5.4).
    assert!(hooks.sealed.is_empty(), "a shard that failed its read-back was reported as finished");
    assert!(disk.reads.get() > 0, "the read-back never asked the host for a byte");
}

/// A sink that cannot take a shard's bytes fails the **run**, as `io` and in the host's own words,
/// naming the file it was writing. Not `verify`: a full disk is not a defect in the assembler.
#[test]
fn a_sink_that_refuses_a_write_fails_the_run_as_io() {
    let disk = Disk { refuse_write_after: Some((0, 4096)), ..Disk::default() };
    let mut hooks = Sinking::default();
    let e = assemble_to_disk(&disk, &options(), &mut hooks).expect_err("the disk filled up");
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
    assert!(e.message.contains("the disk is full"), "the host's own words: {}", e.message);
    assert!(e.message.contains(".OBM"), "the message must name the shard: {}", e.message);
    assert!(hooks.sealed.is_empty());
}

/// …and a sink that cannot give them **back** is `io` too, although §4.8 is where it surfaces.
///
/// This is the same rule as `map_error`'s abort-first one, one seam over: `verify_shard` reports any
/// read failure as a §4.8 defect, so without the host's own message a closed handle would tell a
/// rider that the assembler wrote a set the reader cannot read — the one verdict the docs say never
/// to retry past.
#[test]
fn a_shard_the_sink_cannot_read_back_is_io_not_a_verify_defect() {
    let disk = Disk { refuse_reads: Some(0), ..Disk::default() };
    let e = assemble_to_disk(&disk, &options(), &mut NoHooks).expect_err("the read-back cannot read");
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
    assert!(e.message.contains("the storage handle is closed"), "the host's own words: {}", e.message);
    assert!(e.message.contains(".OBM"), "the message must name the shard: {}", e.message);
}

/// A caller that cannot record a finished shard stops the run as `io`. The file exists and its name
/// is now the only thing that could find it again, so finishing the set would report a map whose
/// files nobody wrote down.
#[test]
fn a_sealed_report_the_caller_refuses_fails_the_run_as_io() {
    let disk = Disk::default();
    let mut hooks = Sinking { fail_at: Some(1), ..Default::default() };
    let opts = BridgeOptions { force_split: true, ..options() };
    let e = assemble_to_disk(&disk, &opts, &mut hooks).expect_err("the caller refused the second shard");
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
    assert!(e.message.contains("could not be recorded"), "{}", e.message);
    assert_eq!(
        hooks.sealed.len(),
        1,
        "the first was recorded before the refusal — that one is the caller's to clean up"
    );
}

/// A cancel is a cancellation wherever it lands, sink or no sink: during the write, where it reaches
/// the store's own abort check, and inside the §4.8 read-back, where it must be seen **before** the
/// host is asked for a byte and must never be reported as a §4.8 defect.
#[test]
fn a_cancel_through_the_sink_path_is_still_a_cancellation() {
    for phase in [Phase::Write, Phase::Verify] {
        let disk = Disk::default();
        let mut rec = Recorder { abort_at: Some((phase, 2)), ..Default::default() };
        let e = assemble_to_disk(&disk, &options(), &mut rec).expect_err("cancelled");
        assert_eq!(e.code, ErrorCode::Aborted, "cancelled during {phase:?}: {}", e.message);
        assert!(e.message.contains("cancelled"), "{}", e.message);
        // §5.4 is what makes that safe: whatever the host has, none of it is a map without the
        // manifest, and the manifest was never asked for.
        assert!(!rec.seen.iter().any(|(p, _)| matches!(p, Phase::Manifest | Phase::Done)));
    }
}

/// The §4.8 read-back goes through the same block cache the input reads do, and for the same reason:
/// the pass walks a sealed shard a record at a time, and one host call per read is one file read and
/// one boundary crossing per record.
///
/// Transparency first — the same bytes either way — and then the count, because "with the cache"
/// only means something against "without".
#[test]
fn the_read_back_is_cached_and_the_cache_changes_no_bytes() {
    let run = |block: usize| {
        let disk = Disk::default();
        let opts = BridgeOptions { read_block_bytes: block, ..options() };
        assemble_to_disk(&disk, &opts, &mut NoHooks).expect("the assembly runs");
        (disk.written(), disk.reads.get())
    };
    let (uncached, engine_reads) = run(1);
    let (cached, host_reads) = run(64 * 1024);
    for ((name, a), (_, b)) in uncached.iter().zip(&cached) {
        assert_same_bytes(a, b, &format!("{name} with the read-back cache off"));
    }
    eprintln!("read-back: {engine_reads} host reads with the cache off, {host_reads} at 64 KiB blocks");
    assert!(host_reads * 10 < engine_reads, "the block cache saved only {engine_reads} → {host_reads} host reads");
}
