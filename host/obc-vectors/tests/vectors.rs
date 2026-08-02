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

/// The **volume-set** descriptors (§4.1, #1039): the two new type bytes, and the one field on this
/// wire whose meaning is not an object id.
///
/// `mapShard`'s `object_id` is a packed `(shard_count, index)` — count in the **high** byte — and
/// that packing is exactly the kind of thing three implementations derive from prose and get
/// mirrored. A fixture is cheaper than the bug: a host that reads it the other way round announces
/// "shard 8 of 2" and is answered `notFound` with nothing to point at.
#[test]
fn volume_set_descriptors_pin_the_part_packing() {
    let route = fixture("route-waypoints.obcr");
    let shard = fixture("transfer-set-shard.bin");
    let manifest = fixture("transfer-set-manifest.bin");

    assert_eq!(shard.len(), 12, "no descriptor change — a set rides the same 12 bytes");
    assert_eq!(shard[0], 1, "op = upload");
    assert_eq!(shard[1], 17, "type = mapShard");
    let part = u16::from_le_bytes([shard[2], shard[3]]);
    assert_eq!(part, 0x0802);
    assert_eq!(part >> 8, 8, "the high byte is the shard count");
    assert_eq!(part & 0xFF, 2, "and the low byte is this shard's index");
    assert_eq!(u32::from_le_bytes([shard[4], shard[5], shard[6], shard[7]]) as usize, route.len());
    assert_eq!(u32::from_le_bytes([shard[8], shard[9], shard[10], shard[11]]), crc32(&route));

    assert_eq!(manifest.len(), 12);
    assert_eq!(manifest[0], 1, "op = upload");
    assert_eq!(manifest[1], 18, "type = mapSet");
    assert_eq!(u16::from_le_bytes([manifest[2], manifest[3]]), 0xFFFF, "the manifest is new-only");
    let total_len = u32::from_le_bytes([manifest[4], manifest[5], manifest[6], manifest[7]]);
    assert_eq!(total_len, 72 + 56 * 8, "a manifest's length is fixed by its shard count (OBCA §5.2)");
    assert_eq!(total_len, obc_formats::obcs::manifest_len(8) as u32, "…as the format authority computes it");
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
