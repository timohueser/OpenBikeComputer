//! Contract tests over the checked-in `protocol-vectors/` fixtures: every file must
//! equal its spec-derived builder byte-for-byte, and the route vectors must load and
//! ride through `obc-route` — the firmware-side half of the S0 shared-vector pin
//! (the app's `swift test` consumes the same files).

use obc_route::{for_each_waypoint, RouteIndex, RouteReader, SliceSource, MAX_POINTS_PER_CHUNK};
use obc_vectors::{all, crc32, dir, ride_v1};

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

/// Every checked-in fixture equals its builder's output. A failure means either an
/// accidental codec drift (fix the code) or a deliberate spec change (regenerate the
/// fixtures **and** flag the app side — the Swift tests pin the same bytes).
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
}

/// The upload descriptor announces the waypoint route's **actual** size and CRC —
/// the fixtures form one coherent transfer transcript, not isolated blobs.
#[test]
fn upload_transcript_is_self_consistent() {
    let route = fixture("route-waypoints.obcr");
    let start = fixture("transfer-upload-start.bin");
    let resume = fixture("transfer-upload-resume.bin");
    let result = fixture("status-transfer-result.bin");

    assert_eq!(start.len(), 16);
    assert_eq!(start[0], 1, "op = upload");
    assert_eq!(start[1], 1, "type = route");
    assert_eq!(u16::from_le_bytes([start[2], start[3]]), 0xFFFF, "id = new");
    assert_eq!(u32::from_le_bytes([start[4], start[5], start[6], start[7]]) as usize, route.len());
    assert_eq!(u32::from_le_bytes([start[8], start[9], start[10], start[11]]), crc32(&route));
    assert_eq!(&start[12..16], &[0u8; 4], "fresh upload starts at 0");

    // The resume differs from the fresh start only in its offset.
    assert_eq!(&resume[..12], &start[..12]);
    let offset = u32::from_le_bytes([resume[12], resume[13], resume[14], resume[15]]);
    assert!(offset > 0 && (offset as usize) < route.len());

    // The closing result: committed (0), every byte durable.
    assert_eq!(result.len(), 8);
    assert_eq!(result[0], 1, "status msg = transferResult");
    assert_eq!(result[3], 0, "committed");
    assert_eq!(u32::from_le_bytes([result[4], result[5], result[6], result[7]]) as usize, route.len());
}

/// The ride object's length is fully determined by its header (spec §7.2).
#[test]
fn ride_vector_length_is_self_describing() {
    let ride = ride_v1();
    assert_eq!(fixture("ride-v1.bin"), ride);
    let name_len = u16::from_le_bytes([ride[1], ride[2]]) as usize;
    let count_off = 19 + name_len; // version + name_len + name + the five stat fields
    let point_count = u32::from_le_bytes(ride[count_off..count_off + 4].try_into().unwrap());
    // Header is 23 bytes + name; each point 14.
    assert_eq!(ride.len(), 23 + name_len + 14 * point_count as usize);
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
