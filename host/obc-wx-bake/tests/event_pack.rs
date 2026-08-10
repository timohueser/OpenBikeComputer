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
    eprintln!("{EVENT_ID} re-bake:\n{}", report.summary());
    // The replay is offline by construction: a `FixtureUpstream` 404s anything it was not given,
    // so a re-bake that reached the network could not have succeeded.
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.published_objects, event.service.len(), "every published object is in the pack");
}

/// Provenance: length + sha256 of every stored member and every baked object.
#[test]
fn every_stored_byte_matches_its_recorded_digest() {
    let event = event();
    let report = pack::verify_digests(&pack_root(), &event).expect("digests");
    eprintln!("{EVENT_ID}: {} digests verified", report.verified);
    assert!(report.verified >= event.service.len() + event.truth_frames.len());
    // The truth ladder's raw observations are recorded but not checked in — deliberately, they
    // are ~450 KB each and nothing here decodes them. They must still carry full provenance.
    for member in rebake::truth_members(&event) {
        assert_eq!(member.role, Role::Truth);
        assert!(member.sha256.is_some(), "{} has no digest", member.url);
        assert!(member.length.is_some_and(|length| length > 0), "{} has no length", member.url);
        assert!(member.archive_url.contains("mtarchive"), "{} is not from the archive", member.archive_url);
    }
    assert!(!report.unmaterialized.is_empty(), "this pack ships with a recorded-only truth ladder");
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
    assert_eq!(product.reference_time, "2020-08-10T18:52:00Z");
    assert_eq!(product.frames.len(), 9);
    assert_eq!(product.frames[0].source_class, SourceClass::Observation);
    assert_eq!(product.frames[0].geometry.cell_size_m, 1_000);
    // The forward frames keep HRRR's own 15-minute steps, which land at 8, 23, 38 ... minutes
    // ahead of an 18:52 observation. Nothing is re-spaced onto a round cadence.
    let forward: Vec<u32> = product.frames[1..].iter().map(|frame| frame.offset_min).collect();
    assert_eq!(forward, vec![8, 23, 38, 53, 68, 83, 98, 113]);
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

    // The observation: a mature derecho over Iowa fills a serious fraction of the window.
    assert!(wet_fraction("service/wx/v1/us/20200810T1852Z/f0.obcg") > 0.02);
    // The model's view of the same storm, and its two-hour-out frame.
    assert!(wet_fraction("service/wx/v1/us/20200810T1852Z/f8.obcg") > 0.02);
    assert!(wet_fraction("service/wx/v1/us/20200810T1852Z/f113.obcg") > 0.005);
    // …and the ground truth at both ends of the ladder.
    assert!(wet_fraction("truth/f14.obcg") > 0.02);
    assert!(wet_fraction("truth/f120.obcg") > 0.005);

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
    let observation =
        obcg::decode_header(read("service/wx/v1/us/20200810T1852Z/f0.obcg")[..obcg::HEADER_LEN].try_into().unwrap())
            .unwrap();
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
