//! The checked-in event pack, held to its own promises.
//!
//! `tests/events/us-derecho-2020-08-10/` is a real cycle of the real service over the real
//! 10 August 2020 Midwest derecho: raw archive bytes in `upstream/`, the tree the baker made of
//! them in `service/`, and what the radar actually saw over the next two hours in `truth/`.
//!
//! Three things are proven here, and together they are what makes the pack usable as evidence:
//!
//! 1. every stored byte still hashes to what `event.json` swears (provenance);
//! 2. re-baking `upstream/` through [`obc_wx_bake::cycle::run_cycle`] reproduces `service/`
//!    **byte for byte** (the baker has not drifted);
//! 3. the frames actually contain the storm — a pack of empty grids would pass (1) and (2) and
//!    be worth nothing.

use std::path::PathBuf;

use obc_formats::obcg;
use obc_formats::precip4;
use obc_wx_bake::manifest::{self, SourceClass};
use obc_wx_bake::pack::{self, rebake, Event, Retrieval, Role};
use obc_wx_bake::source::mrms;

const EVENT_ID: &str = "us-derecho-2020-08-10";

fn pack_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/events").join(EVENT_ID)
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("obc-wx-pack-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn event() -> Event {
    Event::read(&pack_root()).expect("the checked-in pack parses")
}

/// Decode one cell out of a published frame the way a corridor client would.
fn published_cell(bytes: &[u8], col: u32, row: u32) -> u8 {
    let header_bytes: &[u8; obcg::HEADER_LEN] = bytes[..obcg::HEADER_LEN].try_into().unwrap();
    let header = obcg::decode_header(header_bytes).unwrap();
    let (tile_col, tile_row) = header.tile_of_cell(col, row).unwrap();
    let tile_index = header.tile_index(tile_col, tile_row).unwrap();
    let page = header.page_of_entry(tile_index);
    let page_offset = header.page_offset(page).unwrap() as usize;
    let page_slice = &bytes[page_offset..page_offset + header.page_bytes() as usize];
    obcg::validate_page(&header, page_slice).unwrap();
    let within = (tile_index - page * u32::from(header.entries_per_page)) as usize;
    let entry = obcg::decode_entry(page_slice, within).unwrap();
    let payload = if entry.is_dry() {
        &[][..]
    } else {
        &bytes[entry.data_offset as usize..entry.data_offset as usize + usize::from(entry.encoded_len)]
    };
    let mut cells = vec![0u8; header.tile_cells()];
    obcg::decode_tile_cells(&header, &entry, payload, &mut cells).unwrap();
    cells[header.cell_index_in_tile(col, row).unwrap()]
}

/// Every cell of a frame, in OBCG row order (row 0 = south). Decoded tile by tile — a per-cell
/// [`published_cell`] over a 704 x 320 window would re-decode its tile a thousand times over.
fn all_cells(bytes: &[u8]) -> (obcg::Header, Vec<u8>) {
    let header = obcg::decode_header(bytes[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
    let edge = u32::from(header.tile_edge);
    let mut grid = vec![0u8; header.width as usize * header.height as usize];
    let mut tile = vec![0u8; header.tile_cells()];
    for tile_row in 0..header.tile_rows() {
        for tile_col in 0..header.tile_cols() {
            let tile_index = header.tile_index(tile_col, tile_row).unwrap();
            let page = header.page_of_entry(tile_index);
            let page_offset = header.page_offset(page).unwrap() as usize;
            let page_slice = &bytes[page_offset..page_offset + header.page_bytes() as usize];
            obcg::validate_page(&header, page_slice).unwrap();
            let within = (tile_index - page * u32::from(header.entries_per_page)) as usize;
            let entry = obcg::decode_entry(page_slice, within).unwrap();
            let payload = if entry.is_dry() {
                &[][..]
            } else {
                &bytes[entry.data_offset as usize..entry.data_offset as usize + usize::from(entry.encoded_len)]
            };
            obcg::decode_tile_cells(&header, &entry, payload, &mut tile).unwrap();
            for row in 0..edge.min(header.height - tile_row * edge) {
                for col in 0..edge.min(header.width - tile_col * edge) {
                    let global_col = tile_col * edge + col;
                    let global_row = tile_row * edge + row;
                    grid[(global_row * header.width + global_col) as usize] = tile[(row * edge + col) as usize];
                }
            }
        }
    }
    // One spot check against the single-cell reader, so the fast path cannot silently disagree
    // with how a corridor client actually reads a frame.
    let (col, row) = (header.width / 3, header.height / 3);
    assert_eq!(grid[(row * header.width + col) as usize], published_cell(bytes, col, row));
    (header, grid)
}

fn read(relative: &str) -> Vec<u8> {
    let path = pack::resolve(&pack_root(), relative).expect("pack-relative path");
    std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// The headline check: the pack's `service/` is exactly what the baker makes of its `upstream/`.
#[test]
fn the_pack_rebakes_byte_identically() {
    let event = event();
    let report = rebake::verify_rebake(&pack_root(), &event, &scratch("rebake")).expect("the pack re-bakes");
    eprintln!("{EVENT_ID} re-bake:\n{}", report.cycle.summary());
    assert!(report.cycle.warnings.is_empty(), "{:?}", report.cycle.warnings);
    assert_eq!(report.cycle.published_objects, event.service.len(), "every published object is in the pack");

    // Hermeticity, asserted rather than asserted about. `verify_rebake` already refuses a request
    // no member accounts for; here the *converse* is checked, so a pack cannot pass by carrying
    // members the bake never reads either.
    eprintln!("{EVENT_ID} replay made {} requests", report.requests.len());
    assert!(!report.requests.is_empty());
    for member in event.service_members() {
        let expected = match &member.retrieval {
            Retrieval::Probe { .. } => format!("HEAD {}", member.url),
            Retrieval::Body => member.url.clone(),
            Retrieval::Range { start, end_inclusive, .. } => format!("{}#{start}-{end_inclusive}", member.url),
        };
        assert!(report.requests.contains(&expected), "the replay never asked for {expected} — a dead member");
    }
}

/// **F1: the pack must not contain the future.** Every service member's key states when its bytes
/// were published, and no service member may have been published after the capture instant.
///
/// This is the property the shipped pack originally violated: replayed against an archive that
/// already holds the whole day, run discovery picked an HRRR run and an MRMS observation that did
/// not exist yet, so the pack carried a model baseline with an extra hour of assimilation and
/// radar the device could not have had. A nowcaster scored against `truth/` on that basis measures
/// something no device will ever see.
#[test]
fn no_service_member_was_published_after_the_capture_instant() {
    let event = event();
    let at = manifest::parse_rfc3339(&event.bake.now).expect("bake.now is RFC 3339");
    let mut suppressed = 0usize;
    for member in event.service_members() {
        let published =
            pack::archive::published_at(&member.url).unwrap_or_else(|error| panic!("{}: {error}", member.url));
        let exists = match &member.retrieval {
            Retrieval::Probe { object_length } => object_length.is_some(),
            _ => true,
        };
        if !exists {
            // A probe that found nothing is the guard doing its work — and the only kind of
            // service member allowed to name an object from the future.
            suppressed += 1;
            continue;
        }
        assert!(
            published <= at,
            "{} was published at {} — after the capture instant {}",
            member.url,
            manifest::rfc3339(published),
            event.bake.now
        );
    }
    assert!(suppressed > 0, "a capture that suppressed nothing has not exercised the as-of guard at all");
    eprintln!("{EVENT_ID}: {suppressed} probes found nothing, which is the guard's fallback in the document");

    // The truth ladder is the deliberate exception: it *is* the future, and every rung must be.
    for frame in &event.truth_frames {
        let valid = manifest::parse_rfc3339(&frame.valid_at).unwrap();
        assert!(valid > at, "truth frame {} is not ahead of the capture instant", frame.path);
    }
}

/// The pack states the ground it covers and the basemap that ground needs, so a future US pack
/// drifting off the one map the bakery carries is visible in the document.
#[test]
fn the_pack_stays_on_the_basemap_it_names() {
    let event = event();
    assert_eq!(event.basemap_region, pack::US_BASEMAP_REGION);
    let map = pack::US_BASEMAP_BBOX;
    let coverage = event.coverage_udeg;
    let request = event.bake.bbox_udeg.expect("this pack is cropped");

    // What this actually bounds, stated honestly. Coverage exceeds the basemap for two independent
    // reasons, and only one of them is tile alignment:
    //
    //   1. the *requested* `--bbox` may already sit outside Iowa — for this pack the request's east
    //      edge is 90.000 W against Iowa's 90.140 W, so +0.140 of the +0.200 total east overhang is
    //      the request, before a single tile is aligned;
    //   2. crops then align outward to whole tiles of each frame's own lattice, adding up to one
    //      more tile stride (0.64 degrees on the 1 km observation).
    //
    // So the bound is `request overshoot + one tile`, not `one tile`. It is still a real tripwire —
    // it is what fails if someone captures a Kansas storm against the Iowa basemap — but it does not
    // claim tile alignment is the only thing being tolerated.
    let tile = i64::from(mrms::GEOMETRY.tile_edge) * i64::from(mrms::GEOMETRY.cell_lat_udeg);
    assert_eq!(tile, 640_000, "the observation's tile stride is 0.64 degrees");
    for (edge, over, requested_over) in [
        ("south", map.south_udeg - coverage.south_udeg, map.south_udeg - request.south_udeg),
        ("west", map.west_udeg - coverage.west_udeg, map.west_udeg - request.west_udeg),
        ("north", coverage.north_udeg - map.north_udeg, request.north_udeg - map.north_udeg),
        ("east", coverage.east_udeg - map.east_udeg, request.east_udeg - map.east_udeg),
    ] {
        let budget = requested_over.max(0) + tile;
        assert!(
            over <= budget,
            "coverage reaches {:.3} degrees past the {edge} edge of {}, beyond the {:.3} the request \
             ({:.3}) plus one tile allows — this pack needs a basemap conversation, not a quiet capture",
            over as f64 / 1e6,
            pack::US_BASEMAP_REGION,
            budget as f64 / 1e6,
            requested_over.max(0) as f64 / 1e6,
        );
    }
    // The requested window is itself the thing a human chose, so hold *it* to the basemap directly:
    // a request that wandered off Iowa is the drift this convention exists to notice, and tile
    // alignment is no excuse for it.
    for (edge, over) in [
        ("south", map.south_udeg - request.south_udeg),
        ("west", map.west_udeg - request.west_udeg),
        ("north", request.north_udeg - map.north_udeg),
        ("east", request.east_udeg - map.east_udeg),
    ] {
        assert!(
            over <= tile,
            "the requested --bbox reaches {:.3} degrees past the {edge} edge of {}",
            over as f64 / 1e6,
            pack::US_BASEMAP_REGION
        );
    }
    // And it must actually overlap the map, not merely sit near it.
    assert!(coverage.south_udeg < map.north_udeg && coverage.north_udeg > map.south_udeg);
    assert!(coverage.west_udeg < map.east_udeg && coverage.east_udeg > map.west_udeg);
}

/// Provenance: length + sha256 of every stored member and every baked object.
#[test]
fn every_stored_byte_matches_its_recorded_digest() {
    let event = event();
    let report = pack::verify_digests(&pack_root(), &event).expect("digests");
    eprintln!("{EVENT_ID}: {} digests verified", report.verified);
    assert!(report.verified >= event.service.len() + event.truth_frames.len());
    for member in rebake::truth_members(&event) {
        assert_eq!(member.role, Role::Truth);
        assert!(member.sha256.is_some(), "{} has no digest", member.url);
        assert!(member.length.is_some_and(|length| length > 0), "{} has no length", member.url);
        assert!(member.archive_url.contains("mtarchive"), "{} is not from the archive", member.archive_url);
    }
}

/// **The pack has no external dependency left.** Every member's bytes are checked in — including
/// the truth ladder's eight raw MRMS observations, which an earlier round shipped as
/// `stored: false` provenance only.
///
/// That was the wrong trade for a fixture whose whole point is durability. `service/` was already
/// a pure re-run of checked-in bytes, but `truth/` was eight *baked artifacts* whose sources lived
/// on a single free mirror — so the lattice and quantization work ahead would have meant
/// re-fetching 4.3 MB from MTArchive to re-derive them. 4.3 MB in git is the cheaper half of that
/// trade by a wide margin.
#[test]
fn nothing_in_the_pack_has_to_be_fetched() {
    let event = event();
    let missing = rebake::unmaterialized(&event);
    assert!(
        missing.is_empty(),
        "these members still have to come from the network: {:?}",
        missing.iter().map(|member| member.path.as_deref().unwrap_or("?")).collect::<Vec<_>>()
    );
    let report = pack::verify_digests(&pack_root(), &event).expect("digests");
    assert!(report.unmaterialized.is_empty());
    // And the ladder's raw sources really are the checked-in half, not an empty set.
    let truth_bytes: u64 = rebake::truth_members(&event).filter_map(|member| member.length).sum();
    eprintln!("{EVENT_ID}: {truth_bytes} bytes of truth upstream checked in");
    assert!(truth_bytes > 4_000_000, "the truth ladder's raw observations are ~450 KB each");
}

/// …and because they are checked in, `truth/` is now a pure re-run too: eight observed frames
/// re-derived offline from the pack's own bytes and byte-compared. A change to the observation
/// lattice or the quantization table fails here rather than leaving eight stale frames behind.
#[test]
fn the_truth_ladder_rebakes_byte_identically() {
    let event = event();
    let compared = rebake::verify_truth_rebake(&pack_root(), &event).expect("the truth ladder re-bakes");
    assert_eq!(compared, event.truth_frames.len());
    eprintln!("{EVENT_ID}: {compared} truth frames re-derived from checked-in bytes");
}

/// The archive is not the upstream: every member names both, and the *canonical* URL is what the
/// replay serves. Getting this backwards is exactly what would make a pack unreplayable once the
/// live MRMS bucket rolls over.
#[test]
fn members_record_the_canonical_url_and_the_archive_it_came_from() {
    let event = event();
    let mut rewritten = 0usize;
    for member in &event.members {
        assert_eq!(
            pack::archive::archive_url(&member.url).expect("a mapped source"),
            member.archive_url,
            "{} does not rewrite to its recorded archive URL",
            member.url
        );
        assert!(member.licence.contains("NOAA"), "{}: {}", member.url, member.licence);
        if member.url != member.archive_url {
            rewritten += 1;
            assert!(member.url.starts_with("https://noaa-mrms-pds"), "{}", member.url);
        }
    }
    assert!(rewritten > 0, "the MRMS members must come from MTArchive, not the short-retention bucket");
    // The HRRR objects are 150-200 MB and are never checked in whole; only the `.idx`-selected
    // messages are, as explicit byte ranges.
    let ranges: Vec<&pack::Member> =
        event.members.iter().filter(|member| matches!(member.retrieval, Retrieval::Range { .. })).collect();
    assert_eq!(ranges.len(), 8, "eight HRRR PRATE messages");
    assert!(
        ranges.iter().all(|member| member.url.contains("hrrr.t17z.")),
        "the as-of guard's fallback: at 18:52 the newest *complete* HRRR run is 17Z"
    );
    for member in ranges {
        let Retrieval::Range { object_length, start, end_inclusive } = member.retrieval else { unreachable!() };
        assert!(object_length > 100_000_000, "{}: the whole object is {object_length} bytes", member.url);
        assert_eq!(member.length, Some(end_inclusive - start + 1));
        assert!(member.path.as_deref().unwrap().contains(&format!("@{start}-{end_inclusive}")));
    }
}

/// The composed timeline the pack froze: one MRMS observation anchoring eight HRRR forward
/// frames at their own real valid times, on their own native lattices.
#[test]
fn the_frozen_manifest_is_the_real_composed_us_timeline() {
    let event = event();
    let document = manifest::from_json(&read(&format!("service/{}", event.manifest_key))).expect("manifest parses");
    assert_eq!(document.products.len(), 1);
    let product = &document.products[0];
    assert_eq!(product.id, "us");
    // 18:48, not the 18:52 the capture was clocked at: MRMS takes ~3 minutes to publish, so 18:48
    // is the newest observation that existed at the capture instant. The as-of guard is what makes
    // that the answer rather than a wish.
    assert_eq!(product.reference_time, "2020-08-10T18:48:00Z");
    assert_eq!(product.frames.len(), 9);
    assert_eq!(product.frames[0].source_class, SourceClass::Observation);
    assert_eq!(product.frames[0].geometry.cell_size_m, 1_000);
    // The forward frames keep HRRR's own 15-minute steps — the 17Z run's +120..+225 leads, which
    // land 12, 27, 42 ... minutes ahead of an 18:48 observation. Nothing is re-spaced onto a round
    // cadence, and the run is 17Z because the 18Z set was still publishing.
    let forward: Vec<u32> = product.frames[1..].iter().map(|frame| frame.offset_min).collect();
    assert_eq!(forward, vec![12, 27, 42, 57, 72, 87, 102, 117]);
    for frame in &product.frames[1..] {
        assert_eq!(frame.source_class, SourceClass::Forecast);
        assert_eq!(frame.geometry.cell_size_m, 3_000);
    }
    // The crop is honest in the manifest: the product bbox is the cropped window, not CONUS.
    let bbox = event.bake.bbox_udeg.expect("this pack is cropped");
    assert!(product.bbox_udeg.south_udeg <= bbox.south_udeg && product.bbox_udeg.north_udeg >= bbox.north_udeg);
    assert!(product.bbox_udeg.west_udeg <= bbox.west_udeg && product.bbox_udeg.east_udeg >= bbox.east_udeg);
    assert!(product.bbox_udeg.north_udeg - product.bbox_udeg.south_udeg < 5_000_000, "the crop must be corridor-sized");
}

/// A pack of empty grids would pass every structural check above and be worthless. The derecho
/// has to actually be in the bytes — in the observation, in the forecast, and in the truth ladder.
#[test]
fn the_frames_actually_contain_the_storm() {
    let event = event();
    let mut scratch_buffer = vec![0u8; precip4::MAX_CELLS];

    let mut wet_fraction = |relative: &str| -> f64 {
        let bytes = read(relative);
        obcg::validate(&bytes, &mut scratch_buffer).unwrap_or_else(|error| panic!("{relative}: {error:?}"));
        let (header, cells) = all_cells(&bytes);
        let wet =
            cells.iter().filter(|cell| **cell != precip4::INTENSITY_DRY && **cell != precip4::INTENSITY_NODATA).count();
        eprintln!(
            "{relative}: {}x{} cells, {:.2}% wet",
            header.width,
            header.height,
            100.0 * wet as f64 / cells.len() as f64
        );
        wet as f64 / cells.len() as f64
    };

    // The floors are close to the measured values, not decorative. An earlier round used `> 0.02`
    // against a 36.9 % observation — a pack that lost nine tenths of its precipitation would have
    // sailed through. These sit roughly a quarter below what the bytes actually contain, which
    // notices a real regression while leaving room for a re-capture's honest drift.
    let document = manifest::from_json(&read(&format!("service/{}", event.manifest_key))).expect("manifest parses");
    let product = &document.products[0];
    let key = |index: usize| format!("service/{}", product.frames[index].key);

    // The observation: a mature derecho fills a third of the window. Measured 36.92 %.
    assert!(wet_fraction(&key(0)) > 0.28, "the observation frame lost its storm");
    // The model's view of the same storm at its first step, and at the far end of the window.
    // Measured 15.06 % and 8.57 % — the forecast lattice is coarser and the storm leaves eastward.
    assert!(wet_fraction(&key(1)) > 0.11, "the first forecast frame lost its storm");
    assert!(wet_fraction(&key(product.frames.len() - 1)) > 0.06, "the last forecast frame lost its storm");
    // …and the ground truth at both ends of the ladder. Measured 37.90 % and 18.26 %.
    let truth = |index: usize| event.truth_frames[index].path.clone();
    assert!(wet_fraction(&truth(0)) > 0.28, "the first truth frame lost its storm");
    assert!(wet_fraction(&truth(event.truth_frames.len() - 1)) > 0.13, "the last truth frame lost its storm");

    // Truth frames are observations on the observation lattice, stamped with the cycle's anchor,
    // so a later scorer can line one up against the forecast frame nearest it without arithmetic.
    for frame in &event.truth_frames {
        let bytes = read(&frame.path);
        let header = obcg::validate(&bytes, &mut scratch_buffer).unwrap();
        assert_eq!(header.flags, obcg::FLAG_OBSERVED, "{}", frame.path);
        assert_eq!(header.product_id, obcg::PRODUCT_MRMS);
        assert_eq!(header.reference_time, manifest::parse_rfc3339(&event.window_start).unwrap());
        assert_eq!(manifest::rfc3339(header.valid_at), frame.valid_at);
        assert_eq!(header.valid_at - header.reference_time, i64::from(frame.offset_min) * 60);
        assert_eq!(header.cell_size_m, 1_000);
        // The requested ladder is +15 min steps; MRMS publishes every two minutes, so odd
        // requests floor onto the cadence and the pack records both numbers.
        assert!(frame.offset_min <= frame.requested_offset_min);
        assert!(i64::from(frame.requested_offset_min - frame.offset_min) * 60 < event.truth.cadence_seconds);
    }

    // The observation frame and the truth frames share one lattice: same origin, same cell size,
    // same dimensions. Scoring is then a cell-by-cell comparison, with no resampling in between.
    let observation = obcg::decode_header(read(&key(0))[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
    for frame in &event.truth_frames {
        let truth = obcg::decode_header(read(&frame.path)[..obcg::HEADER_LEN].try_into().unwrap()).unwrap();
        assert_eq!(
            (truth.south_lat_udeg, truth.west_lon_udeg, truth.width, truth.height),
            (observation.south_lat_udeg, observation.west_lon_udeg, observation.width, observation.height),
            "{} is not on the observation lattice",
            frame.path
        );
    }
}

/// Rewrite the pack's baked halves after a deliberate format change, the way
/// `cargo test -p obc-vectors regenerate -- --ignored` rewrites `specs/vectors/`:
/// `cargo test -p obc-wx-bake --test event_pack regenerate -- --ignored`.
///
/// `upstream/` is never touched — those are the archive's bytes, the pack's whole point. Only
/// `service/`, `truth/` and the digests `event.json` swears to them are re-derived, from the
/// checked-in upstream and through the production bake. The checks above then re-prove the
/// result, so this hook can only ever move the pack to what today's baker really produces.
#[test]
#[ignore]
fn regenerate() {
    let root = pack_root();
    let mut event = event();
    let scratch = scratch("regenerate");

    // service/: the whole tree, replaced with what the baker makes of upstream/ today.
    rebake::bake_into(&root, &event, &scratch).expect("the pack re-bakes");
    let rebaked = pack::read_tree(&scratch).expect("the fresh service tree");
    std::fs::remove_dir_all(root.join("service")).expect("clear the stale service tree");
    for (key, bytes) in &rebaked {
        pack::write_file(&root.join("service").join(key), bytes).expect("write the service object");
    }
    event.service = rebaked
        .iter()
        .map(|(key, bytes)| pack::ServiceObject {
            key: key.clone(),
            bytes: bytes.len() as u64,
            sha256: pack::sha256(bytes),
        })
        .collect();

    // truth/: each observed frame re-derived from the pack's own stored MRMS bytes.
    let anchor = manifest::parse_rfc3339(&event.window_start).expect("window_start");
    let mut upstream = rebake::truth_upstream(&root, &event).expect("the truth replay");
    for frame in &mut event.truth_frames {
        let valid_at = manifest::parse_rfc3339(&frame.valid_at).expect("valid_at");
        let baked =
            pack::capture::bake_truth_frame(&mut upstream, anchor, frame.offset_min, valid_at, event.bake.bbox_udeg)
                .expect("truth frame re-bakes");
        pack::write_file(&pack::resolve(&root, &frame.path).expect("truth path"), &baked).expect("write");
        frame.bytes = baked.len() as u64;
        frame.sha256 = pack::sha256(&baked);
    }

    event.write(&root).expect("event.json");
    eprintln!(
        "{EVENT_ID}: {} service objects and {} truth frames rewritten",
        event.service.len(),
        event.truth_frames.len()
    );
}

/// The storm moved: consecutive truth frames must differ. A capture bug that fetched the same
/// object eight times would otherwise sail through every check above.
#[test]
fn the_truth_ladder_is_eight_different_moments() {
    let event = event();
    let digests: Vec<&str> = event.truth_frames.iter().map(|frame| frame.sha256.as_str()).collect();
    let mut unique = digests.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), digests.len(), "two truth frames are byte-identical");

    let first = all_cells(&read(&event.truth_frames[0].path)).1;
    let last = all_cells(&read(&event.truth_frames[event.truth_frames.len() - 1].path)).1;
    let changed = first.iter().zip(&last).filter(|(a, b)| a != b).count();
    eprintln!("truth ladder: {changed} of {} cells changed over the window", first.len());
    assert!(changed > first.len() / 20, "the storm barely moved across two hours — suspicious");
}
