//! Builders for the shared S0 wire-protocol test vectors (`protocol-vectors/`).
//!
//! Each function constructs one fixture directly from the spec text
//! (`obc-ble-interface-spec.md` / `OBCR_Spec.md`), independently of the production
//! codecs on either side. The checked-in fixture files are these builders' output;
//! `tests/vectors.rs` asserts they haven't drifted.
//!
//! **Two documented exceptions**, both conversion *outputs* rather than wire layouts:
//! [`build_route`] runs the real `gpx_to_obcr` and [`track_export_gpx`] the real
//! `track_to_gpx`, because neither serialization has a spec to rebuild from — the converter
//! *is* the contract. Those two fixtures therefore pin **agreement**, not correctness: a bug
//! in the converter moves the fixture with it. What they catch is a second implementation
//! drifting from the first, which is exactly their job — the iOS OBCR encoder and the
//! browser's wasm bridge are both held to these bytes.
//!
//! Regenerate after a deliberate spec change with:
//!
//! ```text
//! cargo test -p obc-vectors regenerate -- --ignored
//! ```

use std::path::PathBuf;

use obc_formats::io::{ByteSink, Error, SliceSource};
use obc_route::gpx_to_obcr;

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

/// An in-memory [`ByteSink`] for the streaming converters below.
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

/// Convert a GPX string to OBCR v2 bytes via the reference converter.
pub fn build_route(gpx: &str) -> Vec<u8> {
    let mut sink = VecSink(Vec::new());
    gpx_to_obcr(&SliceSource(gpx.as_bytes()), ROUTE_NAME, &mut sink).unwrap();
    sink.0
}

/// Recorded-track fixture name — carries an `&` so the GPX export's XML escaping is pinned too.
pub const TRACK_NAME: &str = "Schauinsland & back";

/// A recorded `.obct` ride log: a flat array of 20-byte records, **no header**
/// (`obc-formats/src/track.rs`, the byte authority). Built field-by-field from that layout rather
/// than through `encode_record`, so the fixture pins the record independently of the production
/// codec — the same rule the rest of this module follows.
///
/// Shaped for **coverage, not plausibility** (it teleports between hemispheres): five points
/// spanning every branch the GPX exporter has —
///
/// | # | why it is here |
/// | :-- | :-- |
/// | 0 | first point (always opens a `<trkseg>`), all three sensor fields present |
/// | 1 | cadence absent — the `TrackPointExtension` wrapper still appears, one element short |
/// | 2 | every sensor absent — no `<extensions>` block at all |
/// | 3 | `segment_start` after a pause (a second `<trkseg>`), negative lat/lon/elevation, and **power only** (no wrapper) |
/// | 4 | zeroes everywhere: `0.000000` coordinate formatting, and `hr`/`cad`/`pwr` = 0 as real values, distinct from the `0xFF`/`0xFFFF` absent sentinels |
///
/// …plus a deliberate **7-byte partial record** at the end: a power-loss mid-write leaves one, and
/// the log stays valid to the 20-byte boundary, so the exporter must ignore it.
pub fn track_log() -> Vec<u8> {
    /// One record's fields, named after the layout they serialize into. `0xFF` / `0xFFFF` in the
    /// sensor fields are the "absent" sentinels.
    struct Rec {
        lon: i32,
        lat: i32,
        ele: i16,
        flags: u16,
        t_ms: u32,
        hr: u8,
        cad: u8,
        pwr: u16,
    }
    let rec = |lon, lat, ele, flags, t_ms, hr, cad, pwr| Rec { lon, lat, ele, flags, t_ms, hr, cad, pwr };
    let points = [
        rec(7_842_000, 47_995_000, 300, 1, 0, 132, 78, 185),
        rec(7_843_500, 47_996_000, 305, 0, 1_000, 138, 0xFF, 190),
        rec(7_845_000, 47_997_200, 318, 0, 2_000, 0xFF, 0xFF, 0xFFFF),
        rec(-122_419_400, -37_774_900, -12, 1, 63_000, 0xFF, 0xFF, 240),
        rec(0, 0, 0, 0, 64_000, 0, 0, 0),
    ];
    let mut v = Vec::with_capacity(points.len() * 20 + 7);
    for p in points {
        v.extend_from_slice(&p.lon.to_le_bytes()); // 0..4
        v.extend_from_slice(&p.lat.to_le_bytes()); // 4..8
        v.extend_from_slice(&p.ele.to_le_bytes()); // 8..10
        v.extend_from_slice(&le16(p.flags)); // 10..12 — bit 0 = segment_start
        v.extend_from_slice(&le32(p.t_ms)); // 12..16
        v.push(p.hr); // 16
        v.push(p.cad); // 17
        v.extend_from_slice(&le16(p.pwr)); // 18..20
    }
    v.extend_from_slice(&[0xAB; 7]); // the truncated trailing record
    v
}

/// The GPX 1.1 export of [`track_log`], through the production converter (`track_to_gpx`).
///
/// Unlike the binary fixtures there is no independent spec to rebuild this from — the exporter's
/// serialization *is* the contract — so this goes through the real code, exactly like
/// [`build_route`] does for OBCR. Its value is cross-implementation: the browser bridge
/// (`obc-web-convert`, compiled to wasm) must reproduce these bytes character-for-character.
pub fn track_export_gpx() -> Vec<u8> {
    let mut sink = VecSink(Vec::new());
    obc_route::track_to_gpx(&SliceSource(&track_log()), TRACK_NAME, &mut sink).unwrap();
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

/// Ride object v2 (spec §7.2, epic #707): "Sensor Ride", 3 points, with the BLE-sensor summary +
/// per-point sensor fields — a mix of present and absent values (the cross-language contract SE4's
/// iOS codec mirror-pins).
///
/// Byte layout (little-endian):
/// ```text
/// Header (31 bytes + 11-byte name):
///   version      u8   = 2
///   name_len     u16  = 11 · name "Sensor Ride"
///   start_time   u32  = 1_751_460_000
///   distance     u32  = 12_345 m
///   moving_time  u32  = 3_600 s
///   avg_speed    u16  = 343 cm/s
///   climb        u16  = 120 m
///   point_count  u32  = 3
///   avg_hr       u8   = 142
///   max_hr       u8   = 176
///   avg_cad      u8   = 85
///   pad          u8   = 0
///   avg_pwr      u16  = 210
///   max_pwr      u16  = 480
/// Points (18 bytes × 3):     t   lat_1e7      lon_1e7    ele    hr    cad   pwr
///   p0 (all present):        0   480_000_000  78_000_000 214    140   84    205
///   p1 (all absent):        60   480_010_000  78_012_000 219    0xFF  0xFF  0xFFFF
///   p2 (hr+pwr, cad absent):120  480_020_000  78_030_000 i16MIN 150   0xFF  215
/// ```
/// Total = 31 + 11 + 3×18 = 96 bytes.
pub fn ride_v2() -> Vec<u8> {
    let name = "Sensor Ride".as_bytes(); // 11 ASCII bytes
    let mut v = Vec::new();
    v.push(2); // version
    v.extend_from_slice(&le16(name.len() as u16));
    v.extend_from_slice(name);
    v.extend_from_slice(&le32(1_751_460_000)); // start_time
    v.extend_from_slice(&le32(12_345)); // distance m
    v.extend_from_slice(&le32(3_600)); // moving_time s
    v.extend_from_slice(&le16(343)); // avg_speed cm/s
    v.extend_from_slice(&le16(120)); // climb m
    v.extend_from_slice(&le32(3)); // point_count
                                   // Per-ride sensor summary: avg_hr, max_hr, avg_cad, pad, avg_pwr, max_pwr.
    v.push(142); // avg_hr
    v.push(176); // max_hr
    v.push(85); // avg_cad
    v.push(0); // pad
    v.extend_from_slice(&le16(210)); // avg_pwr
    v.extend_from_slice(&le16(480)); // max_pwr
                                     // (t_offset, lat ×1e7, lon ×1e7, ele, hr, cad, pwr) — 0xFF/0xFFFF = absent.
    for (t, lat, lon, ele, hr, cad, pwr) in [
        (0u32, 480_000_000i32, 78_000_000i32, 214i16, 140u8, 84u8, 205u16),
        (60, 480_010_000, 78_012_000, 219, 0xFF, 0xFF, 0xFFFF),
        (120, 480_020_000, 78_030_000, i16::MIN, 150, 0xFF, 215),
    ] {
        v.extend_from_slice(&le32(t));
        v.extend_from_slice(&lat.to_le_bytes());
        v.extend_from_slice(&lon.to_le_bytes());
        v.extend_from_slice(&ele.to_le_bytes());
        v.push(hr);
        v.push(cad);
        v.extend_from_slice(&le16(pwr));
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

/// The `fw_version` string carried in the OBCU update-container fixture — a
/// realistic `git describe` value the iOS picker + device DIS both display.
pub const UPDATE_FW_VERSION: &str = "1.2.0+abc1234";

/// The deterministic raw application image inside the OBCU container fixture: a
/// 128-byte body whose first 32-bit word is a plausible Cortex-M initial stack
/// pointer (`0x2002_0000`, inside the nRF54L15 DK RAM — see
/// `obc_dfu::looks_like_vector_table`), then a byte ramp. Content is opaque to
/// the transfer layer; it exists so the fixture exercises the image CRC too.
pub fn update_raw_image() -> Vec<u8> {
    let mut v = Vec::with_capacity(128);
    v.extend_from_slice(&le32(0x2002_0000)); // plausible initial SP (vector-table-first)
    for i in 4u32..128 {
        v.push((i & 0xFF) as u8);
    }
    v
}

/// A full **OBCU update container** (`OBCU_Spec.md` §1, `UPDATE.BIN`): the fixed
/// 64-byte header (magic `OBCU`, version 1, raw-image length + CRC-32, NUL-padded
/// `fw_version`, header CRC-32 over bytes `0..60`) followed by [`update_raw_image`].
/// Built straight from the spec's field table — independent of the `obc-dfu`
/// production codec, which pins the same bytes from the other side. The iOS
/// companion's `OBCUHeader` decoder validates this file identically.
pub fn update_container_v1() -> Vec<u8> {
    let image = update_raw_image();
    let mut header = [0u8; 64];
    header[0..4].copy_from_slice(b"OBCU");
    header[4..6].copy_from_slice(&le16(1)); // header_version
                                            // 6..8 reserved (0)
    header[8..12].copy_from_slice(&le32(image.len() as u32));
    header[12..16].copy_from_slice(&le32(crc32(&image)));
    let vbytes = UPDATE_FW_VERSION.as_bytes();
    header[16..16 + vbytes.len()].copy_from_slice(vbytes); // NUL-padded to 32
                                                           // 48..60 reserved (0) — future signature-scheme marker
    let hcrc = crc32(&header[..60]);
    header[60..64].copy_from_slice(&le32(hcrc));

    let mut v = Vec::with_capacity(64 + image.len());
    v.extend_from_slice(&header);
    v.extend_from_slice(&image);
    v
}

/// A `transferControl` descriptor (spec §4.2): 12 bytes (protocol v2 — the `offset` field is gone).
pub fn transfer_control(op: u8, ty: u8, object_id: u16, total_len: u32, crc: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(op);
    v.push(ty);
    v.extend_from_slice(&le16(object_id));
    v.extend_from_slice(&le32(total_len));
    v.extend_from_slice(&le32(crc));
    v
}

/// `status` message `transferResult` (spec §4.3, `msg = 1`): 8 bytes.
pub fn status_transfer_result(object_id: u16, status: u8, committed_offset: u32) -> Vec<u8> {
    let mut v = vec![1u8];
    v.extend_from_slice(&le16(object_id));
    v.push(status);
    v.extend_from_slice(&le32(committed_offset));
    v
}

/// `status` message `downloadAnnounce` (spec §4.3, `msg = 4`): the `msg` byte + the 12-byte
/// `transferControl` descriptor with `total_len`/`crc32` filled in (protocol v2 folds the announce
/// onto the `status` envelope). 13 bytes.
pub fn status_download_announce(ty: u8, object_id: u16, total_len: u32, crc: u32) -> Vec<u8> {
    let mut v = vec![4u8];
    v.extend_from_slice(&transfer_control(2, ty, object_id, total_len, crc)); // op = 2 (download)
    v
}

/// The full `protocolVersion` read (spec §1): `version u16 · store_epoch u32 · obcm_version u8`.
/// 7 bytes.
pub fn version_read(version: u16, store_epoch: u32, obcm_version: u8) -> Vec<u8> {
    let mut v = version_read_noobcm(version, store_epoch);
    v.push(obcm_version);
    v
}

/// The **pre-E1** `protocolVersion` read (spec §1): `version u16 · store_epoch u32`, 6 bytes — what
/// a firmware that predates the `obcm_version` byte serves. Every decoder must take it as
/// `obcmVersion = nil` (unknown), never as a fabricated `0`, which would read as "supports OBCM v0"
/// and refuse every real map.
pub fn version_read_noobcm(version: u16, store_epoch: u32) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&le16(version));
    v.extend_from_slice(&le32(store_epoch));
    v
}

/// The **version-only** `protocolVersion` read (spec §1, card-resident epoch #776): a device with
/// **no mounted store** has no epoch, so it serves just `version u16` — 2 bytes. The app decodes the
/// short read as `storeEpoch = nil` and fail-closes the ack. Never a fabricated epoch (0 is legal).
pub fn version_read_nostore(version: u16) -> Vec<u8> {
    le16(version).to_vec()
}

/// `status` message `storeChanged` (spec §4.3): 6 bytes.
pub fn status_store_changed(ty: u8, revision: u32) -> Vec<u8> {
    let mut v = vec![2u8, ty];
    v.extend_from_slice(&le32(revision));
    v
}

/// `status` message `commandResult` (spec §4.3): 4 bytes.
pub fn status_command_result(cmd: u8, status: u8, detail: u8) -> Vec<u8> {
    vec![3u8, cmd, status, detail]
}

/// The `ackRides` command write (spec §4.4, cmd 2): `cmd u8 · count u8 · count × object_id u16 LE`.
pub fn command_ack_rides(ids: &[u16]) -> Vec<u8> {
    let mut v = vec![2u8, ids.len() as u8];
    for id in ids {
        v.extend_from_slice(&le16(*id));
    }
    v
}

/// The `setClock` command write (spec §4.4, cmd 5, epic #638 S2): `cmd u8 = 5 · utc u32 LE ·
/// offset_min i16 LE`. 7 bytes.
pub fn command_set_clock(utc: u32, offset_min: i16) -> Vec<u8> {
    let mut v = vec![5u8];
    v.extend_from_slice(&le32(utc));
    v.extend_from_slice(&offset_min.to_le_bytes());
    v
}

/// The `setRouteRetention` command write (spec §4.4, cmd 6, epic #638 S4): `cmd u8 = 6 · object_id
/// u16 LE · retention u8`. 4 bytes.
pub fn command_set_route_retention(object_id: u16, retention: u8) -> Vec<u8> {
    let mut v = vec![6u8];
    v.extend_from_slice(&le16(object_id));
    v.push(retention);
    v
}

/// The `route-list.bin` entries' auto-expiry spread (epic #638 S4, spec §7.4): id 7 is a **live
/// countdown** (Week2 = `3`, a nonzero `expires_at`), id 8 has a **not-yet-started** clock (Day1 =
/// `1`, `expires_at 0` because `last_used == 0`), and id 9 is **Never** (retention `0`, `expires_at
/// 0`). `EXPIRES_AT_LIVE` is `command-set-clock.bin`'s UTC + a 2-week window, so the fixtures agree.
pub const ROUTE_EXPIRES_AT_LIVE: u32 = 1_783_598_400 + 14 * 86_400;
/// `(expires_at, retention)` per `route-list.bin` entry, in id order (7, 8, 9) — see [`route_list`].
pub const ROUTE_RETENTION_SPREAD: [(u32, u8); 3] = [(ROUTE_EXPIRES_AT_LIVE, 3), (0, 1), (0, 0)];

/// One `routeList` entry (spec §7.4): **84 bytes** — the 76-byte protocol-v2 core (name zero-padded to
/// 48, trailing whole-object content `crc32`, `0` = unknown) + the auto-expiry tail `expires_at u32 ·
/// retention u8 · reserved u8[3]` (epic #638 S4). The tail sits **after** the content `crc32` — it is
/// device-computed volatile state, not route-content identity — so the 76-byte core is byte-identical.
#[allow(clippy::too_many_arguments)] // mirrors the spec's field list one-to-one
pub fn route_list_entry(
    object_id: u16,
    byte_len: u32,
    distance_m: u32,
    ascent_m: u32,
    point_count: u32,
    waypoint_count: u16,
    name: &str,
    crc: u32,
    expires_at: u32,
    retention: u8,
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
    v.extend_from_slice(&le32(crc)); // whole-object content CRC-32 (offset 72)
    v.extend_from_slice(&le32(expires_at)); // auto-expiry tail (offset 76) — outside the content crc32
    v.push(retention); // offset 80
    v.extend_from_slice(&[0u8; 3]); // reserved (offset 81)
    assert_eq!(v.len(), 84);
    v
}

/// A whole `routeList` object (spec §7.4): the **6-byte** v2 list header
/// (`version 2 · entry_len 84 · count · total`) + packed 84-byte entries. `total` = the full catalog
/// size before the `MAX_ROUTES` cap (equal to `count` when nothing was dropped).
pub fn route_list(entries: &[Vec<u8>], total: u16) -> Vec<u8> {
    let mut v = vec![2u8, 84];
    v.extend_from_slice(&le16(entries.len() as u16));
    v.extend_from_slice(&le16(total));
    for e in entries {
        v.extend_from_slice(e);
    }
    v
}

/// Trip fixture name (also the trip object header name field).
pub const TRIP_NAME: &str = "Alpen Traverse";

/// The two resolvable stage route ids in `trip-v1.bin` — the ids of the two `route-list.bin`
/// entries, so the `tripList` totals sum their distance/ascent.
pub const TRIP_STAGE_IDS: [u16; 2] = [7, 8];

/// The deliberately **dangling** third stage id in `trip-v1.bin`: a route id no fixture holds, so the
/// device tolerates it on read and the `tripList` totals skip it (spec §7.7 / §7.4).
pub const TRIP_DANGLING_STAGE: u16 = 99;

/// The trip's own device-assigned object id (its counter is separate from routes/rides, §4.1).
pub const TRIP_ID: u16 = 1;

/// Trip object v1 (spec §7.7): a 56-byte header (`version 1 · stage_count u16 · name ≤ 48`) followed
/// by `stage_count × u16` route object ids in ride order. Length = `56 + 2·stage_count`.
pub fn trip_v1(name: &str, stages: &[u16]) -> Vec<u8> {
    let mut v = Vec::new();
    v.push(1); // version
    v.push(0); // reserved
    v.extend_from_slice(&le16(stages.len() as u16)); // stage_count
    v.push(name.len() as u8); // name_len
    let mut padded = [0u8; 48];
    padded[..name.len()].copy_from_slice(name.as_bytes());
    v.extend_from_slice(&padded); // name, zero-padded to 48
    v.extend_from_slice(&[0u8; 3]); // reserved
    assert_eq!(v.len(), 56, "trip object header is 56 bytes");
    for &id in stages {
        v.extend_from_slice(&le16(id)); // stage route id, ride order
    }
    v
}

/// One `tripList` entry (spec §7.4): **76 bytes**, mirroring `routeList` — name zero-padded to 48,
/// trailing whole-object `crc32` of the stored trip bytes (`0` = unknown). `total_distance_m` /
/// `total_ascent_m` are summed over the trip's **resolvable** stages; `stage_count` counts every
/// stored stage (dangling refs included).
#[allow(clippy::too_many_arguments)] // mirrors the spec's field list one-to-one
pub fn trip_list_entry(
    object_id: u16,
    byte_len: u32,
    total_distance_m: u32,
    total_ascent_m: u32,
    stage_count: u16,
    name: &str,
    crc: u32,
) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&le16(object_id));
    v.extend_from_slice(&le16(0)); // reserved
    v.extend_from_slice(&le32(byte_len));
    v.extend_from_slice(&le32(total_distance_m));
    v.extend_from_slice(&le32(total_ascent_m));
    v.extend_from_slice(&le16(stage_count));
    v.extend_from_slice(&le16(0)); // reserved
    v.push(name.len() as u8);
    let mut padded = [0u8; 48];
    padded[..name.len()].copy_from_slice(name.as_bytes());
    v.extend_from_slice(&padded);
    v.extend_from_slice(&[0u8; 3]); // reserved
    v.extend_from_slice(&le32(crc)); // whole-object content CRC-32
    assert_eq!(v.len(), 76);
    v
}

/// A whole `tripList` object (spec §7.4): the **6-byte** v2 list header
/// (`version 2 · entry_len 76 · count · total`) + packed 76-byte entries. `total` = the full trip
/// catalog size before the `MAX_TRIPS` cap (equal to `count` when nothing was dropped).
pub fn trip_list(entries: &[Vec<u8>], total: u16) -> Vec<u8> {
    let mut v = vec![2u8, 76];
    v.extend_from_slice(&le16(entries.len() as u16));
    v.extend_from_slice(&le16(total));
    for e in entries {
        v.extend_from_slice(e);
    }
    v
}

/// Every fixture as `(file name, bytes)`. The transfer descriptors' `total_len`/
/// `crc32` are the actual length and CRC of `route-waypoints.obcr`, tying the
/// fixtures together end-to-end.
pub fn all() -> Vec<(&'static str, Vec<u8>)> {
    let route_wp = build_route(ROUTE_GPX);
    let route_plain = build_route(&route_gpx_plain());
    let (len, crc) = (route_wp.len() as u32, crc32(&route_wp));
    let (plain_len, plain_crc) = (route_plain.len() as u32, crc32(&route_plain));
    let trip = trip_v1(TRIP_NAME, &[TRIP_STAGE_IDS[0], TRIP_STAGE_IDS[1], TRIP_DANGLING_STAGE]);
    let (trip_len, trip_crc) = (trip.len() as u32, crc32(&trip));
    vec![
        ("route-waypoints.obcr", route_wp),
        ("route-plain.obcr", route_plain),
        // The recorded-track pair (epic #894, A2): the device's 20-byte-record ride log and the
        // GPX its Finish conversion writes from it. Checked in together because the *pair* is the
        // contract the browser conversion bridge must reproduce byte-for-byte in wasm.
        ("track-log.obct", track_log()),
        ("track-export.gpx", track_export_gpx()),
        ("ride-v1.bin", ride_v1()),
        ("ride-v2.bin", ride_v2()),
        ("config-v1.bin", config_v1()),
        // The full protocolVersion read (spec §1): version 2 + a store epoch nonce + the OBCM
        // map-format version the reader reads. The last one is **self-sourced** from
        // `obc_formats::obcm::VERSION` rather than written out as a literal: the fixture's whole
        // point is to be the bytes a current device serves, so an OBCM bump must re-cut it (and, via
        // manifest.json, force the Swift + TS consumers of that number to be looked at) rather than
        // leave three implementations pinned to a number the firmware stopped saying.
        ("version-read.bin", version_read(2, 0xA1B2_C3D4, obc_formats::obcm::VERSION)),
        // The pre-E1 (#911) read: version + epoch, no obcm byte — an older firmware talking to a
        // newer host. Decodes with `obcmVersion` absent, never a fabricated 0.
        ("version-read-noobcm.bin", version_read_noobcm(2, 0xA1B2_C3D4)),
        // The version-only protocolVersion read (spec §1, #776): a device with no mounted store
        // serves just the 2-byte version — the app treats the absent epoch as a failed identity read.
        ("version-read-nostore.bin", version_read_nostore(2)),
        // op=1 upload, type=1 route, id 0xFFFF (new) — 12 bytes (no offset in v2).
        ("transfer-upload-start.bin", transfer_control(1, 1, 0xFFFF, len, crc)),
        // op=2 download request: type=7 rideList, id 0, len/crc unknown.
        ("transfer-download-request.bin", transfer_control(2, 7, 0, 0, 0)),
        // op=3 abort of the active route upload.
        ("transfer-abort.bin", transfer_control(3, 1, 0xFFFF, 0, 0)),
        // The download announce (status msg 4): a route download (id 7 — the waypoint route in
        // route-list.bin), its size + CRC filled.
        ("status-download-announce.bin", status_download_announce(1, 7, len, crc)),
        // Closing result: committed, assigned id 7, all bytes durable.
        ("status-transfer-result.bin", status_transfer_result(7, 0, len)),
        // Reject: a new-route upload (id 0xFFFF) refused at descriptor-open time
        // because the catalog is full. status=6 storageFull, nothing committed.
        ("status-transfer-storage-full.bin", status_transfer_result(0xFFFF, 6, 0)),
        ("status-store-changed.bin", status_store_changed(1, 42)),
        // The phone's ride-possession ack (cmd 2): three stored rides.
        ("command-ack-rides.bin", command_ack_rides(&[3, 5, 9])),
        // Its answer: ok, detail = 3 newly-flagged rides.
        ("status-command-result-ack.bin", status_command_result(2, 0, 3)),
        // The phone's clock stamp (cmd 5, epic #638 S2): 2026-07-09T12:00:00Z (unix 1783598400),
        // +02:00 (offset 120 min). 7 bytes.
        ("command-set-clock.bin", command_set_clock(1_783_598_400, 120)),
        // The phone's route-retention set (cmd 6, epic #638 S4): route id 7 → retention 3 (2 weeks).
        // 4 bytes. Answered with a bare commandResult(ok) + a companion storeChanged(route) on a real
        // change (no storeChanged / no bump when the value is unchanged — the idempotence pin).
        ("command-set-route-retention.bin", command_set_route_retention(7, 3)),
        // The OBCU firmware-update container (spec §1) — a `fwImage` payload (spec
        // §7.6, id 0): 64-byte header + a 128-byte raw image. Pinned on the device
        // side by `obc-dfu` and on the app side by the iOS `OBCUHeader` decoder.
        ("update-container-v1.bin", update_container_v1()),
        // Catalog for the stored route fixtures + a synthetic third entry: fields from their OBCR
        // headers (distance 2207 m, ascent 76 m, 9 points), ids continuing from 7, each with its
        // whole-object content CRC-32, and the epic #638 S4 auto-expiry tail spanning a spread of
        // retention states (`ROUTE_RETENTION_SPREAD`): id 7 a live countdown (Week2, nonzero
        // expires_at), id 8 a not-yet-started clock (Day1, expires_at 0), id 9 a Never route
        // (retention 0, expires_at 0). Id 9 is synthetic (no `.obcr` file — reuses the plain route's
        // size/CRC), present only to pin the Never state on the wire. total = count (nothing truncated).
        (
            "route-list.bin",
            route_list(
                &[
                    route_list_entry(
                        7,
                        len,
                        2207,
                        76,
                        9,
                        2,
                        ROUTE_NAME,
                        crc,
                        ROUTE_RETENTION_SPREAD[0].0,
                        ROUTE_RETENTION_SPREAD[0].1,
                    ),
                    route_list_entry(
                        8,
                        plain_len,
                        2207,
                        76,
                        9,
                        0,
                        ROUTE_NAME,
                        plain_crc,
                        ROUTE_RETENTION_SPREAD[1].0,
                        ROUTE_RETENTION_SPREAD[1].1,
                    ),
                    route_list_entry(
                        9,
                        plain_len,
                        2207,
                        76,
                        9,
                        0,
                        ROUTE_NAME,
                        plain_crc,
                        ROUTE_RETENTION_SPREAD[2].0,
                        ROUTE_RETENTION_SPREAD[2].1,
                    ),
                ],
                3,
            ),
        ),
        // A trip (§7.7): "Alpen Traverse", 3 stages referencing route ids 7 and 8 (both stored in
        // route-list.bin) plus one deliberately dangling id (99) that pins read-tolerance.
        ("trip-v1.bin", trip),
        // The catalog for that one trip (§7.4): byte_len = the trip file; totals summed over the two
        // resolvable stages only (2×2207 m, 2×76 m); stage_count = 3 as stored (incl. the dangling
        // ref); trailing crc32 = the trip file's whole-object CRC-32. total = count (nothing dropped).
        (
            "trip-list.bin",
            trip_list(&[trip_list_entry(TRIP_ID, trip_len, 2 * 2207, 2 * 76, 3, TRIP_NAME, trip_crc)], 1),
        ),
    ]
}
