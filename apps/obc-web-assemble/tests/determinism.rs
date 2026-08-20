//! **The determinism pin**: the bridge's output is the native CLI's output, byte for byte, and it
//! is the same on every run.
//!
//! These are not "does the wrapper work" tests. They exist so that a change to the assembly engine —
//! the renumber tie-break, the layout, a hash-map iteration order that leaks into the output —
//! cannot ship a browser build that quietly disagrees with the command line. The inputs are the
//! checked-in cell tree in `tests/fixture/` and the expected outputs are what
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
    CellReads, ErrorCode, Hooks, KnownEmptyCell, MapWrites, NoHooks, Phase, SealedMap, SourceCell, TerrainCellBytes,
    TerrainLattice, Wiring,
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
/// canonically void, so it has no object (`OBCC_Spec.md` §13.6) and must reach the region as a `0`
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

/// The whole fixture assembly — cells **and** raster — which is what the CLI wrote
/// `expected/map.obcm` from.
fn assemble_fixture(opts: &BridgeOptions, hooks: &mut dyn Hooks) -> obc_web_assemble::Outcome {
    assemble_everything(cells(), Vec::new(), Some(terrain_lattice()), terrain_cells(), &sidecar(), &skin(), opts, hooks)
        .expect("the assembly runs")
}

/// The options the fixture's `expected/` was produced with (`--accept-partial`). The coarse `2^20`
/// cell is necessarily partial at this extract's size, which is why the flag is on rather than the
/// refusal being papered over.
fn options() -> BridgeOptions {
    BridgeOptions { accept_partial: true, ..BridgeOptions::default() }
}

/// One of the two files the native CLI left in `tests/fixture/expected/`: `map.obcm` (the raster
/// spliced into its §1.3 region) and `flat.obcm` (the same selection with no raster at all).
fn expected(name: &str) -> Vec<u8> {
    let path = fixture_dir().join("expected").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{} — see tests/fixture.rs: {e}", path.display()))
}

/// The bytes the run produced, whether it buffered them or the outcome only carries an identity.
fn taken(out: &obc_web_assemble::Outcome) -> &[u8] {
    out.bytes.as_deref().expect("this assembly buffered its map")
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

/// The headline: an assembly through the bridge **is** the file the CLI wrote.
#[test]
fn the_bridge_reproduces_the_native_clis_bytes() {
    let out = assemble_fixture(&options(), &mut NoHooks);
    let want = expected("map.obcm");
    assert_same_bytes(taken(&out), &want, "map.obcm");
    // The identity travels beside the bytes, and it is the identity of the whole file — raster
    // included, because the raster is part of the file now.
    assert_eq!(out.byte_length, want.len() as u64);
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    assert_eq!(s["sha256"], out.sha256.as_str());
    assert_eq!(s["bytes"].as_u64(), Some(out.byte_length));
}

// --- the §1.3 terrain region ---------------------------------------------------------------------

/// `OBCM_Spec.md` §1's header: the version, the offset scale, and the §1.3 region pair.
const HEADER_TERRAIN_OFFSET_AT: usize = 41;
const HEADER_TERRAIN_LEN_AT: usize = 45;

/// The map's spliced raster, as a device resolves it: the §1.3 window the header names.
fn terrain_window(map: &[u8]) -> Option<&[u8]> {
    let at = offset(map, HEADER_TERRAIN_OFFSET_AT);
    let len = offset(map, HEADER_TERRAIN_LEN_AT);
    // §1.3 makes `0` mean absence for both fields or neither; the engine's own header writer
    // refuses the mixed case, so a mixed pair here is a finding rather than a shape to handle.
    assert_eq!(at == 0, len == 0, "§1.3's pair is set together or not at all");
    (at != 0).then(|| &map[at..at + len])
}

/// **The terrain round trip** (EL4): the region the assembler spliced into the map's tail is a legal
/// OBCT container whose directory places every downloaded cell's block verbatim, and whose one
/// unpublished square is the `0` sentinel — read back through the §1.3 window a device forms, from
/// the checked-in bytes.
#[test]
fn the_terrain_region_places_every_published_cell_and_leaves_the_void_absent() {
    let out = assemble_fixture(&options(), &mut NoHooks);
    let map = taken(&out);
    let region = terrain_window(map).expect("this assembly has a raster");

    // OBCT §4.2's header, over the fixture's 2 × 2 rectangle at the catalog's lattice.
    assert_eq!(&region[..4], b"OBCT");
    assert_eq!(region[5], terrain_lattice().posting_log2);
    assert_eq!(region[6], terrain_lattice().cell_log2);
    assert_eq!(u16::from_le_bytes(region[16..18].try_into().unwrap()), 2, "rows");
    assert_eq!(u16::from_le_bytes(region[18..20].try_into().unwrap()), 2, "cols");
    let dir: Vec<u32> = region[32..48].as_chunks::<4>().0.iter().map(|c| u32::from_le_bytes(*c)).collect();
    assert_eq!(dir.iter().filter(|&&e| e == 0).count(), 1, "exactly one canonically void square (OBCC §13.6)");
    assert_eq!(dir[3], 0, "…and it is the rectangle's last slot");

    // Every present block is byte-for-byte the block of the published cell it came from — placement,
    // not grafting. The published objects' own blocks start after their 32-byte header and single
    // directory entry. The block offsets are **region**-relative, which is what makes the spliced
    // raster the same container the bakery publishes rather than one patched for its position.
    for cell in terrain_cells() {
        let (i, j) = {
            let mut parts = cell.id.split('/').skip(1);
            (parts.next().unwrap().parse::<u32>().unwrap(), parts.next().unwrap().parse::<u32>().unwrap())
        };
        let slot = (i - 602) as usize * 2 + (j - 526) as usize;
        let at = dir[slot] as usize;
        assert!(at != 0, "cell {} was published and must be present", cell.id);
        assert_eq!(&region[at..at + 2048], &cell.bytes[36..36 + 2048], "cell {}'s block moved", cell.id);
    }

    // 32-byte header + 4 × 4-byte directory + 3 × 2048-byte blocks, and the map is the flat map plus
    // exactly that — the splice adds the raster and nothing else.
    assert_eq!(region.len(), 32 + 16 + 3 * 2048);
    assert_eq!(map.len(), expected("flat.obcm").len() + region.len());
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    assert_eq!(s["terrain"]["bytes"], region.len());
    assert_eq!(s["terrain"]["cells"], 3);
    assert_eq!(s["terrain"]["slots"], 4);
    // There is no second digest: the raster is a run of bytes inside a file that has one identity.
    assert!(s["terrain"]["sha256"].is_null(), "the raster stopped having a digest of its own");
}

/// A selection with no raster assembles exactly as it did **before the raster was ever spliced in**:
/// `expected/flat.obcm` is the file this fixture produced when terrain was a separate `.OBD`, byte
/// for byte, and its §1.3 pair is `(0, 0)`. `OBCC_Spec.md` §13's degrade-to-flat rule, at the seam
/// where it is easiest to get wrong.
#[test]
fn a_selection_with_no_terrain_is_the_map_it_always_was() {
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    let map = taken(&out);
    assert_same_bytes(map, &expected("flat.obcm"), "flat.obcm");
    assert_eq!(&map[HEADER_TERRAIN_OFFSET_AT..HEADER_TERRAIN_OFFSET_AT + 8], &[0u8; 8], "§1.3's pair is (0, 0)");
    assert!(terrain_window(map).is_none());
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("summary");
    assert!(s["terrain"].is_null());
}

/// A digest the catalog does not confirm is refused, and the whole map is refused with it — the
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

/// Same inputs, same bytes — twice, in one process. The engine renumbers nav nodes and re-bins POIs
/// through hash maps; a run-order dependency in either would show up here as two different files
/// from one fixture.
#[test]
fn two_runs_produce_identical_bytes() {
    let a = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("run one");
    let b = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("run two");
    assert_same_bytes(taken(&a), taken(&b), "the map across two runs");
    assert_eq!(a.sha256, b.sha256);
}

// --- the §1.2 gaps ------------------------------------------------------------------------------
//
// Everything above compares the bridge's bytes against the CLI's, which is what a *drift* guard is
// for — and it is exactly why it cannot see a filler mistake. Both sides are the same engine, so a
// run that wrote its gaps as zeros, or left one out and slid every structure behind it down, would
// agree with itself and with a freshly regenerated fixture, and every offset in the file would still
// resolve. `OBCM_Spec.md` §1.2 says the gaps are part of the file and two bakes agree on them or
// they do not agree at all, so they get a pin of their own: an independent walk of the finished map
// that names each gap the layout implies and reads its bytes.

/// The unit every offset in a map this engine writes counts (§1.1), and the fill byte (§1.2).
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

/// Every gap `OBCM_Spec.md` §1.2 puts in one map, walked from the file's own directories.
///
/// Two kinds of mistake this catches that a byte-for-byte comparison against the CLI cannot: a gap
/// written with the wrong fill (the reader never looks at these bytes, so nothing else notices), and
/// a gap that is not there at all (every offset behind it simply moves, and the file stays
/// self-consistent). The counters at the end are what stop the walk being vacuous — a fixture whose
/// structures happened to land on unit boundaries would exercise nothing.
fn assert_section_gaps(map: &[u8]) -> (usize, usize) {
    let mut gaps = 0usize; // region and section boundaries actually filled
    let mut padded_chunks = 0usize; // §5.1 chunks whose content ended mid-unit
    let mut gap = |from: usize, to: usize, what: &str| {
        assert_filler(map, from, to, what);
        gaps += (to > from) as usize;
    };

    // §1: the 49-byte header, then the run to the style table's boundary.
    assert_eq!(map[4], 14, "the version byte this walk is written against");
    assert_eq!(map[40], 4, "`Offset Scale`, so U = 16");
    let style_at = offset(map, 21);
    assert_eq!(style_at, 64, "the style table starts at align_up(49)");
    gap(49, style_at, "header → style table");

    // §2 → §3: the style table's own tail, and the LOD table's.
    let lod_table_at = offset(map, 26);
    let style_end = style_at + 1 + map[style_at] as usize * 8;
    gap(style_end, lod_table_at, "style table → LOD table");
    let lod_count = map[25] as usize;
    let lod_table_end = lod_table_at + lod_count * 18;

    // §3/§5.1: per LOD, the rounding step between the offset table and `data_start`, and the run
    // behind every chunk's `0xFF` sentinel.
    let mut previous_end = lod_table_end;
    for i in 0..lod_count {
        let entry = lod_table_at + i * 18;
        let index_at = offset(map, entry + 4);
        assert_eq!(index_at % UNIT, 0, "LOD {i}: a scaled `Index Offset` cannot name a non-boundary");
        gap(previous_end, index_at, &format!("→ LOD {i}'s index"));
        let (node_count, chunk_count) = (le32(map, entry + 8), le32(map, entry + 14));
        let chunk_size = u16::from_le_bytes(map[entry + 12..entry + 14].try_into().unwrap()) as usize;
        let table_at = index_at + node_count * 4;
        let table_end = table_at + (chunk_count + 1) * 4;
        let data_start = align_up(table_end);
        gap(table_end, data_start, &format!("LOD {i}: offset table → data_start"));
        for k in 0..chunk_count {
            let (from, to) = (offset(map, table_at + k * 4), offset(map, table_at + (k + 1) * 4));
            assert!(to - from <= align_up(chunk_size), "LOD {i} chunk {k}: span past §5.1's bound");
            // The chunk's content ends at its one sentinel; from there to the unit boundary is
            // filler, so the run of `0xFF` at the end is `1 + (0..U-1)`. A writer that padded with
            // zeros leaves a run of exactly one, and the last byte of the span is not `0xFF`.
            let end = data_start + to;
            let run = map[data_start + from..end].iter().rev().take_while(|&&b| b == FILLER).count();
            assert!(run >= 1, "LOD {i} chunk {k}: no `0xFF` sentinel at the end of the span");
            assert!(run <= UNIT, "LOD {i} chunk {k}: {run} trailing 0xFF, more than a sentinel plus one unit");
            padded_chunks += (run > 1) as usize;
        }
        previous_end = data_start + offset(map, table_at + chunk_count * 4);
    }

    // §7.1: the directory's tail, each category's index → chunks step, and the section's own end.
    let poi_at = offset(map, 32);
    gap(previous_end, poi_at, "last LOD → POI section");
    let categories = map[poi_at] as usize;
    let poi_chunk_size = u16::from_le_bytes(map[poi_at + 1..poi_at + 3].try_into().unwrap()) as usize;
    let dir_end = poi_at + 1 + 2 + categories * 13 + 4 + 2;
    gap(dir_end, align_up(dir_end), "POI directory → first category");
    for c in 0..categories {
        let entry = poi_at + 3 + c * 13;
        let index_at = offset(map, entry + 1);
        let (node_count, chunk_count) = (le32(map, entry + 5), le32(map, entry + 9));
        let index_end = index_at + node_count * 4;
        gap(index_end, align_up(index_end), &format!("POI category {}: index → chunks", c + 1));
        // 512 is a multiple of `U` at every legal scale, so a category's chunk run carries none.
        assert_eq!(align_up(index_end) % UNIT, 0);
        assert_eq!(poi_chunk_size % UNIT, 0, "the fixed POI stride needs no filler inside the run");
        let _ = chunk_count;
    }
    let pool_at = offset(map, dir_end - 6);
    let pool_end = pool_at + 2 + u16::from_le_bytes(map[dir_end - 2..dir_end].try_into().unwrap()) as usize * 29;
    let nav_at = offset(map, 36);
    gap(pool_end, nav_at, "hours pool → nav section");

    // §8.1: the eight bytes behind the 40-byte directory, the alignment run that lands the node
    // chunks on a sector, and the rounding step between the index and those chunks. **All of it is
    // `0xFF` since v14** — v13 wrote zeros for the alignment runs, which is precisely the change no
    // offset in this file can see.
    gap(nav_at + 40, nav_at + 48, "nav directory → profile table");
    let profile_end = nav_at + 48 + map[nav_at + 26] as usize * 56;
    let index_at = offset(map, nav_at);
    let node_count = le32(map, nav_at + 4);
    gap(profile_end, index_at, "profile table → node index (§8.1's alignment run)");
    let index_end = index_at + node_count * 4;
    let chunks_at = align_up(index_end);
    gap(index_end, chunks_at, "node index → node chunks");
    if node_count > 0 {
        // §8.1's sector landing is a producer guarantee for a **populated** graph.
        assert_eq!(chunks_at % 512, 0, "…and the run put them on a sector, which is the point of it");
    } else {
        assert_eq!(index_at, align_up(profile_end), "an empty graph's regions are still nameable");
    }
    let pool_at = offset(map, nav_at + 12);
    assert_eq!(pool_at, chunks_at + le32(map, nav_at + 8) * 512);
    let snap_at = offset(map, nav_at + 28);
    let snap_nodes = le32(map, nav_at + 32);
    gap(pool_at + le32(map, nav_at + 16) * 512, snap_at, "edge pool → snap index");
    gap(snap_at + snap_nodes * 4, align_up(snap_at + snap_nodes * 4), "snap index → snap chunks");
    (gaps, padded_chunks)
}

/// The gap walk over the fixture's own map.
#[test]
fn every_section_boundary_of_the_assembled_map_is_filler() {
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    let (gaps, padded_chunks) = assert_section_gaps(taken(&out));
    // The fixture must actually *have* gaps, or the walk above asserts nothing. Both counters are
    // the two costs §1.2 quantifies separately: the per-region ones, and the per-chunk ones.
    assert!(gaps >= 8, "only {gaps} non-empty region gaps — this fixture no longer exercises §1.2");
    assert!(padded_chunks > 0, "no §5.1 chunk ended mid-unit, so the per-chunk filler is untested");
}

/// …and over the map the raster was spliced into, where the same walk must find the same gaps: the
/// §1.3 region rides in the tail, past everything §1.2 names, so splicing it must not move a byte of
/// the layout in front of it.
#[test]
fn splicing_the_raster_moves_none_of_the_gaps_in_front_of_it() {
    let out = assemble_fixture(&options(), &mut NoHooks);
    let map = taken(&out);
    let spliced = assert_section_gaps(map);
    let flat = assert_section_gaps(&expected("flat.obcm"));
    assert_eq!(spliced, flat, "the raster changed the §1.2 gap structure of the map in front of it");
    // …and it sits behind the last thing the walk reached, on a unit boundary as §1.1 requires of
    // anything a scaled offset names.
    let at = offset(map, HEADER_TERRAIN_OFFSET_AT);
    assert_eq!(at % UNIT, 0);
    assert_eq!(at + terrain_window(map).expect("a raster").len(), map.len(), "the raster is the tail");
}

/// **`Edge Id` is a `(chunk, ordinal)` pair, not a byte offset** (§8.4). The two agree on the first
/// record of the first chunk and nowhere else, so a fixture with several edges in one chunk is what
/// tells them apart: under v14 the ids of a nine-edge single-chunk pool are `0..=8`, where the v13
/// byte offsets ran to the hundreds.
#[test]
fn the_edge_ids_the_merge_mints_are_chunks_and_ordinals() {
    let out = assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut NoHooks).expect("the assembly runs");
    let map = taken(&out);
    let nav_at = offset(map, 36);
    assert_eq!(le32(map, nav_at + 16), 1, "the fixture's whole edge pool is one 512-byte chunk");

    // Every `Edge Id` in the §8.3 adjacency, read off the node chunks.
    let chunks_at = align_up(offset(map, nav_at) + le32(map, nav_at + 4) * 4);
    let mut ids: Vec<u32> = Vec::new();
    for k in 0..le32(map, nav_at + 8) {
        let chunk = &map[chunks_at + k * 512..][..512];
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
    assert_same_bytes(taken(&got), taken(&want), "the map with the cells reversed");
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
/// `driver`'s module header), which is a contract with `obcm_assemble::assemble_full`'s internals
/// that nothing else enforces. If the engine gains or loses a phase, this test fails — instead of a
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
        vec![Phase::Open, Phase::Poi, Phase::Nav, Phase::Plan, Phase::Write, Phase::Verify, Phase::Done],
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
/// One read per boundary, four for the write and verify passes (start/end each), one final total:
/// ten in all.
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
/// baden-württemberg's 26.2 s, #1116's phase-D harness), and the engine makes exactly *one* store
/// call for the whole pass — so a bar driven by store calls alone reaches its write-phase maximum
/// and then freezes for two fifths of the wait. `VerifySource::read_at` is what stops that, and this
/// is the test that says so: the pass reports many times, strictly forward, over a wide span of the
/// bar.
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
    // (0.203 + the write term) and `done` is the 1.0. This fixture's output is 0.61× its input
    // bytes — the projection both terms are measured against — so neither term reaches its full span
    // and the final report closes the gap. At the scale the 1.00 ratio was measured on (#1116's
    // harness regions), output ≈ input and they do.
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
    assert!(e.message.contains("partial file"), "{}", e.message);
    // Nothing past the write was reported: the abort is honoured at the next store call.
    assert!(!rec.seen.iter().any(|(p, _)| *p == Phase::Done));
}

/// **The verify-abort pin**, and the sharper half of the same defect: with §4.8 making one store
/// call and reporting nothing, an abort armed anywhere inside it was a **no-op** — the run went on
/// to produce the whole map, so a cancel button pressed during the longest phase of the run did
/// nothing at all.
///
/// Both moments are checked: the boundary callback (`n = 1`, the review's own probe) and one from
/// inside the read loop (`n = 4`), which only the `read_at` poll can honour. And in both, the
/// failure must read as `aborted` — `verify_map` turns any read refusal into `Error::Verify`, so
/// the naive mapping would tell the rider the assembler is broken because they pressed cancel.
#[test]
fn an_abort_armed_inside_the_verify_pass_stops_the_run() {
    for n in [1, 4] {
        let mut rec = Recorder { abort_at: Some((Phase::Verify, n)), ..Default::default() };
        let e = match assemble_cells(cells(), &sidecar(), &skin(), &options(), &mut rec) {
            // The pre-fix behaviour, exactly: cancel during §4.8 and the whole map is produced anyway.
            Ok(out) => panic!("cancelled at verify callback {n}, and it still produced {out:?}"),
            Err(e) => e,
        };
        assert_eq!(e.code, ErrorCode::Aborted, "cancelled at verify callback {n}: {}", e.message);
        assert!(e.message.contains("cancelled"), "{}", e.message);
        // The abort is honoured on the very next read, so the run never reaches `done` and nothing
        // downstream is told there is a map.
        assert!(!rec.seen.iter().any(|(p, _)| *p == Phase::Done));
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
    assert_eq!(s["bytes"].as_u64(), Some(taken(&out).len() as u64));
    // The §4.8 verify report is present and non-vacuous — the whole point of running verify in the
    // tab is that the caller can see it did. (The CLI prints it to a terminal; a browser has none,
    // so here it is a field.)
    assert!(s["verified"]["chunks"].as_u64().expect("a chunk count") > 0);
    assert!(s["verified"]["features"].as_u64().expect("a feature count") > 0);
    // The fixture's seam is real: nav nodes were unified across it and an islet was pruned.
    assert!(s["nav"]["unified"].as_u64().expect("a unified count") > 0, "the fixture's seam must unify junctions");
    assert!(s["nav"]["pruned_nodes"].as_u64().expect("a prune count") > 0, "the fixture's islet must be pruned");
    assert_eq!(s["poi"]["records"], 4);
    assert_eq!(out.warnings, Vec::<String>::new(), "this fixture is clean; a warning here is a real finding");
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
    assert_same_bytes(taken(&out), &expected("map.obcm"), "map.obcm from host reads");
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
        (out.bytes.expect("buffered"), reads)
    };
    let (uncached, engine_reads) = run(1);
    let (cached, host_reads) = run(64 * 1024);
    assert_same_bytes(&uncached, &cached, "the map with the read cache off");
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

// --- the map, written outside wasm memory (#1116 D1) --------------------------------------------

/// The host's own storage, as this crate sees it through [`MapWrites`]: one file, plus the three
/// things a real disk does that a `Vec<u8>` in the same process never would — refuse a write, refuse
/// a read, and hand back bytes that are not the ones it was given.
#[derive(Default)]
struct Disk {
    bytes: std::cell::RefCell<Vec<u8>>,
    sealed: std::cell::Cell<bool>,
    /// Refuse `write` once this many bytes have been accepted — a disk filling up mid-file.
    refuse_write_after: Option<usize>,
    /// Refuse every `read_at`: a handle closed under the §4.8 read-back's feet.
    refuse_reads: bool,
    /// Flip one byte at `seal`, **behind the driver's back**. Nothing in this process ever sees the
    /// change: this crate's digest was taken from the bytes on the way in, and the engine's from the
    /// same bytes by its own path. Only a §4.8 pass that genuinely re-reads the file can notice.
    corrupt: Option<usize>,
    /// How many times the host was asked for bytes — the read-back's own crossing count.
    reads: std::cell::Cell<usize>,
}

impl MapWrites for Disk {
    fn create(&self) -> Result<(), String> {
        self.bytes.borrow_mut().clear();
        self.sealed.set(false);
        Ok(())
    }

    fn write(&self, bytes: &[u8]) -> Result<(), String> {
        let mut file = self.bytes.borrow_mut();
        if let Some(after) = self.refuse_write_after {
            if file.len() + bytes.len() > after {
                return Err("the disk is full".into());
            }
        }
        assert!(!self.sealed.get(), "the map was written after it was sealed");
        file.extend_from_slice(bytes);
        Ok(())
    }

    fn read_at(&self, offset: u64, into: &mut [u8]) -> Result<(), String> {
        self.reads.set(self.reads.get() + 1);
        if self.refuse_reads {
            return Err("the storage handle is closed".into());
        }
        assert!(self.sealed.get(), "the map was read back before it was sealed");
        let file = self.bytes.borrow();
        let at = offset as usize;
        let want = file.get(at..at + into.len()).ok_or_else(|| format!("the map has no byte {at}"))?;
        into.copy_from_slice(want);
        Ok(())
    }

    fn seal(&self) -> Result<(), String> {
        self.sealed.set(true);
        if let Some(at) = self.corrupt {
            self.bytes.borrow_mut()[at] ^= 0xff;
        }
        Ok(())
    }
}

/// A caller whose map is written by the host, recording what it was told it now has.
#[derive(Default)]
struct Sinking {
    sealed: Vec<SealedMap>,
    /// Refuse the report, as a caller whose own bookkeeping failed.
    refuse: bool,
}

impl Hooks for Sinking {
    fn now_us(&mut self) -> u64 {
        0
    }
    fn progress(&mut self, _phase: Phase, _fraction: f64) -> bool {
        false
    }
    fn map_sealed(&mut self, map: SealedMap) -> Result<(), String> {
        if self.refuse {
            return Err("the finished map could not be recorded".into());
        }
        self.sealed.push(map);
        Ok(())
    }
}

/// The fixture assembled with its map written through `disk` instead of into this address space.
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

/// **The D1 pin**: a map that was never in this address space is the same file — the same bytes, the
/// same digest — as the one the native CLI wrote.
///
/// This is the claim the whole phase rests on, and one file made it sharper rather than softer: a
/// DACH map is a single ~9 GiB object, so it is not merely too big to hold, it is too big to
/// *address*. The seam that lets the engine write it must contribute no format knowledge, and this
/// is where that is checked.
#[test]
fn a_map_written_through_the_sink_is_the_native_clis_bytes() {
    let disk = Disk::default();
    let mut hooks = Sinking::default();
    let out = assemble_to_disk(&disk, &options(), &mut hooks).expect("the assembly runs");
    let want = expected("map.obcm");

    // Nothing came back: the whole file, raster included, went to the host.
    assert!(out.bytes.is_none(), "a sunk map must not be resident too");
    assert_same_bytes(&disk.bytes.borrow(), &want, "map.obcm through the sink");

    // …and what the caller was *told* it has matches what the engine says it wrote. The host saved
    // these bytes without ever seeing them, so this equality is the only thing between a
    // mislabelled file and a card.
    assert_eq!(hooks.sealed.len(), 1);
    assert_eq!(hooks.sealed[0].sha256, out.sha256);
    assert_eq!(hooks.sealed[0].byte_length, want.len() as u64);
    let s: serde_json::Value = serde_json::from_str(&out.summary_json).expect("the summary is JSON");
    assert_eq!(s["sha256"], out.sha256.as_str());
    assert_eq!(s["bytes"].as_u64(), Some(want.len() as u64));
}

/// **The proof that §4.8 reads the file.** Flip one byte of the sealed map behind the driver's
/// back — the sink's own storage changed, nothing in this process did — and the verify pass must
/// reject it.
///
/// It is the test the buffered store could never pass. With the bytes in a `Vec`, "read the map
/// back" and "look at the map" are the same operation, so §4.8 proves the *encoder* agrees with the
/// *decoder* and nothing about the medium. With a sink the medium is the thing that can lie, and a
/// read-back that quietly answered out of an in-memory copy would ship a corrupt map with a clean
/// verdict. Byte 0 is the OBCM magic, so what fails is unmistakably the reader.
#[test]
fn a_map_the_sink_corrupts_on_disk_fails_verify() {
    let disk = Disk { corrupt: Some(0), ..Disk::default() };
    let mut hooks = Sinking::default();
    let e = assemble_to_disk(&disk, &options(), &mut hooks).expect_err("the file on disk is not the one written");
    assert_eq!(e.code, ErrorCode::Verify, "{}", e.message);
    // Nothing was reported as sealed: §4.8 is a precondition of telling the caller it has a map.
    assert!(hooks.sealed.is_empty(), "a map that failed its read-back was reported as finished");
    assert!(disk.reads.get() > 0, "the read-back never asked the host for a byte");
}

/// A sink that cannot take the map's bytes fails the **run**, as `io` and in the host's own words.
/// Not `verify`: a full disk is not a defect in the assembler.
#[test]
fn a_sink_that_refuses_a_write_fails_the_run_as_io() {
    let disk = Disk { refuse_write_after: Some(4096), ..Disk::default() };
    let mut hooks = Sinking::default();
    let e = assemble_to_disk(&disk, &options(), &mut hooks).expect_err("the disk filled up");
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
    assert!(e.message.contains("the disk is full"), "the host's own words: {}", e.message);
    assert!(e.message.contains("the map could not be written"), "{}", e.message);
    assert!(hooks.sealed.is_empty());
}

/// …and a sink that cannot give them **back** is `io` too, although §4.8 is where it surfaces.
///
/// This is the same rule as `map_error`'s abort-first one, one seam over: `verify_map` reports any
/// read failure as a §4.8 defect, so without the host's own message a closed handle would tell a
/// rider that the assembler wrote a map the reader cannot read — the one verdict the docs say never
/// to retry past.
#[test]
fn a_map_the_sink_cannot_read_back_is_io_not_a_verify_defect() {
    let disk = Disk { refuse_reads: true, ..Disk::default() };
    let e = assemble_to_disk(&disk, &options(), &mut NoHooks).expect_err("the read-back cannot read");
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
    assert!(e.message.contains("the storage handle is closed"), "the host's own words: {}", e.message);
    assert!(e.message.contains("the map"), "the message must say what could not be read: {}", e.message);
}

/// A caller that cannot record the finished map stops the run as `io`. The file exists and its
/// digest is the only thing that says which bytes are in it, so reporting success would hand on a
/// map nobody wrote down.
#[test]
fn a_sealed_report_the_caller_refuses_fails_the_run_as_io() {
    let disk = Disk::default();
    let mut hooks = Sinking { refuse: true, ..Default::default() };
    let e = assemble_to_disk(&disk, &options(), &mut hooks).expect_err("the caller refused the report");
    assert_eq!(e.code, ErrorCode::Io, "{}", e.message);
    assert!(e.message.contains("could not be recorded"), "{}", e.message);
    // The bytes are on the host's storage all the same — that file is the caller's to clean up, and
    // is why the refusal is reported rather than swallowed.
    assert_eq!(disk.bytes.borrow().len(), expected("map.obcm").len());
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
        // Whatever reached the host is a partial file, and the run never reported it finished.
        assert!(!rec.seen.iter().any(|(p, _)| *p == Phase::Done));
    }
}

/// The §4.8 read-back goes through the same block cache the input reads do, and for the same reason:
/// the pass walks the sealed map a record at a time, and one host call per read is one file read and
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
        let written = disk.bytes.borrow().clone();
        (written, disk.reads.get())
    };
    let (uncached, engine_reads) = run(1);
    let (cached, host_reads) = run(64 * 1024);
    assert_same_bytes(&uncached, &cached, "the map with the read-back cache off");
    eprintln!("read-back: {engine_reads} host reads with the cache off, {host_reads} at 64 KiB blocks");
    assert!(host_reads * 10 < engine_reads, "the block cache saved only {engine_reads} → {host_reads} host reads");
}
