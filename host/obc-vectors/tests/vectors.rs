//! Contract tests over the checked-in `specs/vectors/` fixtures: every file must
//! equal its spec-derived builder byte-for-byte, and the route vectors must load and
//! ride through `obc-route`. The app's `swift test` consumes the same files.

use obc_elevation::{TerrainReader, TileCache};
use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_formats::track::RECORD_LEN as TRACK_RECORD_LEN;
use obc_route::{for_each_waypoint, track_to_gpx, RouteIndex, RouteObjectInfo, RouteReader, MAX_POINTS_PER_CHUNK};
use obc_vectors::{
    all, crc32, dir, ride_v1, ride_v2, terrain_coord, terrain_height, terrain_shard, TERRAIN_CELL_LOG2,
    TERRAIN_CELL_MIN_I, TERRAIN_CELL_MIN_J, TERRAIN_COLS, TERRAIN_NODATA_AT, TERRAIN_POSTING_LOG2, TERRAIN_ROWS,
    TRACK_NAME, TRIP_DANGLING_STAGE, TRIP_ID, TRIP_NAME, TRIP_STAGE_IDS,
};
use obc_weather::WeatherReader;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| {
        panic!("fixture {name} unreadable ({e}) — run `cargo test -p obc-vectors regenerate -- --ignored`")
    })
}

/// An in-memory sink for re-running a streaming converter against a checked-in fixture.
#[derive(Default)]
struct VecSink {
    buf: Vec<u8>,
}

impl ByteSink for VecSink {
    fn write(&mut self, b: &[u8]) -> Result<(), Error> {
        self.buf.extend_from_slice(b);
        Ok(())
    }
    fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
        let o = off as usize;
        self.buf[o..o + b.len()].copy_from_slice(b);
        Ok(())
    }
}

/// Spec §6's pinned check value — validates the vector crate's own CRC reference.
#[test]
fn crc32_check_value() {
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
}

#[test]
fn weather_vectors_cross_the_production_reader_and_reject_malformed_files() {
    for (name, expected) in obc_vectors::obcw::positives() {
        let bytes = fixture(name);
        assert_eq!(bytes, expected, "positive OBCW fixture drift: {name}");
        let source = SliceSource(&bytes);
        let reader = WeatherReader::open(&source).unwrap_or_else(|error| panic!("{name} rejected: {error:?}"));
        assert_eq!(reader.header().total_len as usize, bytes.len());
        assert_eq!(reader.hourly(0).unwrap().valid_time_offset_s, 0);
    }

    let dwd = fixture("weather-dwd-96x96-9f.obcw");
    let dwd_source = SliceSource(&dwd);
    let dwd_reader = WeatherReader::open(&dwd_source).unwrap();
    assert_eq!(dwd.len(), 46_480, "the spec's 45.39 KiB DWD-shaped raw estimate");
    assert_eq!(dwd_reader.header().frame_count, 9);
    assert_eq!(dwd_reader.frame(0).unwrap().tile_count, 36);

    let maximum = fixture("weather-max-policy.obcw");
    assert_eq!(maximum.len(), obc_vectors::obcw::PRODUCER_POLICY_MAX_LEN);
    let maximum_source = SliceSource(&maximum);
    WeatherReader::open(&maximum_source).expect("the exact policy-boundary object is valid");

    let latent = fixture("weather-latent-observation.obcw");
    let latent_source = SliceSource(&latent);
    let latent_reader = WeatherReader::open(&latent_source).expect("pre-hourly-base observation is valid");
    assert_eq!(latent_reader.frame(0).unwrap().valid_at, latent_reader.header().valid_from - 4 * 3_600);

    for (name, expected) in obc_vectors::obcw::negatives() {
        let bytes = fixture(name);
        assert_eq!(bytes, expected, "negative OBCW fixture drift: {name}");
        if name != "weather-invalid-truncated.obcw" && name != "weather-invalid-crc.obcw" {
            let stored = u32::from_le_bytes(
                bytes[obc_formats::obcw::HDR_CRC32..obc_formats::obcw::HDR_CRC32 + 4].try_into().unwrap(),
            );
            let mut crc_bytes = bytes.clone();
            crc_bytes[obc_formats::obcw::HDR_CRC32..obc_formats::obcw::HDR_CRC32 + 4].fill(0);
            assert_eq!(stored, crc32(&crc_bytes), "structural negative must carry a valid CRC: {name}");
        }
        let source = SliceSource(&bytes);
        assert!(WeatherReader::open(&source).is_err(), "malformed OBCW fixture accepted: {name}");
    }
}

#[test]
fn grid_vectors_cross_the_byte_authority_and_reject_malformed_objects() {
    use obc_formats::obcg;

    for (name, expected) in obc_vectors::obcg::positives() {
        let bytes = fixture(name);
        assert_eq!(bytes, expected, "positive OBCG fixture drift: {name}");
        let mut scratch = vec![0u8; obc_formats::precip4::MAX_CELLS];
        let header = obcg::validate(&bytes, &mut scratch).unwrap_or_else(|error| panic!("{name} rejected: {error:?}"));
        assert_eq!(header.total_len as usize, bytes.len());
    }

    // Pinned cells: the Swift decoder must agree on exactly these values (OBCG_Spec.md §11).
    let sample = |bytes: &[u8], col: u32, row: u32| -> u8 {
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
        let mut out = vec![0u8; header.tile_cells()];
        obcg::decode_tile_cells(&header, &entry, payload, &mut out).unwrap();
        out[header.cell_index_in_tile(col, row).unwrap()]
    };
    let multipage = fixture("grid-multipage.obcg");
    assert_eq!(sample(&multipage, 0, 0), 6);
    assert_eq!(sample(&multipage, 39, 39), 9);
    assert_eq!(sample(&multipage, 20, 20), 0, "dry sentinel decodes as dry");
    let raw = fixture("grid-raw-tile.obcg");
    assert_eq!(sample(&raw, 0, 0), 11);
    assert_eq!(sample(&raw, 12, 0), 9);
    assert_eq!(sample(&raw, 5, 3), 12);
    let nodata = fixture("grid-nodata-tile.obcg");
    assert_eq!(sample(&nodata, 8, 8), obc_formats::precip4::INTENSITY_NODATA, "no-data is never dry");
    let padding = fixture("grid-edge-padding.obcg");
    assert_eq!(sample(&padding, 3, 4), 2);
    assert_eq!(sample(&padding, 20, 20), 12);
    let rle_wins = fixture("grid-rle-wins.obcg");
    assert_eq!(sample(&rle_wins, 0, 0), 0);
    assert_eq!(sample(&rle_wins, 8, 0), 1, "the second eight-cell run");
    let deflated = fixture("grid-deflate-tile.obcg");
    assert_eq!(sample(&deflated, 0, 0), 0);
    assert_eq!(sample(&deflated, 7, 7), 0, "the whole 8 x 8 source cell carries one value");
    assert_eq!(sample(&deflated, 8, 8), 9, "block (1, 1) of the upsampled source");
    assert_eq!(sample(&deflated, 63, 63), ((7 * 8 + 7) % 13) as u8);

    // §5's codec choice, pinned per vector: the tie goes to RLE4, varied short runs stay RLE4,
    // pseudo-random cells stay raw4, and only the shapes deflate genuinely beats use codec 2.
    let codec_of = |bytes: &[u8]| -> u8 {
        let header_bytes: &[u8; obcg::HEADER_LEN] = bytes[..obcg::HEADER_LEN].try_into().unwrap();
        let header = obcg::decode_header(header_bytes).unwrap();
        let page_offset = header.page_offset(0).unwrap() as usize;
        let page_slice = &bytes[page_offset..page_offset + header.page_bytes() as usize];
        obcg::decode_entry(page_slice, 0).unwrap().codec
    };
    for (name, expected) in [
        ("grid-raw-tile.obcg", obcg::CODEC_RAW4),
        ("grid-rle-tile.obcg", obcg::CODEC_RLE4),
        ("grid-rle-wins.obcg", obcg::CODEC_RLE4),
        ("grid-deflate-tile.obcg", obcg::CODEC_DEFLATE4),
        ("grid-deflate-padding-bits.obcg", obcg::CODEC_DEFLATE4),
        ("grid-deflate-edge256.obcg", obcg::CODEC_DEFLATE4),
        ("grid-nodata-tile.obcg", obcg::CODEC_DEFLATE4),
    ] {
        assert_eq!(codec_of(&fixture(name)), expected, "codec drift: {name}");
    }

    // §5: the padding bits of a deflate stream's last byte are not data. This vector differs from
    // `grid-deflate-tile` in exactly those bits, so a decoder that validated them would reject a
    // conforming object — and it must decode to the identical cells.
    let padded = fixture("grid-deflate-padding-bits.obcg");
    assert_ne!(padded, deflated, "the padding-bit vector is a different byte image");
    assert_eq!(padded.len(), deflated.len());
    for (col, row) in [(0u32, 0u32), (8, 8), (63, 63), (17, 41)] {
        assert_eq!(sample(&padded, col, row), sample(&deflated, col, row), "cell ({col}, {row})");
    }

    // The production geometry: one 65,536-cell tile whose payload needs the directory's uint16.
    let edge256 = fixture("grid-deflate-edge256.obcg");
    let edge256_header: &[u8; obcg::HEADER_LEN] = edge256[..obcg::HEADER_LEN].try_into().unwrap();
    let edge256_header = obcg::decode_header(edge256_header).unwrap();
    assert_eq!(edge256_header.tile_edge, 256);
    assert_eq!(edge256_header.tile_cells(), 65_536);
    assert!(edge256_header.data_len > 255, "the only payload longer than a uint8: {}", edge256_header.data_len);
    assert!(edge256_header.data_len < 32_768, "and still under §5's pre-inflate ceiling");
    assert_eq!(sample(&edge256, 0, 0), sample(&edge256, 15, 15), "one 16 x 16 block, one value");
    // The whole point of codec 2: 4,096 upsampled cells cost 2,048 raw4 bytes and 512 RLE4 bytes.
    let deflate_header: &[u8; obcg::HEADER_LEN] = deflated[..obcg::HEADER_LEN].try_into().unwrap();
    let deflate_payload = obcg::decode_header(deflate_header).unwrap().data_len;
    assert!(deflate_payload < 128, "the upsampled tile compresses below 128 bytes: {deflate_payload}");

    for (name, expected) in obc_vectors::obcg::negatives() {
        let bytes = fixture(name);
        assert_eq!(bytes, expected, "negative OBCG fixture drift: {name}");
        let mut scratch = vec![0u8; obc_formats::precip4::MAX_CELLS];
        assert!(obcg::validate(&bytes, &mut scratch).is_err(), "malformed OBCG fixture accepted: {name}");
    }
}

/// OBCG_Spec.md §7: corridor extraction touches only the header, the covering directory pages,
/// and the needed non-dry tiles. A counting range reader proves the request set exactly.
#[test]
fn grid_corridor_extraction_touches_only_header_pages_and_needed_tiles() {
    use obc_formats::obcg;
    use std::cell::RefCell;

    let bytes = fixture("grid-multipage.obcg");
    let requests: RefCell<Vec<(u32, u32)>> = RefCell::new(Vec::new());
    let read = |offset: u32, len: u32| -> Vec<u8> {
        requests.borrow_mut().push((offset, len));
        bytes[offset as usize..(offset + len) as usize].to_vec()
    };

    // Corridor: the north-east quarter of the window — cells (20, 20) through (39, 39).
    let header_read = read(0, obcg::HEADER_LEN as u32);
    let header_bytes: &[u8; obcg::HEADER_LEN] = header_read.as_slice().try_into().unwrap();
    let header = obcg::decode_header(header_bytes).unwrap();
    let (tile_col_min, tile_row_min) = header.tile_of_cell(20, 20).unwrap();
    let (tile_col_max, tile_row_max) = header.tile_of_cell(39, 39).unwrap();
    assert_eq!((tile_col_min, tile_row_min, tile_col_max, tile_row_max), (1, 1, 2, 2));

    // Covering pages, computed from the header alone, fetched once each.
    let mut pages: Vec<u32> = Vec::new();
    for tile_row in tile_row_min..=tile_row_max {
        for tile_col in tile_col_min..=tile_col_max {
            let page = header.page_of_entry(header.tile_index(tile_col, tile_row).unwrap());
            if !pages.contains(&page) {
                pages.push(page);
            }
        }
    }
    assert_eq!(pages, vec![2, 3, 4], "tiles 4, 5, 7, 8 at two entries per page");
    let mut wet_cells = 0usize;
    for page in &pages {
        let page_read = read(header.page_offset(*page).unwrap(), header.page_bytes());
        obcg::validate_page(&header, &page_read).unwrap();
        for tile_row in tile_row_min..=tile_row_max {
            for tile_col in tile_col_min..=tile_col_max {
                let tile_index = header.tile_index(tile_col, tile_row).unwrap();
                if header.page_of_entry(tile_index) != *page {
                    continue;
                }
                let within = (tile_index - page * u32::from(header.entries_per_page)) as usize;
                let entry = obcg::decode_entry(&page_read, within).unwrap();
                if entry.is_dry() {
                    continue; // a dry tile costs no tile read
                }
                let payload = read(entry.data_offset, u32::from(entry.encoded_len));
                let mut out = vec![0u8; header.tile_cells()];
                obcg::decode_tile_cells(&header, &entry, &payload, &mut out).unwrap();
                wet_cells += out.iter().filter(|&&cell| cell != 0 && cell != 15).count();
            }
        }
    }
    assert_eq!(wet_cells, 1, "exactly the north-east corner cell");

    // The complete request ledger: one header read, the three covering page reads, and one tile
    // read per non-dry needed tile — tiles 5 and 7 are edge tiles (no-data padding, so encoded)
    // and tile 8 holds the wet corner; interior tile 4 is a dry sentinel and costs nothing.
    let ledger = requests.borrow();
    assert_eq!(ledger.len(), 7, "request ledger: {ledger:?}");
    assert_eq!(ledger[0], (0, obcg::HEADER_LEN as u32));
    let page_reads: Vec<(u32, u32)> =
        pages.iter().map(|page| (header.page_offset(*page).unwrap(), header.page_bytes())).collect();
    let mut tile_reads = 0usize;
    for request in ledger.iter().skip(1) {
        if page_reads.contains(request) {
            continue;
        }
        // Every remaining request is a tile payload read inside the data section.
        let (tile_offset, tile_len) = *request;
        assert!(tile_offset >= header.data_offset, "tile read lies in the data section: {request:?}");
        assert!(u64::from(tile_offset) + u64::from(tile_len) <= u64::from(header.total_len));
        tile_reads += 1;
    }
    assert_eq!(tile_reads, 3, "request ledger: {ledger:?}");
    // On this deliberately tiny fixture the directory dominates, so the byte ratio is not the
    // point — the exact request set above is. Still pin that no request was a whole-object read.
    let fetched: u32 = ledger.iter().map(|(_, len)| len).sum();
    assert!(fetched < bytes.len() as u32, "corridor fetched {fetched} of {} bytes", bytes.len());
    assert!(ledger.iter().all(|&(_, len)| (len as usize) < bytes.len()));
}

/// Every checked-in fixture equals its builder's output. A failure is either codec
/// drift (fix the code) or a deliberate spec change (regenerate + flag the app side).
#[test]
fn fixtures_match_the_spec_builders() {
    for (name, bytes) in all() {
        assert_eq!(fixture(name), bytes, "{name} drifted from the spec builder");
    }
}

/// The route fixtures load through the production reader; the waypoint-bearing one
/// rides identically to its plain twin (OBCR v2's storage-only guarantee).
#[test]
fn route_vectors_load_and_ride_identically() {
    let with = fixture("route-waypoints.obcr");
    let plain = fixture("route-plain.obcr");
    let (src_w, src_p) = (SliceSource(&with), SliceSource(&plain));

    let idx_w = RouteIndex::read(&src_w).unwrap();
    let idx_p = RouteIndex::read(&src_p).unwrap();
    assert_eq!(idx_w.name(), "Vector Loop");
    assert_eq!(idx_w.name(), idx_p.name());
    assert_eq!(idx_w.point_count, idx_p.point_count);
    assert_eq!(idx_w.total_distance_m, idx_p.total_distance_m);
    assert_eq!(idx_w.total_ascent_m, idx_p.total_ascent_m);
    assert_eq!(idx_w.chunks().len(), idx_p.chunks().len());

    let (r_w, r_p) = (RouteReader::new(&idx_w, &src_w), RouteReader::new(&idx_p, &src_p));
    let mut a = heapless::Vec::<_, MAX_POINTS_PER_CHUNK>::new();
    let mut b = heapless::Vec::<_, MAX_POINTS_PER_CHUNK>::new();
    for k in 0..idx_w.chunks().len() {
        r_w.decode_chunk(k, &mut a).unwrap();
        r_p.decode_chunk(k, &mut b).unwrap();
        assert_eq!(a, b, "chunk {k} diverged");
    }

    // Waypoints: two, sorted into ride order (the GPX lists them reversed).
    let mut names = Vec::new();
    let mut last_along = 0;
    let count = for_each_waypoint(&src_w, |w| {
        assert!(w.dist_along_m >= last_along, "not sorted");
        last_along = w.dist_along_m;
        names.push(w.name.to_string());
    })
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(names, ["Brunnen", "Pass Summit"]);
    assert_eq!(for_each_waypoint(&src_p, |_| panic!("plain route has no waypoints")).unwrap(), 0);

    // The wire facts a routeList entry serves (S0 §7.4) agree with the manifest and the full index.
    let info = RouteObjectInfo::read(&src_w).unwrap();
    assert_eq!(info.name.as_str(), "Vector Loop");
    assert_eq!(info.distance_m, idx_w.total_distance_m);
    assert_eq!(info.ascent_m, idx_w.total_ascent_m);
    assert_eq!(info.point_count, idx_w.point_count);
    assert_eq!(info.waypoint_count, 2);
    assert_eq!(RouteObjectInfo::read(&src_p).unwrap().waypoint_count, 0);
}

/// The recorded-track pair (A2, #896). `track-log.obct` is a flat 20-byte-record array with a
/// deliberate partial tail, and `track-export.gpx` is what the production exporter writes from it
/// — the pair the browser conversion bridge must reproduce byte-for-byte in wasm. Pinned here
/// from the other side: the log decodes through the production record codec back to the fields
/// the builder wrote, and the export re-derives from the checked-in log (not from the builder's
/// own in-memory copy), so a drift in either file fails.
#[test]
fn track_vectors_pin_the_log_and_its_export() {
    let log = fixture("track-log.obct");
    let gpx = String::from_utf8(fixture("track-export.gpx")).expect("the export is UTF-8");

    // Length is self-describing: whole records plus the truncated tail a power-loss leaves.
    assert_eq!(log.len() % TRACK_RECORD_LEN, 7, "the fixture keeps a partial trailing record");
    let whole = log.len() / TRACK_RECORD_LEN;
    assert_eq!(whole, 5);

    // The hand-built records decode through the production codec to the documented spread:
    // sensor presence walks all-present → one-absent → all-absent → power-only → all-zero.
    let point = |k: usize| {
        let mut rec = [0u8; TRACK_RECORD_LEN];
        rec.copy_from_slice(&log[k * TRACK_RECORD_LEN..(k + 1) * TRACK_RECORD_LEN]);
        obc_formats::track::decode_record(&rec)
    };
    let sensors: Vec<_> = (0..whole).map(|k| (point(k).hr, point(k).cadence, point(k).power)).collect();
    assert_eq!(
        sensors,
        vec![
            (Some(132), Some(78), Some(185)),
            (Some(138), None, Some(190)),
            (None, None, None),
            (None, None, Some(240)),
            (Some(0), Some(0), Some(0)), // zero is a value, not the absent sentinel
        ]
    );
    assert_eq!((point(0).segment_start, point(3).segment_start), (true, true), "two segments");
    assert_eq!((point(3).lon, point(3).lat, point(3).ele), (-122_419_400, -37_774_900, -12), "negative signs");

    // The export re-derives from the checked-in log: same bytes, so the .obct and the .gpx cannot
    // drift apart independently.
    let mut sink = VecSink::default();
    track_to_gpx(&SliceSource(&log), TRACK_NAME, &mut sink).unwrap();
    assert_eq!(String::from_utf8(sink.buf).unwrap(), gpx, "track-export.gpx drifted from track-log.obct");

    // The shapes the exporter's branches produce, spelled out once (the browser bridge reproduces
    // this exact text, so a change here is a change to a cross-language contract).
    assert_eq!(gpx.matches("<trkseg>").count(), 2, "the pause opens a second segment");
    assert_eq!(gpx.matches("<trkpt").count(), whole, "the partial trailing record is ignored");
    assert!(gpx.contains("<trk><name>Schauinsland &amp; back</name>"), "the name is XML-escaped");
    assert!(gpx.contains("lat=\"-37.774900\" lon=\"-122.419400\"><ele>-12</ele>"), "negative fixed-6 degrees");
    assert!(gpx.contains("lat=\"0.000000\" lon=\"0.000000\""), "zero keeps all six decimals");
    assert!(
        gpx.contains("<extensions><power>240</power></extensions>"),
        "power alone skips the TrackPointExtension wrapper"
    );
    assert!(
        gpx.contains("<gpxtpx:hr>138</gpxtpx:hr></gpxtpx:TrackPointExtension><power>190</power>"),
        "an absent cadence drops only its element"
    );
    assert!(!gpx.contains("<time>"), "no fabricated timestamps");
}

/// The 12-byte upload descriptor announces the waypoint route's actual size and CRC, and the
/// download-announce (status msg 4) carries the same 12-byte descriptor — the fixtures form one
/// coherent transfer transcript.
#[test]
fn upload_transcript_is_self_consistent() {
    let route = fixture("route-waypoints.obcr");
    let start = fixture("transfer-upload-start.bin");
    let announce = fixture("status-download-announce.bin");
    let result = fixture("status-transfer-result.bin");

    assert_eq!(start.len(), 12, "v2 descriptor is 12 bytes (offset dropped)");
    assert_eq!(start[0], 1, "op = upload");
    assert_eq!(start[1], 1, "type = route");
    assert_eq!(u16::from_le_bytes([start[2], start[3]]), 0xFFFF, "id = new");
    assert_eq!(u32::from_le_bytes([start[4], start[5], start[6], start[7]]) as usize, route.len());
    assert_eq!(u32::from_le_bytes([start[8], start[9], start[10], start[11]]), crc32(&route));

    // The download announce: msg 4 + the 12-byte descriptor (op = download), same size + CRC.
    assert_eq!(announce.len(), 13, "msg byte + 12-byte descriptor");
    assert_eq!(announce[0], 4, "status msg = downloadAnnounce");
    assert_eq!(announce[1], 2, "op = download");
    assert_eq!(announce[2], 1, "type = route");
    assert_eq!(u32::from_le_bytes([announce[5], announce[6], announce[7], announce[8]]) as usize, route.len());
    assert_eq!(u32::from_le_bytes([announce[9], announce[10], announce[11], announce[12]]), crc32(&route));

    // The closing result: committed (0), every byte durable.
    assert_eq!(result.len(), 8);
    assert_eq!(result[0], 1, "status msg = transferResult");
    assert_eq!(result[3], 0, "committed");
    assert_eq!(u32::from_le_bytes([result[4], result[5], result[6], result[7]]) as usize, route.len());
}

/// Each ride object's length is fully determined by its header + version (spec §7.2).
#[test]
fn ride_vector_length_is_self_describing() {
    let v1 = ride_v1();
    assert_eq!(fixture("ride-v1.bin"), v1);
    let name_len = u16::from_le_bytes([v1[1], v1[2]]) as usize;
    let count_off = 19 + name_len; // version + name_len + name + the five stat fields
    let point_count = u32::from_le_bytes(v1[count_off..count_off + 4].try_into().unwrap());
    // v1 header is 23 bytes + name; each point 14.
    assert_eq!(v1.len(), 23 + name_len + 14 * point_count as usize);

    let v2 = ride_v2();
    assert_eq!(fixture("ride-v2.bin"), v2);
    let name_len = u16::from_le_bytes([v2[1], v2[2]]) as usize;
    let point_count = u32::from_le_bytes(v2[19 + name_len..23 + name_len].try_into().unwrap());
    // v2 header is 31 bytes + name; each point 18.
    assert_eq!(v2.len(), 31 + name_len + 18 * point_count as usize);
}

/// Both ride vectors read through the production header reader (`obc_route::RideInfo`) with the
/// manifest's values, and the production layout agrees byte-for-byte with the hand-built fixtures.
/// The v1 fixture pins the legacy decode (all sensor fields absent); v2 pins the sensor summary +
/// the version-keyed length.
#[test]
fn ride_vector_reads_through_the_production_codec() {
    let v1 = fixture("ride-v1.bin");
    let info = obc_route::RideInfo::read(&SliceSource(&v1)).unwrap();
    assert_eq!(info.version, 1);
    assert_eq!(info.name.as_str(), "Höhenweg");
    assert_eq!(info.start_time, 1_751_450_000);
    assert_eq!(info.distance_m, 42_500);
    assert_eq!(info.moving_time_s, 9_000);
    assert_eq!(info.avg_speed_cms, 472);
    assert_eq!(info.climb_m, 810);
    assert_eq!(info.point_count, 3);
    assert_eq!(
        (info.avg_hr, info.max_hr, info.avg_cadence, info.avg_power, info.max_power),
        (None, None, None, None, None),
        "a v1 object has no sensor summary"
    );
    assert_eq!(v1.len() as u32, obc_formats::ride::object_len(info.version, info.name.len(), info.point_count));

    let v2 = fixture("ride-v2.bin");
    let info = obc_route::RideInfo::read(&SliceSource(&v2)).unwrap();
    assert_eq!(info.version, 2);
    assert_eq!(info.name.as_str(), "Sensor Ride");
    assert_eq!(info.start_time, 1_751_460_000);
    assert_eq!(info.distance_m, 12_345);
    assert_eq!(info.moving_time_s, 3_600);
    assert_eq!(info.avg_speed_cms, 343);
    assert_eq!(info.climb_m, 120);
    assert_eq!(info.point_count, 3);
    assert_eq!(
        (info.avg_hr, info.max_hr, info.avg_cadence, info.avg_power, info.max_power),
        (Some(142), Some(176), Some(85), Some(210), Some(480)),
        "v2 carries the per-ride sensor summary"
    );
    assert_eq!(v2.len() as u32, obc_formats::ride::object_len(info.version, info.name.len(), info.point_count));

    // The elevation profile reader streams the v2 object's points (p2's ele sentinel is skipped).
    let p = obc_route::ride_elevation_profile(&SliceSource(&v2)).unwrap();
    assert_eq!((p.min_ele_m, p.max_ele_m), (214, 219), "the ele-sentinel point contributes no sample");
}

/// The trip vectors pin §7.7 (the trip object) and the §7.4 `tripList` addition, and tie together:
/// the trip references two route ids that `route-list.bin` actually holds (7, 8) plus one
/// deliberately dangling id (99), and the `tripList` totals sum only the resolvable stages while its
/// `stage_count` counts every stored stage (dangling included).
#[test]
fn trip_vectors_are_self_consistent() {
    let trip = fixture("trip-v1.bin");
    // Header: version 1, reserved 0, stage_count 3, name "Alpen Traverse".
    assert_eq!(trip[0], 1, "trip object version");
    assert_eq!(trip[1], 0, "reserved");
    let stage_count = u16::from_le_bytes([trip[2], trip[3]]);
    assert_eq!(stage_count, 3);
    let name_len = trip[4] as usize;
    assert_eq!(&trip[5..5 + name_len], TRIP_NAME.as_bytes());
    // Length is self-describing: 56-byte header + 2 bytes/stage.
    assert_eq!(trip.len(), 56 + 2 * stage_count as usize);
    let stages: Vec<u16> =
        (0..stage_count as usize).map(|k| u16::from_le_bytes([trip[56 + 2 * k], trip[56 + 2 * k + 1]])).collect();
    assert_eq!(stages, vec![TRIP_STAGE_IDS[0], TRIP_STAGE_IDS[1], TRIP_DANGLING_STAGE]);

    // The two resolvable stages are exactly the ids route-list.bin enumerates; the third dangles.
    // Decode (object_id, distance_m, ascent_m) from each route-list entry so the expected tripList
    // totals below are DERIVED by the spec's summation rule (a stage resolves iff a stored route
    // holds its id; a dangling ref contributes nothing) — not restated as literals.
    let rl = fixture("route-list.bin");
    let (rl_count, rl_entry_len) = (u16::from_le_bytes([rl[2], rl[3]]) as usize, rl[1] as usize);
    let routes: Vec<(u16, u32, u32)> = (0..rl_count)
        .map(|k| {
            let b = 6 + rl_entry_len * k;
            (
                u16::from_le_bytes([rl[b], rl[b + 1]]),
                u32::from_le_bytes(rl[b + 8..b + 12].try_into().unwrap()), // distance_m
                u32::from_le_bytes(rl[b + 12..b + 16].try_into().unwrap()), // ascent_m
            )
        })
        .collect();
    let held: Vec<u16> = routes.iter().map(|&(id, ..)| id).collect();
    assert!(TRIP_STAGE_IDS.iter().all(|id| held.contains(id)), "both resolvable stages are stored routes");
    assert!(!held.contains(&TRIP_DANGLING_STAGE), "the third stage is deliberately dangling");
    let resolved = || stages.iter().filter_map(|s| routes.iter().find(|&&(id, ..)| id == *s));
    let want_distance: u32 = resolved().map(|&(_, d, _)| d).sum();
    let want_ascent: u32 = resolved().map(|&(_, _, a)| a).sum();
    assert_eq!(resolved().count(), 2, "exactly the two resolvable stages contribute to the totals");

    // tripList: 6-byte v2 header, one 76-byte entry, total == count == 1.
    let tl = fixture("trip-list.bin");
    assert_eq!(tl[0], 2, "list version");
    assert_eq!(tl[1], 76, "tripList entry_len (mirrors routeList)");
    assert_eq!(u16::from_le_bytes([tl[2], tl[3]]), 1, "count");
    assert_eq!(u16::from_le_bytes([tl[4], tl[5]]), 1, "total == count (nothing dropped)");
    let e = &tl[6..];
    assert_eq!(e.len(), 76, "one 76-byte entry");
    assert_eq!(u16::from_le_bytes([e[0], e[1]]), TRIP_ID, "trip id (its own counter)");
    assert_eq!(u32::from_le_bytes([e[4], e[5], e[6], e[7]]) as usize, trip.len(), "byte_len = stored trip file");
    assert_eq!(u32::from_le_bytes([e[8], e[9], e[10], e[11]]), want_distance, "distance summed over resolvable stages");
    assert_eq!(u32::from_le_bytes([e[12], e[13], e[14], e[15]]), want_ascent, "ascent summed over resolvable stages");
    assert_eq!(u16::from_le_bytes([e[16], e[17]]), stage_count, "stage_count as stored (incl. dangling)");
    let name_len = e[20] as usize;
    assert_eq!(&e[21..21 + name_len], TRIP_NAME.as_bytes());
    // Trailing whole-object crc32 = the trip file's CRC-32 (the content fingerprint routes use).
    assert_eq!(u32::from_le_bytes([e[72], e[73], e[74], e[75]]), crc32(&trip), "entry crc32 fingerprints the trip");
}

/// The OBCT terrain shard (`OBCT_Spec.md`): the checked-in bytes parse through the production
/// `obc-elevation` reader, and the three sampling rules that a second implementation is most likely
/// to get wrong — the cross-cell fetch, the coverage clamp and `NODATA` propagation — produce the
/// numbers the spec's worked examples state.
///
/// The interpolated values are asserted against the **closed form of the plane**
/// (`100 + 3·di + 5·dj`, rounded half away from zero), not against a table copied out of the
/// reader: on a plane the two must agree exactly, so this is an oracle rather than a mirror.
#[test]
fn terrain_vector_samples_through_the_production_reader() {
    let bytes = fixture("terrain-shard.obcd");
    assert_eq!(bytes, terrain_shard(), "the fixture drifted from the spec builder");
    assert_eq!(bytes.len(), 32 + 16 + 3 * 2048, "header + 2×2 directory + three cell blocks");

    let src = SliceSource(&bytes);
    let reader = TerrainReader::parse(&src).expect("the hand-built container parses");
    let header = reader.header();
    assert_eq!((header.posting_log2, header.cell_log2), (TERRAIN_POSTING_LOG2, TERRAIN_CELL_LOG2));
    assert_eq!((header.cell_rows, header.cell_cols), (TERRAIN_ROWS, TERRAIN_COLS));
    let mut cache = TileCache::<4>::new();

    // µdeg helpers over the fixture's lattice offsets.
    let lat = |di: u32| terrain_coord(TERRAIN_CELL_MIN_I, di);
    let lon = |dj: u32| terrain_coord(TERRAIN_CELL_MIN_J, dj);
    // The plane's closed form at a sub-posting offset, rounded half away from zero (spec §5.2).
    let plane = |di: f64, dj: f64| {
        let h = 100.0 + 3.0 * di + 5.0 * dj;
        (h.abs() + 0.5).floor().copysign(h) as i16
    };

    // The literal µdeg coordinates `manifest.json` publishes, so the two cannot drift apart.
    assert_eq!((lat(0), lon(0)), (46_972_928, 7_979_008), "the rectangle's base sample");
    assert_eq!((lat(2) + 256, lon(3) + 128), (46_974_208, 7_980_672));
    assert_eq!((lat(31) + 256, lon(3)), (46_989_056, 7_980_544));
    assert_eq!((lat(2), lon(63) + 256), (46_973_952, 8_011_520));

    // Worked example 1 (spec §5.6): a quarter/half-posting offset inside one tile.
    assert_eq!(reader.sample(&mut cache, lat(2) + 256, lon(3) + 128), Some(124));
    assert_eq!(plane(2.5, 3.25), 124);

    // Worked example 2: half a posting below the cell seam in latitude — the upper corners come out
    // of the *next cell down the directory*, and the plane stays a plane across it.
    assert_eq!(reader.sample(&mut cache, lat(31) + 256, lon(3)), Some(210));
    assert_eq!(plane(31.5, 3.0), 210);

    // Worked example 3: half a posting past the rectangle's east edge — the missing corner clamps
    // to the last covered sample, so the surface flattens instead of extrapolating.
    assert_eq!(reader.sample(&mut cache, lat(2), lon(63)), Some(421));
    assert_eq!(reader.sample(&mut cache, lat(2), lon(63) + 256), Some(421), "clamped, not 424");

    // Lattice points return their own sample, in every tile of every present cell.
    for (di, dj) in [(0u32, 0u32), (15, 15), (16, 16), (31, 31), (32, 0), (63, 17), (0, 63)] {
        assert_eq!(reader.sample(&mut cache, lat(di), lon(dj)), Some(terrain_height(di, dj)), "({di}, {dj})");
    }

    // The hole: every query inside the absent cell is uncovered.
    assert_eq!(reader.sample(&mut cache, lat(40) + 100, lon(40) + 100), None, "the absent cell");
    // The void: any query whose corner set touches the NODATA sample is None, and its neighbour two
    // postings away is untouched.
    let (vi, vj) = TERRAIN_NODATA_AT;
    assert_eq!(reader.sample(&mut cache, lat(vi), lon(vj)), None);
    assert_eq!(reader.sample(&mut cache, lat(vi - 1) + 1, lon(vj - 1) + 1), None, "no partial interpolation");
    assert_eq!(reader.sample(&mut cache, lat(vi + 2), lon(vj + 2)), Some(terrain_height(vi + 2, vj + 2)));
}

/// The three `weatherRequestContext` reads (spec §11, WX3 #1188). `fixtures_match_the_spec_builders`
/// already pins the bytes; what this adds is the *reading* of them — the field offsets decoded by
/// hand, and the two claims the layout exists to make.
///
/// The first is that the bundle group names a real bundle: `weather-request-context-full.bin`'s
/// generation, time and CRC are checked against `weather-dwd-96x96-9f.obcw` itself, so the fixture
/// pair is a device holding *that* forecast and asking for its successor, not three integers of the
/// right width. The second is that absence is a cleared flag: the no-fix file's coordinates are
/// zero, and a decoder that reads them anyway has put a rider on the equator.
#[test]
fn weather_request_context_vectors_describe_a_real_request() {
    use obc_vectors::weather_request as wx;

    let rd_u16 = |b: &[u8], off: usize| u16::from_le_bytes(b[off..off + 2].try_into().unwrap());
    let rd_u32 = |b: &[u8], off: usize| u32::from_le_bytes(b[off..off + 4].try_into().unwrap());
    let rd_i64 = |b: &[u8], off: usize| i64::from_le_bytes(b[off..off + 8].try_into().unwrap());

    let full = fixture("weather-request-context-full.bin");
    let empty = fixture("weather-request-context-empty.bin");
    let no_fix = fixture("weather-request-context-no-fix.bin");

    // Every context is a self-declaring 52-byte v1 value with both reserved fields zero. The
    // declared length is what a reader trusts before it indexes anything, so it is checked first.
    for (name, bytes) in [("full", &full), ("empty", &empty), ("no-fix", &no_fix)] {
        assert_eq!(bytes.len(), wx::CONTEXT_ENCODED_LEN, "{name}");
        assert_eq!(bytes[0], wx::CONTEXT_VERSION, "{name} version");
        assert_eq!(bytes[1] as usize, wx::CONTEXT_ENCODED_LEN, "{name} declares its own length");
        assert_eq!(bytes[7], 0, "{name} reserved0");
        assert_eq!(rd_u16(bytes, 34), 0, "{name} reserved1");
    }

    // The full read: everything valid, scheduled, on the 30-minute interval.
    assert_eq!(
        rd_u16(&full, 2),
        wx::VALID_POSITION | wx::VALID_BEARING | wx::VALID_SPEED | wx::VALID_BUNDLE | wx::VALID_ROUTE,
        "every optional group is present — no field is left where a wrong offset could hide"
    );
    assert_eq!(rd_u16(&full, 4), wx::REASON_SCHEDULED);
    assert_eq!(full[6], wx::REFRESH_EVERY_30);
    assert_eq!(rd_u32(&full, 8), wx::FULL_REQUEST_ID);
    assert_eq!(rd_u32(&full, 12) as i32, wx::FULL_LAT_UDEG, "lat at 12");
    assert_eq!(rd_u32(&full, 16) as i32, wx::FULL_LON_UDEG, "lon at 16 — a transposed pair lands in Somalia");
    assert_eq!(rd_u16(&full, 28), wx::FULL_BEARING_DEG);
    assert_eq!(rd_u16(&full, 30), wx::FULL_SPEED_DECI_MS);
    assert_eq!(rd_u16(&full, 32), wx::FULL_ROUTE_ID, "the active route is one route-list.bin catalogs");

    // The bundle group identifies the checked-in OBCW fixture — read back out of that file rather
    // than restated here, so the two cannot drift into describing different forecasts.
    let held = fixture(wx::HELD_BUNDLE_FIXTURE);
    let generation = rd_u32(&held, obc_formats::obcw::HDR_GENERATION);
    let generated_at = rd_i64(&held, obc_formats::obcw::HDR_GENERATED_AT);
    assert_eq!(rd_u32(&full, 36), generation, "bundle_generation names the held bundle");
    assert_eq!(rd_i64(&full, 40), generated_at, "…as does bundle_generated_at");
    assert_eq!(rd_u32(&full, 48), crc32(&held), "…and the whole-object CRC-32 the upload announced");
    // The scheduled reason is arithmetic, not decoration: the fix is exactly one refresh interval
    // after the bundle it would replace.
    assert_eq!(rd_i64(&full, 20), generated_at + 30 * 60, "fix_utc is 30 minutes past the held bundle");

    // The resting value: nothing claimed, but still a v1 value with the default interval stated.
    assert_eq!(rd_u16(&empty, 2), 0, "no validity");
    assert_eq!(rd_u16(&empty, 4), 0, "no reason");
    assert_eq!(empty[6], wx::REFRESH_EVERY_30, "the device default, not a zeroed byte");
    assert!(empty[8..].iter().all(|&b| b == 0), "everything past the refresh byte is zero");

    // The urgent no-fix request: reasons but no data, and — the trap — a *scheduled* refresh of Off
    // that must not read as "this device raises no requests".
    assert_eq!(rd_u16(&no_fix, 2), 0, "no group is valid");
    assert_eq!(rd_u16(&no_fix, 4), wx::REASON_URGENT | wx::REASON_NO_BUNDLE);
    assert_eq!(no_fix[6], wx::REFRESH_OFF, "Off configures the schedule, not the right to ask");
    assert_eq!(rd_u32(&no_fix, 8), wx::NO_FIX_REQUEST_ID);
    assert_ne!(wx::NO_FIX_REQUEST_ID, wx::FULL_REQUEST_ID, "two files, two requests");
    assert_eq!((rd_u32(&no_fix, 12), rd_u32(&no_fix, 16), rd_i64(&no_fix, 20)), (0, 0, 0));
    assert_eq!(rd_u32(&no_fix, 36), 0, "no bundle held — absence is the cleared flag, not generation 0");
}

/// The **forward-compatibility** context (#1214): the day a firmware appends a fifth refresh
/// interval, this is the byte every phone already in the field receives.
///
/// The file is `weather-request-context-full.bin` at every offset but two, and that is the assertion
/// — the unknown byte is checked to change *only* the schedule the phone can name, not the request,
/// the position, or the bundle identity. An implementation that treated the value as malformed would
/// have to discard all of it, which is precisely the trap: §11.8 makes an unrecognised interval fatal
/// in the *write* direction only, and a read that refused it would turn an ordinary enum append into
/// the day weather went dead on every shipped app.
#[test]
fn an_unknown_refresh_byte_costs_a_context_read_nothing_else() {
    use obc_vectors::weather_request as wx;

    let full = fixture("weather-request-context-full.bin");
    let unknown = fixture("weather-request-context-unknown-refresh.bin");

    assert_eq!(unknown.len(), wx::CONTEXT_ENCODED_LEN, "still a well-formed 52-byte v1 value");
    assert_eq!(unknown[0], wx::CONTEXT_VERSION, "…of the version this build knows");
    assert_eq!(unknown[1] as usize, wx::CONTEXT_ENCODED_LEN, "…declaring its own length");
    assert_eq!(unknown[6], wx::REFRESH_UNKNOWN, "the byte v1 never defined");
    assert!(unknown[6] > 4, "…and it is outside the enum, not merely an unusual member of it");
    assert_eq!(u32::from_le_bytes(unknown[8..12].try_into().unwrap()), wx::UNKNOWN_REFRESH_REQUEST_ID);

    // Byte-for-byte identical to the full context except the refresh byte and the nonce. Nothing
    // else about the request changes because the phone cannot name the interval.
    let differ: Vec<usize> = (0..wx::CONTEXT_ENCODED_LEN).filter(|&i| full[i] != unknown[i]).collect();
    assert!(differ.contains(&6), "the refresh byte is the difference this file exists for");
    assert!(
        differ.iter().all(|&i| i == 6 || (8..12).contains(&i)),
        "only the refresh byte and the request-id nonce may differ, not {differ:?}"
    );
    assert_eq!(&full[12..], &unknown[12..], "position, fix, bearing, route and bundle identity all survive");
}

/// Sign coverage (#1214): the four signed fields of the context, and the two unsigned ones most
/// likely to be read signed by a mirror.
///
/// Nothing else in the fixture set carries a negative coordinate or a pre-1970 time, so before this
/// file a decoder could read `lat_udeg`/`lon_udeg` as `u32` and both timestamps as `u64` and pass the
/// whole suite. Each assertion below is a value that a wrong-signedness read gets *visibly* wrong —
/// a latitude of 4245°, a clock 585 billion years ahead — rather than subtly.
#[test]
fn the_southern_context_pins_every_signed_field() {
    use obc_vectors::weather_request as wx;

    let rd_u16 = |b: &[u8], off: usize| u16::from_le_bytes(b[off..off + 2].try_into().unwrap());
    let rd_u32 = |b: &[u8], off: usize| u32::from_le_bytes(b[off..off + 4].try_into().unwrap());
    let rd_i64 = |b: &[u8], off: usize| i64::from_le_bytes(b[off..off + 8].try_into().unwrap());

    let southern = fixture("weather-request-context-southern.bin");
    assert_eq!(southern.len(), wx::CONTEXT_ENCODED_LEN);
    assert_eq!(southern[0], wx::CONTEXT_VERSION);
    assert_eq!(southern[1] as usize, wx::CONTEXT_ENCODED_LEN);
    assert_eq!(southern[7], 0, "reserved0");
    assert_eq!(rd_u16(&southern, 34), 0, "reserved1");

    assert_eq!(
        rd_u16(&southern, 2),
        wx::VALID_POSITION | wx::VALID_BEARING | wx::VALID_SPEED | wx::VALID_BUNDLE,
        "everything but the route — a cleared bit beside the four groups this file exercises"
    );
    assert_eq!(rd_u16(&southern, 4), wx::REASON_SCHEDULED);
    assert_eq!(southern[6], wx::REFRESH_EVERY_60);
    assert_eq!(rd_u32(&southern, 8), wx::SOUTHERN_REQUEST_ID);
    assert_ne!(wx::SOUTHERN_REQUEST_ID, wx::FULL_REQUEST_ID, "a fourth file, a fourth request");

    // The two i32 coordinates. Read unsigned these are 4 245 636 407 and 4 222 081 175 — impossible
    // rather than merely wrong, which is what a sign fixture is for.
    let (lat, lon) = (rd_u32(&southern, 12) as i32, rd_u32(&southern, 16) as i32);
    assert_eq!((lat, lon), (wx::SOUTHERN_LAT_UDEG, wx::SOUTHERN_LON_UDEG), "Patagonia — south *and* west");
    assert!(lat < 0 && lon < 0, "both hemispheres are the point");

    // The two i64 timestamps, both before the epoch, and each at its own offset so one correct
    // sign extension cannot cover for the other.
    // Read out of the *file*, not off the builder's constants: asserting a constant is negative
    // says nothing about the bytes on disk, which are the only thing a mirror will ever see.
    let fix_utc = rd_i64(&southern, 20);
    let bundle_generated_at = rd_i64(&southern, 40);
    assert_eq!(fix_utc, wx::SOUTHERN_FIX_UTC, "fix_utc at 20");
    assert_eq!(bundle_generated_at, wx::SOUTHERN_BUNDLE_GENERATED_AT, "bundle_generated_at at 40");
    assert!(fix_utc < 0, "fix_utc is pre-1970");
    assert!(bundle_generated_at < 0, "bundle_generated_at is pre-1970");
    assert_eq!(
        fix_utc - bundle_generated_at,
        60 * 60,
        "the scheduled reason is arithmetic here too — one 60-minute interval past what it holds"
    );

    assert_eq!(rd_u16(&southern, 28), wx::SOUTHERN_BEARING_DEG);
    assert_eq!(rd_u16(&southern, 30), wx::SOUTHERN_SPEED_DECI_MS);

    // …and the trap run the other way: two u32s with the top bit set, which a *signed* mirror reads
    // as -2 and -2147483647.
    let bundle_generation = rd_u32(&southern, 36);
    let bundle_crc32 = rd_u32(&southern, 48);
    assert_eq!(bundle_generation, wx::SOUTHERN_BUNDLE_GENERATION);
    assert_eq!(bundle_crc32, wx::SOUTHERN_BUNDLE_CRC32);
    assert!(bundle_generation > i32::MAX as u32, "generation is unsigned, and this file states it");
    assert!(bundle_crc32 > i32::MAX as u32, "crc32 is unsigned, and this file states it");
}

/// The two append-only extensions the Weather Request contract rode in on (#1188): the identity
/// read's capability word and Config's trailing refresh byte. Both are pinned as *pairs* with the
/// fixtures that predate them, because in both cases the only way to get it wrong is an offset, and
/// an offset bug reads the shorter file correctly.
#[test]
fn the_weather_capability_and_config_refresh_are_appends() {
    use obc_vectors::weather_request as wx;

    let features = fixture("version-read-features.bin");
    let pre_wx3 = fixture("version-read.bin");
    assert_eq!(features.len(), 11, "version u16 · store_epoch u32 · obcm_version u8 · feature_bits u32");
    assert_eq!(pre_wx3.len(), 7, "the read a device without weather still serves");
    assert_eq!(u16::from_le_bytes([features[0], features[1]]), 2, "no protocol bump — the word is an append");
    assert_eq!(u32::from_le_bytes(features[2..6].try_into().unwrap()), wx::FEATURES_STORE_EPOCH);
    assert_eq!(
        features[6],
        obc_formats::obcm::VERSION,
        "self-sourced like every other obcm_version byte — an OBCM bump re-cuts this file too"
    );
    assert_eq!(u32::from_le_bytes(features[7..11].try_into().unwrap()), wx::FEATURE_WEATHER);
    // The first seven bytes are the older read's layout unchanged — same version, same map version,
    // only the epoch nonce differs. That is the whole claim an "append" makes.
    assert_eq!(&features[..2], &pre_wx3[..2], "version field is untouched");
    assert_eq!(features[6], pre_wx3[6], "obcm_version field is untouched");
    assert_ne!(&features[2..6], &pre_wx3[2..6], "…and the epoch differs, so the files are not confusable");

    let config = fixture("config-weather-refresh.bin");
    let name_len = u16::from_le_bytes([config[0], config[1]]) as usize;
    assert_eq!(&config[2..2 + name_len], wx::CONFIG_NAME.as_bytes());
    assert_eq!(config[2 + name_len], wx::CONFIG_UNITS, "units keep their v1 offset");
    assert_eq!(config[3 + name_len], wx::REFRESH_EVERY_60, "…and the refresh byte follows them");
    assert_eq!(config.len(), 2 + name_len + 2, "name_len u16 · name · units u8 · weather_refresh u8");
    // The v1 blob beside it is the same object without that last byte, which is what makes an
    // absent refresh distinguishable from `Off` rather than a matter of taste.
    let v1 = fixture("config-v1.bin");
    let v1_name_len = u16::from_le_bytes([v1[0], v1[1]]) as usize;
    assert_eq!(v1.len(), 2 + v1_name_len + 1, "no trailing byte at all");
}

/// The Config blob whose refresh byte names an interval nobody defines (#1214) — the same wire value
/// §11.8 says must be **refused as a write** and **tolerated as a read**.
///
/// The bytes here only have to be well-formed *apart from* that field: the direction rule lives in
/// the codecs (`obc-ble`'s `refresh_to_apply` vs `known_refresh`, and the TS mirror's helper), and
/// this file is the single input both are held to. What it pins on its own is that the blob really
/// is otherwise ordinary — same three-field layout, a name of a third distinct length, and a
/// trailing byte that a reader either finds where the spec says it is or gets wrong twice over.
#[test]
fn the_unknown_config_refresh_is_an_otherwise_ordinary_blob() {
    use obc_vectors::weather_request as wx;

    let bytes = fixture("config-weather-refresh-unknown.bin");
    let name_len = u16::from_le_bytes([bytes[0], bytes[1]]) as usize;
    assert_eq!(&bytes[2..2 + name_len], wx::CONFIG_UNKNOWN_NAME.as_bytes());
    assert_eq!(bytes[2 + name_len], wx::CONFIG_UNKNOWN_UNITS, "metric");
    assert_eq!(bytes[3 + name_len], wx::CONFIG_UNKNOWN_REFRESH, "the byte no version of the enum defines");
    assert!(bytes[3 + name_len] > 4, "…outside `0..=4`, so a later append cannot quietly retire this file");
    assert_eq!(bytes.len(), 2 + name_len + 2, "name_len u16 · name · units u8 · weather_refresh u8");

    // Three Config fixtures, three name lengths: the trailing byte sits at a different offset in
    // each, so no reader passes all three with one hard-coded index.
    let sibling = fixture("config-weather-refresh.bin");
    let v1 = fixture("config-v1.bin");
    assert_ne!(name_len, u16::from_le_bytes([sibling[0], sibling[1]]) as usize, "vs config-weather-refresh");
    assert_ne!(name_len, u16::from_le_bytes([v1[0], v1[1]]) as usize, "vs config-v1");
    // …and this one's `units` is zero on purpose: an off-by-one reader lands on it and decodes a
    // *known* `Off`, so the misalignment surfaces as a wrong answer rather than as another
    // "unknown" the tolerant read direction would have accepted regardless.
    assert_eq!(wx::CONFIG_UNKNOWN_UNITS, wx::REFRESH_OFF, "the off-by-one trap this fixture sets");
    assert_ne!(wx::CONFIG_UNKNOWN_REFRESH, wx::REFRESH_UNKNOWN, "a distinct unknown from the context file's");
}

/// Rewrite every fixture from the builders. Run only after a deliberate spec change:
/// `cargo test -p obc-vectors regenerate -- --ignored` — then hand the diff to the
/// app side (its Swift tests pin the same files).
#[test]
#[ignore]
fn regenerate() {
    std::fs::create_dir_all(dir()).unwrap();
    for (name, bytes) in all() {
        std::fs::write(dir().join(name), bytes).unwrap();
    }
}
