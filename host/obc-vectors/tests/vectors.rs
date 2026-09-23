//! Contract tests over the checked-in `specs/vectors/` fixtures: every file must
//! equal its spec-derived builder byte-for-byte, and the route vectors must load and
//! ride through `obc-route`. The app's `swift test` consumes the same files.

use obc_elevation::{TerrainReader, TileCache};
use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_formats::{ride::FOOTER_LEN as RIDE_FOOTER_LEN, track::RECORD_LEN as TRACK_RECORD_LEN};
use obc_route::{
    for_each_waypoint, track_to_gpx, BikeType, RouteIndex, RouteObjectInfo, RouteReader, MAX_POINTS_PER_CHUNK,
};
use obc_vectors::{
    all, crc32, dir, ride_v5, terrain_coord, terrain_height, terrain_shard, TERRAIN_CELL_LOG2, TERRAIN_CELL_MIN_I,
    TERRAIN_CELL_MIN_J, TERRAIN_COLS, TERRAIN_NODATA_AT, TERRAIN_POSTING_LOG2, TERRAIN_ROWS, TRACK_NAME, TRIP_DAYS,
    TRIP_KEY, TRIP_NAME, TRIP_START_DATE,
};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| {
        panic!("fixture {name} unreadable ({e}) — run `cargo run -p obc-vectors --example regenerate --locked`")
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

#[test]
fn landmark_vector_uses_the_production_record_decoder() {
    use obc_formats::obcm::landmarks::{LandmarkRecord, RECORD_LEN};
    let bytes = fixture("landmark-section-v16.bin");
    let encoded: &[u8; RECORD_LEN] = bytes[16..16 + RECORD_LEN].try_into().unwrap();
    let record = LandmarkRecord::decode(encoded).unwrap();
    assert_eq!(record.qid, 123);
    assert_eq!(&bytes[record.articles.offset as usize..][..2], b"de");
    assert_eq!((record.lon, record.lat), (8_000_000, 47_000_000));
    assert!(record.osm.is_none());
    assert!(record.photo.is_absent());
    assert_eq!(record.encode(), *encoded);
    for reference in [record.name, record.articles] {
        assert!(reference.range((16 + RECORD_LEN) as u32, bytes.len() as u32, 65_535).is_some());
    }
}

#[test]
fn place_vector_uses_the_production_metadata_decoder() {
    use obc_formats::obcm::{PoiApproach, PoiMetadata, SourceId};
    let bytes = fixture("place-train-v15.bin");
    assert_eq!(bytes.len(), 64);
    assert_eq!(bytes[8], 20);
    let expected = PoiMetadata {
        source: SourceId::osm(1, 123),
        approach: Some(PoiApproach { source: SourceId::osm(1, 456), lat: 46_561_320, lon: 8_361_490, profile_mask: 5 }),
    };
    assert_eq!(PoiMetadata::decode(&bytes[36..]), Some(expected));
    assert_eq!(expected.encode().as_slice(), &bytes[36..]);
    let mut invalid = bytes[36..].to_vec();
    invalid[27] = 1;
    assert_eq!(PoiMetadata::decode(&invalid), None);
    for (range, value) in [(0..8, 0), (8..16, 0), (24..25, 0)] {
        let mut invalid = bytes[36..].to_vec();
        invalid[range].fill(value);
        assert_eq!(PoiMetadata::decode(&invalid), None);
    }
    let mut invalid = bytes[36..].to_vec();
    invalid[16..20].copy_from_slice(&90_000_001i32.to_le_bytes());
    assert_eq!(PoiMetadata::decode(&invalid), None);
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

    // The metadata used by current flat-store listings agrees with the full index.
    let info = RouteObjectInfo::read(&src_w).unwrap();
    assert_eq!(info.name.as_str(), "Vector Loop");
    assert_eq!(info.distance_m, idx_w.total_distance_m);
    assert_eq!(info.ascent_m, idx_w.total_ascent_m);
    assert_eq!(info.point_count, idx_w.point_count);
    assert_eq!(info.waypoint_count, 2);
    assert_eq!(RouteObjectInfo::read(&src_p).unwrap().waypoint_count, 0);
}

/// Header byte 7 is the bike type: a known value reads back, anything above 3 is rejected.
#[test]
fn route_vector_carries_its_bike_type() {
    let mut bytes = fixture("route-plain.obcr");
    assert_eq!(RouteIndex::read(&SliceSource(&bytes)).unwrap().bike_type(), BikeType::Road);
    bytes[obc_formats::obcr::BIKE_TYPE_OFF] = BikeType::Mtb as u8;
    assert_eq!(RouteIndex::read(&SliceSource(&bytes)).unwrap().bike_type(), BikeType::Mtb);
    bytes[obc_formats::obcr::BIKE_TYPE_OFF] = 4;
    assert!(RouteIndex::read(&SliceSource(&bytes)).is_err());
}

#[test]
fn route_descriptor_envelopes_match_the_shared_overlap_contract() {
    let bytes = fixture("route-visit.obcr");
    let source = SliceSource(&bytes);
    let index = RouteIndex::read(&source).unwrap();
    assert!(RouteReader::new(&index, &source).visit_descriptor().unwrap().is_some());
    for name in ["route-visit-waypoint-overlap.obcr", "route-visit-index-overlap.obcr"] {
        let bytes = fixture(name);
        assert!(RouteIndex::read(&SliceSource(&bytes)).is_err(), "{name} overlaps the descriptor envelope");
    }
}

/// The sample-codec vector and the finished-ride GPX export. `track-log.obct` is exactly five
/// complete 20-byte records used only to pin the sample codec. The exporter consumes
/// `ride-v5.bin`; unfinished/headerless arrays are deliberately not a ride input.
#[test]
fn track_vectors_pin_the_log_and_its_export() {
    let log = fixture("track-log.obct");
    let gpx = String::from_utf8(fixture("track-export.gpx")).expect("the export is UTF-8");

    assert_eq!(log.len() % TRACK_RECORD_LEN, 0);
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

    // The export re-derives from the checked-in finished ride-v5 object.
    let mut sink = VecSink::default();
    track_to_gpx(&SliceSource(&fixture("ride-v5.bin")), TRACK_NAME, &mut sink).unwrap();
    assert_eq!(String::from_utf8(sink.buf).unwrap(), gpx, "track-export.gpx drifted from ride-v5.bin");

    // The shapes the exporter's branches produce, spelled out once (the browser bridge reproduces
    // this exact text, so a change here is a change to a cross-language contract).
    assert_eq!(gpx.matches("<trkseg>").count(), 2, "the pause opens a second segment");
    assert_eq!(gpx.matches("<trkpt").count(), 3);
    assert!(gpx.contains("<trk><name>Schauinsland &amp; back</name>"), "the name is XML-escaped");
    assert!(
        gpx.contains(
            "<gpxtpx:hr>140</gpxtpx:hr><gpxtpx:cad>84</gpxtpx:cad></gpxtpx:TrackPointExtension><power>205</power>"
        ),
        "the first point carries the full sensor extension"
    );
    assert!(
        gpx.contains("<gpxtpx:hr>150</gpxtpx:hr></gpxtpx:TrackPointExtension><power>215</power>"),
        "an absent cadence drops only its element on the final segment"
    );
    assert!(!gpx.contains("<time>"), "no fabricated timestamps");
}

/// A ride-v5 object's length is exactly its verbatim samples plus one fixed footer.
#[test]
fn ride_vector_length_is_self_describing() {
    let ride = ride_v5();
    assert_eq!(fixture("ride-v5.bin"), ride);
    let footer = &ride[ride.len() - RIDE_FOOTER_LEN..];
    assert_eq!(&footer[..5], b"OBRF\x05");
    let point_count = u32::from_le_bytes(footer[26..30].try_into().unwrap());
    assert_eq!(ride.len(), TRACK_RECORD_LEN * point_count as usize + RIDE_FOOTER_LEN);
}

/// The independent vector reads through the production footer and detail codecs.
#[test]
fn ride_vector_reads_through_the_production_codec() {
    let ride = fixture("ride-v5.bin");
    let info = obc_route::RideInfo::read(&SliceSource(&ride)).unwrap();
    assert_eq!(info.version, 5);
    assert_eq!(info.name.as_str(), "Sensor Ride");
    assert_eq!(info.start_time, 1_751_460_000);
    assert_eq!(info.distance_m, 12_345);
    assert_eq!(info.moving_time_s, 3_600);
    assert_eq!(info.avg_speed_cms, 343);
    assert_eq!((info.climb_m, info.descent_m), (120, 95));
    assert_eq!(info.point_count, 3);
    assert_eq!(
        (info.avg_hr, info.max_hr, info.avg_cadence, info.avg_power, info.max_power, info.energy_kj),
        (Some(142), Some(176), Some(85), Some(210), Some(480), Some(756)),
        "the footer carries the per-ride sensor summary"
    );
    assert_eq!(info.bike, obc_formats::bike::BikeType::Gravel);
    assert_eq!(info.trip, obc_formats::ride::TripRef::new(TRIP_KEY, 1, 3));
    assert_eq!(info.trip_name.as_str(), TRIP_NAME);
    assert_eq!(ride.len() as u64, obc_formats::ride::checked_object_len(info.point_count).unwrap());

    let (mut p, mut facts) = (obc_route::Profile::EMPTY, obc_route::RideTrackFacts::EMPTY);
    let mut preview = heapless::Vec::<_, 3>::new();
    obc_route::ride_track_into(&SliceSource(&ride), &mut p, &mut facts, &mut preview).unwrap();
    assert_eq!((p.min_ele_m, p.max_ele_m), (214, 225));
    assert_eq!(preview.as_slice(), &[(7_800_000, 48_000_000), (7_801_200, 48_001_000), (7_803_000, 48_002_000)]);
}

/// The trip vector pins §7.7, including full-width ids and its self-describing length.
#[test]
fn trip_vectors_are_self_consistent() {
    let trip = fixture("trip-v3.bin");
    assert_eq!(trip[0], 3, "trip object version");
    assert_eq!(trip[1], 0, "reserved");
    let day_count = u16::from_le_bytes([trip[2], trip[3]]);
    assert_eq!(day_count as usize, TRIP_DAYS.len());
    let name_len = trip[4] as usize;
    assert_eq!(&trip[5..5 + name_len], TRIP_NAME.as_bytes());
    assert_eq!(u16::from_le_bytes([trip[54], trip[55]]), TRIP_START_DATE);
    assert_eq!(u64::from_le_bytes(trip[56..64].try_into().unwrap()), TRIP_KEY);
    // Length is self-describing: 64-byte header + 16 bytes/day.
    assert_eq!(trip.len(), 64 + 16 * day_count as usize);
    let days: Vec<(u64, u32, u32)> = trip[64..]
        .as_chunks::<16>()
        .0
        .iter()
        .map(|d| {
            (
                u64::from_le_bytes(d[0..8].try_into().unwrap()),
                u32::from_le_bytes(d[8..12].try_into().unwrap()),
                u32::from_le_bytes(d[12..16].try_into().unwrap()),
            )
        })
        .collect();
    assert_eq!(days, TRIP_DAYS);
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
