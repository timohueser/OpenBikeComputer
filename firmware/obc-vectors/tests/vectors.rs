//! Contract tests over the checked-in `protocol-vectors/` fixtures: every file must
//! equal its spec-derived builder byte-for-byte, and the route vectors must load and
//! ride through `obc-route`. The app's `swift test` consumes the same files.

use obc_route::{for_each_waypoint, RouteIndex, RouteObjectInfo, RouteReader, SliceSource, MAX_POINTS_PER_CHUNK};
use obc_vectors::{all, crc32, dir, ride_v1, ride_v2};

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(dir().join(name)).unwrap_or_else(|e| {
        panic!("fixture {name} unreadable ({e}) — run `cargo test -p obc-vectors regenerate -- --ignored`")
    })
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
    assert_eq!(v1.len() as u32, obc_route::ride_object_len(info.version, info.name.len(), info.point_count));

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
    assert_eq!(v2.len() as u32, obc_route::ride_object_len(info.version, info.name.len(), info.point_count));

    // The elevation profile reader streams the v2 object's points (p2's ele sentinel is skipped).
    let p = obc_route::ride_elevation_profile(&SliceSource(&v2)).unwrap();
    assert_eq!((p.min_ele_m, p.max_ele_m), (214, 219), "the ele-sentinel point contributes no sample");
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
