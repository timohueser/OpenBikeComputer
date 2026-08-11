//! Manifest v2 (WXR4 #1243), against the **shared** fixture both clients read.
//!
//! `specs/vectors/wx-manifest-v2.json` is the first manifest ever cross-pinned between the Rust and
//! Swift clients: until now only the `.obcg`/`.obcw` byte vectors were shared, and Swift synthesised
//! its own manifests with `ManifestBuilder`, so the two parsers could drift on the one document
//! every rider reads first. The Swift twin of this file goes in
//! `companion-ios/Packages/OBCKit/Tests/OBCWeatherTests/ManifestV2Tests.swift` (WXR5 #1244).
//!
//! The bbox cases are **not written here**. They live in `specs/vectors/manifest.json`'s
//! `wx_manifest_v2.bbox_equivalence` and this suite is a driver over them, so a case added for one
//! language is automatically a case the other must answer identically. That table is the whole
//! cross-client contract, and it is deliberately built out of the geometry a second implementer can
//! get wrong while passing everything else: an exact shard boundary, a southern-hemisphere corridor,
//! an antimeridian wrap, the polar band, and three bboxes that must be refused rather than clamped.

use std::path::PathBuf;

use obc_wx_client::manifest_v2::{self, Bbox, BboxError, ManifestError, PlanOutcome, ShardId, ShardState};

fn repo_file(relative: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(relative);
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn shared_fixture() -> Vec<u8> {
    repo_file("specs/vectors/wx-manifest-v2.json")
}

fn parsed() -> manifest_v2::Manifest {
    manifest_v2::parse(&shared_fixture()).expect("the shared fixture parses")
}

/// Inside the fixture's generation, so nothing is expired unless a test says so.
fn during() -> i64 {
    manifest_v2::parse_rfc3339("2026-08-10T14:40:00Z").expect("ts")
}

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

/// **The test that replaces product selection**, driven from the cross-language table.
///
/// Every case pins three things a Swift twin could otherwise get wrong independently: the shard set,
/// the composed keys, and the plan's *outcome* — because "no objects" is three different answers and
/// only one of them is about rain.
#[test]
fn every_pinned_bbox_case_agrees_with_the_shared_fixture() {
    let manifest = parsed();
    let vectors: serde_json::Value = serde_json::from_slice(&repo_file("specs/vectors/manifest.json")).expect("json");
    let cases = vectors["wx_manifest_v2"]["bbox_equivalence"].as_array().expect("bbox_equivalence");
    assert!(cases.len() >= 10, "the table is the contract; do not shrink it");

    for case in cases {
        let name = case["name"].as_str().expect("name");
        let bbox = Bbox {
            south_udeg: case["bbox_udeg"]["south"].as_i64().expect("south"),
            west_udeg: case["bbox_udeg"]["west"].as_i64().expect("west"),
            north_udeg: case["bbox_udeg"]["north"].as_i64().expect("north"),
            east_udeg: case["bbox_udeg"]["east"].as_i64().expect("east"),
        };
        let expected_shards: Vec<ShardId> = case["shards"]
            .as_array()
            .expect("shards")
            .iter()
            .map(|shard| ShardId {
                col: shard["col"].as_u64().expect("col") as u32,
                row: shard["row"].as_u64().expect("row") as u32,
            })
            .collect();

        match case.get("error").and_then(serde_json::Value::as_str) {
            Some("out_of_range") => {
                assert_eq!(manifest.grid.shards_for(&bbox), Err(BboxError::OutOfRange), "{name}");
            }
            Some(other) => panic!("{name}: unknown pinned error {other}"),
            None => {
                assert_eq!(manifest.grid.shards_for(&bbox).expect(name), expected_shards, "{name}: shard set");
                let keys: Vec<String> =
                    expected_shards.iter().map(|shard| manifest.grid.shard_key(0, *shard)).collect();
                let pinned: Vec<&str> =
                    case["f0_keys"].as_array().expect("f0_keys").iter().map(|k| k.as_str().unwrap()).collect();
                assert_eq!(keys, pinned, "{name}: composed keys");
            }
        }

        let plan = manifest.plan(&bbox, during());
        let expected = match case["outcome"].as_str().expect("outcome") {
            "covered" => PlanOutcome::Covered,
            "uncovered" => PlanOutcome::Uncovered,
            "out_of_domain" => PlanOutcome::OutOfDomain,
            other => panic!("{name}: unknown pinned outcome {other}"),
        };
        assert_eq!(plan.outcome, expected, "{name}: outcome");
        if expected != PlanOutcome::Covered {
            assert!(plan.fetch.is_empty() && plan.dry.is_empty(), "{name}: only Covered carries vectors");
        }
    }
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

    // A whole-timeline plan over the f120 hole fetches eight objects and reports the ninth as dry —
    // the two are carried in different vectors, so neither can be rendered as the other.
    let over_the_hole = around(85_000_000, 175_000_000, 100_000);
    let plan = manifest.plan(&over_the_hole, during());
    assert_eq!(plan.outcome, PlanOutcome::Covered);
    assert_eq!(plan.fetch.len(), 8);
    assert_eq!(plan.dry, vec![(120, ShardId { col: 5, row: 3 })]);
    assert!(plan.fetch.iter().all(|read| read.key.ends_with("s5-3.obcg")));
}

/// The bitmap and the shard list are two spellings of one fact, and the module holds them equal by
/// making both private and looking the shard up in the list. Pinned across the whole fixture so the
/// two can never answer differently for any shard of any frame.
#[test]
fn the_bitmap_and_the_lookup_are_the_same_answer() {
    let manifest = parsed();
    let grid = &manifest.grid;
    let mut present = 0usize;
    for frame in &manifest.frames {
        for row in 0..grid.shard_rows {
            for col in 0..grid.shard_cols {
                let shard = ShardId { col, row };
                let by_bitmap = frame.is_present(grid, shard);
                let by_lookup = matches!(frame.state_of(grid, shard), ShardState::Present { .. });
                assert_eq!(by_bitmap, by_lookup, "f{} s{col}-{row}", frame.offset_min);
                present += usize::from(by_bitmap);
            }
        }
        assert!(frame.shards().windows(2).all(|pair| pair[0].id < pair[1].id), "ascending by (row, col)");
    }
    assert_eq!(present, 9 * 24 - 3, "the fixture's three deliberate holes");
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

/// The document is strict where being lenient would let a manifest steer the client, where a grid it
/// cannot address leaves nothing to degrade to, and where the client and the service's sweep would
/// otherwise disagree about what exists.
#[test]
fn the_document_is_strict_about_version_addressing_the_grid_and_retention() {
    let base: serde_json::Value = serde_json::from_slice(&shared_fixture()).expect("json");
    let reparse = |document: &serde_json::Value| manifest_v2::parse(&serde_json::to_vec(document).unwrap());

    let mut wrong_version = base.clone();
    wrong_version["version"] = serde_json::json!(1);
    assert_eq!(reparse(&wrong_version), Err(ManifestError::UnsupportedVersion(1)));

    for (pointer, value) in [
        ("/key_prefix", serde_json::json!("../../etc")),
        ("/key_prefix", serde_json::json!("/wx/v2")),
        ("/generation", serde_json::json!("20260810T1430Z/../..")),
        // Three generations is the client and the sweep disagreeing about what exists.
        ("/previous_generations", serde_json::json!(["20260810T1415Z", "20260810T1400Z", "20260810T1345Z"])),
        // The shard grid must be the one that tiles the lattice.
        ("/lattice/shard_cols", serde_json::json!(7)),
        // A shard no OBCG header could express is not worth a Range read.
        ("/lattice/shard_width", serde_json::json!(36_000)),
        ("/lattice/covered_rows/end", serde_json::json!(18_001)),
        // A cadence that disagrees with its own frame list is a mis-derived cycle.
        ("/cadence/frames", serde_json::json!(8)),
        // A generation that expires before its replacement is due.
        ("/freshness/stale_after", serde_json::json!("2026-08-10T14:40:00Z")),
    ] {
        let mut broken = base.clone();
        *broken.pointer_mut(pointer).expect(pointer) = value;
        assert!(matches!(reparse(&broken), Err(ManifestError::Malformed(_))), "{pointer} must be refused");
    }

    // Two frames naming the same object at two validities.
    let mut duplicate = base.clone();
    let mut clone = duplicate["frames"][1].clone();
    clone["offset_min"] = serde_json::json!(0);
    duplicate["frames"].as_array_mut().unwrap().insert(1, clone);
    duplicate["cadence"]["frames"] = serde_json::json!(10);
    assert!(matches!(reparse(&duplicate), Err(ManifestError::Malformed(_))), "duplicate offset_min");
}

/// Every deadline is read, not held. The client compares timestamps against the document, so the
/// service can change the cadence without a client release.
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

/// **Expiry is no weather, and no weather is not a dry map.** The check lives inside `plan` rather
/// than in a caller's discipline, because "did anyone remember to call `is_usable` first" is exactly
/// the contract that holds until the one call site that forgets — and the thing it would render is
/// the forbidden one.
#[test]
fn an_expired_generation_is_no_weather_not_a_dry_map() {
    let manifest = parsed();
    let freiburg = around(48_000_000, 7_850_000, 100_000);
    let after = manifest.freshness.stale_after + 1;

    let live = manifest.plan(&freiburg, during());
    assert_eq!(live.outcome, PlanOutcome::Covered);
    assert_eq!(live.fetch.len(), 9);

    let expired = manifest.plan(&freiburg, after);
    assert_eq!(expired.outcome, PlanOutcome::Expired);
    assert!(expired.fetch.is_empty() && expired.dry.is_empty());
    // The frames are still there and still true; what expired is the right to answer with them.
    assert!(matches!(
        manifest.frame(0).expect("f0").state_of(&manifest.grid, ShardId { col: 3, row: 2 }),
        ShardState::Present { .. }
    ));
}

/// **R2-1: the unclamped intersection, pinned where it is observable.**
///
/// On the global lattice this fix hides: every in-range coordinate is on the lattice, so
/// `Bbox::validate` catches the off-map cases before the arithmetic ever runs, and clamp-first and
/// intersect-first agree on everything left. A **regional** grid is the only geometry where they
/// disagree — and the manifest states the grid, so a regional one is a baker deploy away rather
/// than hypothetical. This constructs one and pins the difference directly, because round 1's fix
/// was otherwise revertible with the suite staying green.
///
/// The grid: 10 x 6 degrees over central Europe at the canonical pitch, cut into 2 x 2 shards.
#[test]
fn an_off_lattice_bbox_vanishes_rather_than_collapsing_onto_the_edge() {
    let grid = manifest_v2::Grid {
        south_lat_udeg: 47_000_000,
        west_lon_udeg: 5_000_000,
        cell_udeg: 10_000,
        width: 1_000,
        height: 600,
        shard_width: 500,
        shard_height: 300,
        shard_cols: 2,
        shard_rows: 2,
        tile_edge: 256,
        entries_per_page: 128,
        cell_size_m: 1_113,
        covered_rows: 0..600,
        key_prefix: "wx/v2".into(),
        generation: "20260810T1430Z".into(),
    };

    // Inside: the control, so a grid that answered nothing to everything cannot pass this test.
    let inside = Bbox { south_udeg: 48_000_000, west_udeg: 7_750_000, north_udeg: 48_100_000, east_udeg: 7_950_000 };
    assert_eq!(grid.shards_for(&inside).expect("well formed"), vec![ShardId { col: 0, row: 0 }]);

    // Every direction of "wholly outside, but a perfectly legal coordinate pair". Clamp-first
    // returns the nearest edge shard for each of these — the rider is handed a neighbouring
    // region's weather instead of being told they are off the map. Intersect-first returns nothing.
    let outside = [
        ("west of it", Bbox { south_udeg: 48_000_000, west_udeg: 0, north_udeg: 48_100_000, east_udeg: 1_000_000 }),
        (
            "east of it",
            Bbox { south_udeg: 48_000_000, west_udeg: 20_000_000, north_udeg: 48_100_000, east_udeg: 21_000_000 },
        ),
        (
            "south of it",
            Bbox { south_udeg: 40_000_000, west_udeg: 7_750_000, north_udeg: 41_000_000, east_udeg: 7_950_000 },
        ),
        (
            "north of it",
            Bbox { south_udeg: 60_000_000, west_udeg: 7_750_000, north_udeg: 61_000_000, east_udeg: 7_950_000 },
        ),
        (
            "diagonally past the corner",
            Bbox { south_udeg: 60_000_000, west_udeg: 20_000_000, north_udeg: 61_000_000, east_udeg: 21_000_000 },
        ),
    ];
    for (where_, bbox) in outside {
        assert_eq!(
            grid.shards_for(&bbox).expect("a legal coordinate pair, just not on this lattice"),
            Vec::<ShardId>::new(),
            "{where_}: an off-lattice bbox must vanish, not collapse onto the edge shard"
        );
    }

    // Exactly abutting the west edge from outside is still outside: the window is half-open, so an
    // east edge on the lattice origin closes the cell before it, which is not a cell of this grid.
    let abutting = Bbox { south_udeg: 48_000_000, west_udeg: 4_000_000, north_udeg: 48_100_000, east_udeg: 5_000_000 };
    assert_eq!(grid.shards_for(&abutting).expect("well formed"), Vec::<ShardId>::new());
    // One microdegree further east and it touches the first cell.
    let touching = Bbox { south_udeg: 48_000_000, west_udeg: 4_000_000, north_udeg: 48_100_000, east_udeg: 5_000_001 };
    assert_eq!(grid.shards_for(&touching).expect("well formed"), vec![ShardId { col: 0, row: 0 }]);
}

/// **R2-2: shard order is `(row, col)`, pinned against literals.**
///
/// `ShardId`'s `Ord` is written out rather than derived, because the derive would order by
/// `(col, row)` — field order — while the document orders by `(row, col)` everywhere: `shards[]`,
/// the result of `shards_for`, and the presence bit index `row * shard_cols + col`. That mismatch
/// is not cosmetic: `state_of` binary-searches the shard list, so a derived `Ord` makes it answer
/// **`Dry` for shards that exist**, which is the one answer this whole issue forbids.
///
/// Sorting a shuffled set and comparing against a hand-written expected order is the pin. A revert
/// to `#[derive(Ord)]` fails here on the first pair that differs.
#[test]
fn shard_ids_sort_by_row_then_col_not_col_then_row() {
    let mut shards = vec![
        ShardId { col: 5, row: 0 },
        ShardId { col: 0, row: 1 },
        ShardId { col: 1, row: 0 },
        ShardId { col: 0, row: 2 },
        ShardId { col: 2, row: 1 },
        ShardId { col: 0, row: 0 },
        ShardId { col: 5, row: 3 },
        ShardId { col: 1, row: 1 },
    ];
    shards.sort();
    assert_eq!(
        shards,
        vec![
            ShardId { col: 0, row: 0 },
            ShardId { col: 1, row: 0 },
            ShardId { col: 5, row: 0 },
            ShardId { col: 0, row: 1 },
            ShardId { col: 1, row: 1 },
            ShardId { col: 2, row: 1 },
            ShardId { col: 0, row: 2 },
            ShardId { col: 5, row: 3 },
        ],
        "rows first, then columns within a row — a derived (col, row) Ord fails here"
    );

    // The pair the derive gets backwards, stated on its own so the failure names the rule.
    assert!(
        ShardId { col: 5, row: 0 } < ShardId { col: 0, row: 1 },
        "the last shard of row 0 precedes the first shard of row 1"
    );
    assert!(ShardId { col: 0, row: 1 } > ShardId { col: 1, row: 0 });

    // And the order the shared fixture is written in agrees, end to end.
    let manifest = parsed();
    let mut walked: Vec<ShardId> = Vec::new();
    for row in 0..manifest.grid.shard_rows {
        for col in 0..manifest.grid.shard_cols {
            walked.push(ShardId { col, row });
        }
    }
    let mut sorted = walked.clone();
    sorted.sort();
    assert_eq!(walked, sorted, "the bit index walk row-major and the sort order are the same order");
}

// ── the shared rejection corpus ────────────────────────────────────────────────────────────

/// Apply one `rejection_equivalence` patch op to a parsed document. RFC 6901 pointers, two ops.
///
/// Deliberately tiny and deliberately duplicated in Swift rather than pulled from a JSON-Patch
/// crate: the whole value of the corpus is that two independent implementations answer the same
/// way, and a shared library on one side against a hand-rolled walker on the other is exactly the
/// asymmetry that makes a "cross-language" fixture test one language.
fn apply_patch(document: &mut serde_json::Value, op: &serde_json::Value) {
    let path: Vec<&str> = op["path"].as_str().expect("path").trim_start_matches('/').split('/').collect();
    let (last, parents) = path.split_last().expect("a non-empty pointer");
    let mut cursor = document;
    for step in parents {
        cursor = match cursor {
            serde_json::Value::Array(items) => &mut items[step.parse::<usize>().expect("index")],
            other => &mut other[*step],
        };
    }
    match (op["op"].as_str().expect("op"), cursor) {
        ("set", serde_json::Value::Object(map)) => {
            map.insert((*last).to_string(), op["value"].clone());
        }
        ("remove", serde_json::Value::Object(map)) => {
            map.remove(*last);
        }
        ("remove", serde_json::Value::Array(items)) => {
            items.remove(last.parse::<usize>().expect("index"));
        }
        (op, _) => panic!("unsupported patch op {op}"),
    }
}

/// **The other half of the cross-client contract.** `bbox_equivalence` pins what the two clients
/// compute; this pins what they *refuse* — and the review that asked for it found five documents
/// the two answered differently and three that crashed one of them outright.
///
/// A manifest is the first thing a rider's phone fetches from a network nobody controls, so "does
/// not crash" is the floor and "answers identically" is the contract. Every case is a mutation of
/// the shared fixture, and the verdicts live in the fixture directory rather than here, so the
/// Swift twin cannot quietly diverge by testing a different list.
#[test]
fn the_shared_rejection_corpus_is_answered_identically() {
    let directory: serde_json::Value =
        serde_json::from_slice(&repo_file("specs/vectors/manifest.json")).expect("vector directory");
    let cases = directory["wx_manifest_v2"]["rejection_equivalence"]["cases"].as_array().expect("cases");
    assert!(cases.len() >= 20, "the corpus must not shrink; it is the only pin on rejection parity");
    let base: serde_json::Value = serde_json::from_slice(&shared_fixture()).expect("fixture");

    for case in cases {
        let name = case["name"].as_str().expect("name");
        let mut document = base.clone();
        for op in case["patch"].as_array().expect("patch") {
            apply_patch(&mut document, op);
        }
        // A walker that silently did nothing would pass every `accepted` row. Swift has the same
        // guard; without it on this side, only the `rejected` and `skipped_frames > 0` rows would
        // notice a mis-walked pointer, and the plain-accepted rows would be decoration.
        assert_ne!(document, base, "{name}: the patch changed nothing — check the pointer");
        let bytes = serde_json::to_vec(&document).expect("serialise");
        let parsed = manifest_v2::parse(&bytes);
        match case["verdict"].as_str().expect("verdict") {
            "rejected" => assert!(parsed.is_err(), "{name}: must be refused, parsed instead"),
            "accepted" => {
                let manifest = parsed.unwrap_or_else(|error| panic!("{name}: must parse, got {error}"));
                let expected = case["skipped_frames"].as_u64().unwrap_or(0) as usize;
                assert_eq!(manifest.skipped_frames, expected, "{name}: skipped frames");
            }
            other => panic!("{name}: unknown verdict {other}"),
        }
    }
}

/// The base document must still be the happy path, or every "accepted" row above is vacuous.
#[test]
fn the_rejection_corpus_starts_from_a_document_that_parses_cleanly() {
    assert_eq!(parsed().skipped_frames, 0);
}
