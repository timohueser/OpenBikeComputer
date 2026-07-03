//! Builders for the shared S0 wire-protocol test vectors (`protocol-vectors/`).
//!
//! Each function constructs one fixture directly from the spec text
//! (`obc-ble-interface-spec.md` / `OBCR_Spec.md`), independently of the production
//! codecs on either side. The checked-in fixture files are these builders' output;
//! `tests/vectors.rs` asserts they haven't drifted. Regenerate after a deliberate
//! spec change with:
//!
//! ```text
//! cargo test -p obc-vectors regenerate -- --ignored
//! ```

use std::path::PathBuf;

use obc_route::{gpx_to_obcr, ByteSink, Error, SliceSource};

/// The `protocol-vectors/` directory at the repo root.
pub fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol-vectors")
}

/// CRC-32/IEEE per spec §6: reflected, poly `0xEDB88320`, init/xorout `0xFFFFFFFF`.
/// Check value: `crc32(b"123456789") == 0xCBF43926`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in bytes {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { 0xEDB8_8320 ^ (crc >> 1) } else { crc >> 1 };
        }
    }
    crc ^ 0xFFFF_FFFF
}

/// The deterministic route source: a short rolling track at 48°N with two `<wpt>`
/// waypoints listed out of ride order (as GPX carries them).
pub const ROUTE_GPX: &str = include_str!("route-source.gpx");

/// Route fixture name (also the OBCR header name field).
pub const ROUTE_NAME: &str = "Vector Loop";

/// `ROUTE_GPX` with its `<wpt>` elements removed — the same track, no waypoints.
pub fn route_gpx_plain() -> String {
    ROUTE_GPX.lines().filter(|l| !l.trim_start().starts_with("<wpt ")).fold(String::new(), |mut s, l| {
        s.push_str(l);
        s.push('\n');
        s
    })
}

/// Convert a GPX string to OBCR v2 bytes via the reference converter.
pub fn build_route(gpx: &str) -> Vec<u8> {
    struct VecSink(Vec<u8>);
    impl ByteSink for VecSink {
        fn write(&mut self, b: &[u8]) -> Result<(), Error> {
            self.0.extend_from_slice(b);
            Ok(())
        }
        fn patch_at(&mut self, off: u32, b: &[u8]) -> Result<(), Error> {
            let o = off as usize;
            self.0[o..o + b.len()].copy_from_slice(b);
            Ok(())
        }
    }
    let mut sink = VecSink(Vec::new());
    gpx_to_obcr(&SliceSource(gpx.as_bytes()), ROUTE_NAME, &mut sink).unwrap();
    sink.0
}

fn le16(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}
fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Ride object v1 (spec §7.2): "Höhenweg", 3 points, the last without elevation.
pub fn ride_v1() -> Vec<u8> {
    let name = "Höhenweg".as_bytes(); // 9 UTF-8 bytes
    let mut v = Vec::new();
    v.push(1); // version
    v.extend_from_slice(&le16(name.len() as u16));
    v.extend_from_slice(name);
    v.extend_from_slice(&le32(1_751_450_000)); // start_time
    v.extend_from_slice(&le32(42_500)); // distance m
    v.extend_from_slice(&le32(9_000)); // moving_time s
    v.extend_from_slice(&le16(472)); // avg_speed cm/s
    v.extend_from_slice(&le16(810)); // climb m
    v.extend_from_slice(&le32(3)); // point_count
                                   // (t_offset, lat ×1e7, lon ×1e7, ele)
    for (t, lat, lon, ele) in [
        (0u32, 480_000_000i32, 78_000_000i32, 214i16),
        (60, 480_010_000, 78_012_000, 219),
        (120, 480_020_000, 78_030_000, i16::MIN),
    ] {
        v.extend_from_slice(&le32(t));
        v.extend_from_slice(&lat.to_le_bytes());
        v.extend_from_slice(&lon.to_le_bytes());
        v.extend_from_slice(&ele.to_le_bytes());
    }
    v
}

/// Config object v1 (spec §7.3): name "OBC Tourer", metric.
pub fn config_v1() -> Vec<u8> {
    let name = b"OBC Tourer";
    let mut v = Vec::new();
    v.extend_from_slice(&le16(name.len() as u16));
    v.extend_from_slice(name);
    v.push(0); // units: metric
    v
}

/// A `transferControl` descriptor (spec §4.2): 16 bytes.
pub fn transfer_control(op: u8, ty: u8, object_id: u16, total_len: u32, crc: u32, offset: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(op);
    v.push(ty);
    v.extend_from_slice(&le16(object_id));
    v.extend_from_slice(&le32(total_len));
    v.extend_from_slice(&le32(crc));
    v.extend_from_slice(&le32(offset));
    v
}

/// `status` message `transferResult` (spec §4.3): 8 bytes.
pub fn status_transfer_result(object_id: u16, status: u8, committed_offset: u32) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend_from_slice(&le16(object_id));
    v.push(status);
    v.extend_from_slice(&le32(committed_offset));
    v
}

/// `status` message `storeChanged` (spec §4.3): 6 bytes.
pub fn status_store_changed(ty: u8, revision: u32) -> Vec<u8> {
    let mut v = vec![2u8, ty];
    v.extend_from_slice(&le32(revision));
    v
}

/// One `routeList` entry (spec §7.4): 72 bytes, name zero-padded to 48.
#[allow(clippy::too_many_arguments)] // mirrors the spec's field list one-to-one
pub fn route_list_entry(
    object_id: u16,
    byte_len: u32,
    distance_m: u32,
    ascent_m: u32,
    point_count: u32,
    waypoint_count: u16,
    name: &str,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&le16(object_id));
    v.extend_from_slice(&le16(0)); // reserved
    v.extend_from_slice(&le32(byte_len));
    v.extend_from_slice(&le32(distance_m));
    v.extend_from_slice(&le32(ascent_m));
    v.extend_from_slice(&le32(point_count));
    v.extend_from_slice(&le16(waypoint_count));
    v.push(name.len() as u8);
    let mut padded = [0u8; 48];
    padded[..name.len()].copy_from_slice(name.as_bytes());
    v.extend_from_slice(&padded);
    v.push(0); // reserved
    assert_eq!(v.len(), 72);
    v
}

/// A whole `routeList` object (spec §7.4): the 4-byte list header + packed 72-byte entries.
pub fn route_list(entries: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![1u8, 72];
    v.extend_from_slice(&le16(entries.len() as u16));
    for e in entries {
        v.extend_from_slice(e);
    }
    v
}

/// `objectStore` digest (spec §4.5): 10 bytes.
pub fn object_store(revision: u32, routes: u16, rides: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&le32(revision));
    v.extend_from_slice(&le16(routes));
    v.extend_from_slice(&le16(rides));
    v.extend_from_slice(&le16(0));
    v
}

/// Every fixture as `(file name, bytes)`. The transfer descriptors' `total_len`/
/// `crc32` are the actual length and CRC of `route-waypoints.obcr`, tying the
/// fixtures together end-to-end.
pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    let route_wp = build_route(ROUTE_GPX);
    let route_plain = build_route(&route_gpx_plain());
    let (len, crc) = (route_wp.len() as u32, crc32(&route_wp));
    let plain_len = route_plain.len() as u32;
    let resume_offset = len / 2;
    vec![
        ("route-waypoints.obcr", route_wp),
        ("route-plain.obcr", route_plain),
        ("ride-v1.bin", ride_v1()),
        ("config-v1.bin", config_v1()),
        // op=1 upload, type=1 route, id 0xFFFF (new).
        ("transfer-upload-start.bin", transfer_control(1, 1, 0xFFFF, len, crc, 0)),
        // Non-zero offset: pins the `offset` field's byte layout. Uploads are NOT
        // resumable (spec §1 principle 4) — an encoding fixture, not a resume flow.
        ("transfer-upload-resume.bin", transfer_control(1, 1, 0xFFFF, len, crc, resume_offset)),
        // op=2 download request: type=7 rideList, id 0, len/crc unknown.
        ("transfer-download-request.bin", transfer_control(2, 7, 0, 0, 0, 0)),
        // op=3 abort of the active route upload.
        ("transfer-abort.bin", transfer_control(3, 1, 0xFFFF, 0, 0, 0)),
        // Closing result: committed, assigned id 7, all bytes durable.
        ("status-transfer-result.bin", status_transfer_result(7, 0, len)),
        ("status-store-changed.bin", status_store_changed(1, 42)),
        ("object-store.bin", object_store(42, 3, 5)),
        // Catalog for both stored route fixtures: fields from their OBCR headers
        // (distance 2207 m, ascent 76 m, 9 points), ids continuing from 7.
        (
            "route-list.bin",
            route_list(&[
                route_list_entry(7, len, 2207, 76, 9, 2, ROUTE_NAME),
                route_list_entry(8, plain_len, 2207, 76, 9, 0, ROUTE_NAME),
            ]),
        ),
    ]
}
