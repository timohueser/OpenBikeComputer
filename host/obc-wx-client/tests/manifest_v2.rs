//! Manifest v2 (WXR4 #1243), against the **shared** fixture both clients read.
//!
//! `specs/vectors/wx-manifest-v2.json` is the first manifest ever cross-pinned between the Rust
//! and Swift clients: until now only the `.obcg`/`.obcw` byte vectors were shared, and Swift
//! synthesised its own manifests with `ManifestBuilder`, so the two parsers could drift on the one
//! document every rider reads first. The Swift twin of this file goes in
//! `companion-ios/Packages/OBCKit/Tests/OBCWeatherTests/ManifestV2Tests.swift` (WXR5 #1244) and
//! must assert the **same shard key set for the same bbox** as
//! `a_bbox_becomes_a_shard_key_set_by_arithmetic` below. That equivalence is what replaces
//! `ProductSelectionTests`.

use std::path::PathBuf;

use obc_wx_client::manifest_v2::{self, Bbox, ManifestError, ShardId, ShardState};

fn shared_fixture() -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/vectors/wx-manifest-v2.json");
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn parsed() -> manifest_v2::Manifest {
    manifest_v2::parse(&shared_fixture()).expect("the shared fixture parses")
}

/// A corridor bbox around a point, in microdegrees — the shape `select::Corridor` produces.
fn around(lat_udeg: i64, lon_udeg: i64, span_udeg: i64) -> Bbox {
    Bbox {
        south_udeg: lat_udeg - span_udeg,
        west_udeg: lon_udeg - span_udeg,
        north_udeg: lat_udeg + span_udeg,
        east_udeg: lon_udeg + span_udeg,
    }
}

#[test]
fn the_shared_fixture_parses_and_states_the_whole_grid() {
    let manifest = parsed();
    assert_eq!(manifest.generation, "20260810T1430Z");
    assert_eq!(manifest.previous_generations, vec!["20260810T1415Z", "20260810T1400Z"]);
    assert_eq!(manifest.skipped_frames, 0);
    assert_eq!(manifest.frames.len(), 9);

    // Nothing here is a client constant: the client reads the grid it must address.
    let grid = &manifest.grid;
    assert_eq!((grid.width, grid.height), (36_000, 18_000));
    assert_eq!((grid.shard_cols, grid.shard_rows, grid.shard_count()), (6, 4, 24));
    assert_eq!((grid.shard_width, grid.shard_height), (6_144, 4_608));
    assert_eq!((grid.tile_edge, grid.entries_per_page, grid.cell_size_m), (256, 128, 1_113));
    assert_eq!(grid.covered_rows, 12..17_987, "the polar band is stated once, not inferred");
    assert_eq!(manifest.cadence.frame_step_min, 15);
    assert_eq!(manifest.cadence.frames, 9);
    assert_eq!(manifest.cadence.max_source_skew_s, 1_800);
    // Every source that may have painted a cell; there is no per-cell provenance to narrow it to.
    assert_eq!(
        manifest.attribution.iter().map(|entry| entry.source_id.as_str()).collect::<Vec<_>>(),
        vec!["dwd-rv", "us", "icon-eu", "gfs"]
    );
}

/// **The test that replaces product selection.** A bbox in, a shard key set out, by division.
/// The Swift twin must produce this exact list of keys from this exact bbox.
#[test]
fn a_bbox_becomes_a_shard_key_set_by_arithmetic() {
    let manifest = parsed();
    let grid = &manifest.grid;

    // Freiburg, ~11 km of corridor: one shard, wholly interior.
    let freiburg = around(48_000_000, 7_850_000, 100_000);
    assert_eq!(grid.shards_for(&freiburg), vec![ShardId { col: 3, row: 2 }]);
    assert_eq!(
        grid.shard_key(0, ShardId { col: 3, row: 2 }),
        "wx/v2/20260810T1430Z/f0/s3-2.obcg",
        "the key is composed from key_prefix + generation + offset + (col,row), never read"
    );
    assert_eq!(grid.shard_key(120, ShardId { col: 3, row: 2 }), "wx/v2/20260810T1430Z/f120/s3-2.obcg");

    // A corridor straddling the col 0/1 shard seam at -118.56 deg: two shards, and the client
    // needs no containment test to discover it.
    let seam = around(34_000_000, -118_560_000, 200_000);
    assert_eq!(grid.shards_for(&seam), vec![ShardId { col: 0, row: 2 }, ShardId { col: 1, row: 2 }]);

    // A bbox landing exactly on the seam takes the shard it opens, not the one it closes: the
    // lattice window is half-open, so the east edge at the boundary is not in the eastern shard.
    let boundary =
        Bbox { south_udeg: 34_000_000, west_udeg: -118_600_000, north_udeg: 34_010_000, east_udeg: -118_560_000 };
    assert_eq!(grid.shards_for(&boundary), vec![ShardId { col: 0, row: 2 }]);

    // Four shards, the worst case the shard size was chosen against: a corridor sitting on the
    // crossing of the col 3/4 seam (lon 65.76 deg) and the row 2/3 seam (lat 48.24 deg).
    let corner = around(48_240_000, 65_760_000, 500_000);
    assert_eq!(
        grid.shards_for(&corner),
        vec![
            ShardId { col: 3, row: 2 },
            ShardId { col: 4, row: 2 },
            ShardId { col: 3, row: 3 },
            ShardId { col: 4, row: 3 }
        ]
    );
}

/// The three-valued answer, which is the whole reason the bitmap exists: **a 404 must never mean
/// dry**, and a dry shard must never look like a failure.
#[test]
fn missing_is_not_dry_and_dry_is_not_missing() {
    let manifest = parsed();
    let grid = &manifest.grid;
    let f0 = manifest.frame(0).expect("f0");

    // Present: an object to fetch, with the integrity data to check it against.
    match f0.state_of(grid, ShardId { col: 3, row: 2 }) {
        ShardState::Present { key, bytes, object_crc32, observed } => {
            assert_eq!(key, "wx/v2/20260810T1430Z/f0/s3-2.obcg");
            assert_eq!(bytes, 120_000 + 3 * 3_000 + 2 * 700);
            assert_eq!(object_crc32, 0x51A0_0000 + 47 * 0x0001_0101);
            assert!(observed, "a shard painted end to end by radar says so, per shard, not per frame");
        }
        other => panic!("expected an object, got {other:?}"),
    }

    // Dry: the baker measured every cell dry and published nothing. No request, no error.
    assert_eq!(f0.state_of(grid, ShardId { col: 2, row: 0 }), ShardState::Dry);
    assert_eq!(f0.state_of(grid, ShardId { col: 3, row: 0 }), ShardState::Dry);
    // ...and dryness is per frame: the same shard has an object at f15.
    assert!(matches!(
        manifest.frame(15).expect("f15").state_of(grid, ShardId { col: 2, row: 0 }),
        ShardState::Present { .. }
    ));
    // ...and the last frame has its own hole.
    assert_eq!(manifest.frame(120).expect("f120").state_of(grid, ShardId { col: 5, row: 3 }), ShardState::Dry);
    assert!(matches!(f0.state_of(grid, ShardId { col: 5, row: 3 }), ShardState::Present { .. }));

    // Off the grid: geometry, not weather and not an error.
    assert_eq!(f0.state_of(grid, ShardId { col: 6, row: 0 }), ShardState::OutOfDomain);
    assert_eq!(f0.state_of(grid, ShardId { col: 0, row: 4 }), ShardState::OutOfDomain);

    // A whole-timeline plan over a dry corner asks for 8 objects, not 9, and reports no failure
    // for the ninth.
    let over_the_hole = around(85_000_000, 175_000_000, 100_000);
    assert_eq!(grid.shards_for(&over_the_hole), vec![ShardId { col: 5, row: 3 }]);
    assert_eq!(manifest.plan(&over_the_hole).len(), 8, "nine frames, and f120's s5-3 is dry");
}

/// The bitmap and `shards[]` are one statement. A document where they disagree is not reconciled —
/// either direction of reconciliation invents a fact about whether an object exists.
#[test]
fn a_frame_whose_bitmap_and_list_disagree_is_refused_rather_than_reconciled() {
    let mut document: serde_json::Value = serde_json::from_slice(&shared_fixture()).expect("json");

    // A shard listed but not in the bitmap.
    let mut listed_not_flagged = document.clone();
    listed_not_flagged["frames"][0]["shards"].as_array_mut().unwrap().push(serde_json::json!({
        "col": 2, "row": 0, "bytes": 1234, "object_crc32": "0x00000001", "observed": false
    }));
    let parsed = manifest_v2::parse(&serde_json::to_vec(&listed_not_flagged).unwrap()).expect("document survives");
    assert_eq!(parsed.skipped_frames, 1, "the frame is skipped and counted, never fatal");
    assert_eq!(parsed.frames.len(), 8);

    // A shard in the bitmap with no entry.
    document["frames"][1]["shards"].as_array_mut().unwrap().remove(0);
    let parsed = manifest_v2::parse(&serde_json::to_vec(&document).unwrap()).expect("document survives");
    assert_eq!(parsed.skipped_frames, 1);
    assert!(parsed.frame(15).is_none());
}

/// The document is strict where being lenient would let a manifest steer the client, and where a
/// grid it cannot address leaves nothing to degrade to.
#[test]
fn the_document_is_strict_about_version_addressing_and_the_grid() {
    let base: serde_json::Value = serde_json::from_slice(&shared_fixture()).expect("json");
    let reparse = |document: &serde_json::Value| manifest_v2::parse(&serde_json::to_vec(document).unwrap());

    let mut wrong_version = base.clone();
    wrong_version["version"] = serde_json::json!(1);
    assert_eq!(reparse(&wrong_version), Err(ManifestError::UnsupportedVersion(1)));

    for (pointer, value) in [
        ("/key_prefix", serde_json::json!("../../etc")),
        ("/key_prefix", serde_json::json!("/wx/v2")),
        ("/generation", serde_json::json!("20260810T1430Z/../..")),
        // The shard grid must be the one that tiles the lattice, or client and baker disagree
        // about which object holds a cell.
        ("/lattice/shard_cols", serde_json::json!(7)),
        // A shard no OBCG header could express is not worth a Range read.
        ("/lattice/shard_width", serde_json::json!(36_000)),
        ("/lattice/covered_rows/end", serde_json::json!(18_001)),
    ] {
        let mut broken = base.clone();
        *broken.pointer_mut(pointer).expect(pointer) = value;
        assert!(matches!(reparse(&broken), Err(ManifestError::Malformed(_))), "{pointer} must be refused");
    }
}

/// Every deadline is read, not held. The client compares timestamps against the document, so the
/// service can change the cadence without a client release — and expiry is "no weather", which is
/// not "no rain".
#[test]
fn the_deadlines_come_from_the_document_not_from_a_client_constant() {
    let manifest = parsed();
    let stale_after = manifest_v2::parse_rfc3339("2026-08-10T16:30:00Z").expect("ts");
    assert_eq!(manifest.freshness.stale_after, stale_after);
    assert_eq!(
        manifest.freshness.next_generation_expected_at,
        manifest_v2::parse_rfc3339("2026-08-10T14:45:00Z").expect("ts")
    );
    assert!(manifest.freshness.is_usable(stale_after), "inclusive to the last second");
    assert!(!manifest.freshness.is_usable(stale_after + 1));
    assert_eq!(manifest.freshness.manifest_max_age_s, 60);
    let fetched_at = manifest.generated_at;
    assert!(!manifest.freshness.manifest_is_stale(fetched_at, fetched_at + 60));
    assert!(manifest.freshness.manifest_is_stale(fetched_at, fetched_at + 61));
}
