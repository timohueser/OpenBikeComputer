//! The **ride object** (v1 / v2) — the compact tracked-ride layout a ride crosses the BLE link
//! as (`obc-ble-interface-spec.md` §7.2) *and* the durable per-ride file the device stores at
//! Finish (`/tracks/RD{id}.ORD`). The stored file **is** the wire object, so a BLE ride
//! download is a verbatim byte stream with no encode on the transfer path.
//!
//! Layout (little-endian; pinned by `protocol-vectors/ride-v{1,2}.bin` against the Swift
//! `RideObjectCodec`):
//!
//! ```text
//! Header (v1: 23 bytes + name  ·  v2: 31 bytes + name):
//!   version      u8   = 1 or 2
//!   name_len     u16  · name UTF-8 (name_len bytes follow immediately)
//!   start_time   u32  unix seconds
//!   distance     u32  meters
//!   moving_time  u32  seconds
//!   avg_speed    u16  cm/s
//!   climb        u16  meters
//!   point_count  u32
//!   -- v2 only, the per-ride BLE-sensor summary (epic #707): --
//!   avg_hr       u8   bpm    · 0xFF   = no data this ride
//!   max_hr       u8   bpm    · 0xFF   = no data
//!   avg_cad      u8   rpm    · 0xFF   = no data
//!   pad          u8   = 0
//!   avg_pwr      u16  W      · 0xFFFF = no data
//!   max_pwr      u16  W      · 0xFFFF = no data
//! Point record (v1: 14 bytes · v2: 18 bytes, × point_count):
//!   t_offset  u32  seconds since start_time
//!   lat       i32  degrees × 1e7
//!   lon       i32  degrees × 1e7
//!   ele       i16  meters · i16::MIN = no elevation
//!   -- v2 only: --
//!   hr        u8   bpm · 0xFF   = absent
//!   cad       u8   rpm · 0xFF   = absent
//!   pwr       u16  W   · 0xFFFF = absent
//! ```
//!
//! The byte length is fully determined **per version**: v1 `23 + name_len + 14 × point_count`,
//! v2 `31 + name_len + 18 × point_count` — a decoder must reject a payload whose length disagrees
//! (spec §7.2), which is also this file's power-cut guard (a torn write leaves a shorter file).
//! The device serves whichever version it wrote the file as; the app accepts both — old v1 rides
//! on the card must still list, download and delete (v2 is an additive object version, no protocol
//! bump, spec §1).
//!
//! [`track_to_ride`] is the Finish-time converter (it writes v2): one streaming pass over the
//! recorded `.obct` log, no resident whole-ride buffer. Coordinate translation: track records
//! store **microdegrees** in `lon, lat` order; ride points store **degrees × 1e7** in `lat, lon`
//! order. Sensor fields carry 1:1 from the v2 track records (absent ↔ sentinel). The version byte
//! is held back as `0` and patched in as the final write, so an interrupted save is rejected
//! ([`Error::BadVersion`]) rather than masquerading as a ride.

use heapless::String;

use crate::track::decode_record;
use obc_formats::io::{ByteSink, ByteSource, Error};
use obc_formats::obcr::NAME_CAP;
use obc_formats::ride::{is_supported_version, VERSION_V2};
use obc_formats::track::RECORD_LEN as TRACK_RECORD_LEN;

// The ride-object codec/constants are owned by `obc-formats`; imported under the module-local
// `RIDE_*` / `ride_*` names this converter + reader read. Not re-exported — consumers reach the
// format authority via `obc_formats::ride`.
use obc_formats::ride::{
    checked_object_len as checked_ride_object_len, header_len as ride_header_len, CAD_NONE as RIDE_CAD_NONE,
    HEADER_LEN_V2 as RIDE_HEADER_LEN_V2, HR_NONE as RIDE_HR_NONE, POINT_LEN_V2 as RIDE_POINT_LEN_V2,
    PWR_NONE as RIDE_PWR_NONE, VERSION as RIDE_VERSION,
};

/// The ride totals the header carries, plus the wall-clock anchor that turns the log's
/// monotonic-millis timestamps into unix seconds. The app accumulates the totals live
/// (`Activity`); the anchor is "the unix time now, and the monotonic millis now" at Finish —
/// [`track_to_ride`] back-dates it by the first record's `t_ms` to date the ride's start, so
/// nothing needs to have been captured when the session began.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideStats {
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    /// Unix seconds that were true at [`anchor_ms`](RideStats::anchor_ms).
    pub unix_at_anchor: u32,
    /// The monotonic millis (the [`TrackPoint::t_ms`](crate::TrackPoint::t_ms) clock) at which
    /// [`unix_at_anchor`](RideStats::unix_at_anchor) was read.
    pub anchor_ms: u32,
    /// Per-ride BLE-sensor summary (epic #707, SE2's `Activity` accessors), written into the v2
    /// header. `None` (→ sentinel) when the ride saw no fresh sample of that quantity.
    pub avg_hr: Option<u8>,
    pub max_hr: Option<u8>,
    pub avg_cadence: Option<u8>,
    pub avg_power: Option<u16>,
    pub max_power: Option<u16>,
}

/// A stored ride object's header — what the BLE `rideList` entry serves without touching the
/// point records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideInfo {
    /// The stored object's version (1 or 2). Point-record readers ([`point_len`](obc_formats::ride::point_len)) and the
    /// point offset ([`ride_header_len`]) key off this.
    pub version: u8,
    /// Truncated to [`NAME_CAP`] on a char boundary for display; the on-disk name may be longer
    /// (`name_len` is a `u16`), and the length validation always uses the on-disk length.
    pub name: String<NAME_CAP>,
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    pub point_count: u32,
    /// Per-ride BLE-sensor summary (epic #707). Always `None` for a v1 object; for v2, decoded
    /// from the header's sentinel-marked fields.
    pub avg_hr: Option<u8>,
    pub max_hr: Option<u8>,
    pub avg_cadence: Option<u8>,
    pub avg_power: Option<u16>,
    pub max_power: Option<u16>,
}

impl RideInfo {
    /// Read + validate a stored ride object's header. Accepts **v1 and v2**: an unknown version
    /// (including the held-back `0` of a torn save) is [`Error::BadVersion`]; the fully-determined
    /// length for that version (`obc_formats::ride::object_len(version, name_len, point_count)`) must equal the
    /// source's — the torn-write guard. A v1 object decodes with every sensor field `None`. Point
    /// records are not touched.
    pub fn read(src: &dyn ByteSource) -> Result<RideInfo, Error> {
        let mut head = [0u8; 3];
        src.read_at(0, &mut head)?;
        let version = head[0];
        if !is_supported_version(version) {
            return Err(Error::BadVersion);
        }
        let name_len = u16::from_le_bytes([head[1], head[2]]) as usize;
        // The fixed tail (20 B for v1, 28 B for v2) sits right after the name; a source too short
        // is malformed. `point_count` is at tail offset 16 in both versions.
        let tail_len = ride_header_len(version) - 3;
        let mut tail = [0u8; RIDE_HEADER_LEN_V2 - 3];
        let tail = &mut tail[..tail_len];
        src.read_at(3 + name_len as u32, tail).map_err(|_| Error::BadOffset)?;
        let point_count = u32::from_le_bytes([tail[16], tail[17], tail[18], tail[19]]);
        let object_len = checked_ride_object_len(version, name_len, point_count).map_err(|_| Error::BadOffset)?;
        if src.len() != object_len {
            return Err(Error::BadOffset);
        }
        // v2 sensor summary (tail offset 20..28); v1 has no such bytes → all absent.
        let (avg_hr, max_hr, avg_cadence, avg_power, max_power) = if version >= VERSION_V2 {
            (
                opt_u8(tail[20], RIDE_HR_NONE),
                opt_u8(tail[21], RIDE_HR_NONE),
                opt_u8(tail[22], RIDE_CAD_NONE),
                // tail[23] is the reserved pad (0), skipped.
                opt_u16(u16::from_le_bytes([tail[24], tail[25]]), RIDE_PWR_NONE),
                opt_u16(u16::from_le_bytes([tail[26], tail[27]]), RIDE_PWR_NONE),
            )
        } else {
            (None, None, None, None, None)
        };

        let mut name = String::new();
        let show = name_len.min(NAME_CAP);
        if show > 0 {
            let mut buf = [0u8; NAME_CAP];
            src.read_at(3, &mut buf[..show])?;
            let _ = name.push_str(utf8_prefix(&buf[..show]));
        }
        Ok(RideInfo {
            version,
            name,
            start_time: u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]),
            distance_m: u32::from_le_bytes([tail[4], tail[5], tail[6], tail[7]]),
            moving_time_s: u32::from_le_bytes([tail[8], tail[9], tail[10], tail[11]]),
            avg_speed_cms: u16::from_le_bytes([tail[12], tail[13]]),
            climb_m: u16::from_le_bytes([tail[14], tail[15]]),
            point_count,
            avg_hr,
            max_hr,
            avg_cadence,
            avg_power,
            max_power,
        })
    }
}

/// A sentinel-marked `u8` field → `None` when it equals `sentinel`, else `Some`.
fn opt_u8(v: u8, sentinel: u8) -> Option<u8> {
    (v != sentinel).then_some(v)
}

/// A sentinel-marked `u16` field → `None` when it equals `sentinel`, else `Some`.
fn opt_u16(v: u16, sentinel: u16) -> Option<u16> {
    (v != sentinel).then_some(v)
}

/// The longest valid-UTF-8 prefix of `b` — a byte-capped name may have split a multi-byte char.
fn utf8_prefix(b: &[u8]) -> &str {
    match core::str::from_utf8(b) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&b[..e.valid_up_to()]).unwrap_or(""),
    }
}

/// Records converted per [`ByteSource`] read — one SD read fills a block rather than a record,
/// like the GPX converter's [`track_to_gpx`](crate::track_to_gpx) pass.
const BLOCK_RECORDS: usize = 64;

/// Convert a recorded `.obct` log into a **ride object v2** written to `sink` — the Finish-time
/// sibling of [`track_to_gpx`](crate::track_to_gpx), one streaming pass, no whole-ride buffer.
///
/// - `start_time` is the wall time of the **first record**: the anchor in `stats` back-dated by
///   the millis between that record and the anchor. An empty log dates itself at the anchor.
/// - `t_offset` is whole seconds since the first record (`t_ms` deltas are wrap-safe, like the
///   wall clock's).
/// - Coordinates translate µ° → 10⁻⁷ ° (× 10, no overflow: ±180 × 10⁷ fits `i32`) and swap into
///   the ride object's `lat, lon` order (track records are `lon, lat`).
/// - `ele` is carried verbatim — the log stamps every point (0 before the first baro sample), so
///   the device never writes [`ELE_NONE`](obc_formats::ride::ELE_NONE); the sentinel exists for other encoders (the app).
/// - The per-ride sensor summary ([`RideStats::avg_hr`] etc.) heads the v2 header; per-point
///   `hr`/`cad`/`pwr` carry 1:1 from the v2 track records, absent ↔ sentinel.
/// - Segment breaks don't exist in the ride object; a trailing partial record is ignored (the
///   log stays valid at any 20-byte boundary, same as the GPX pass).
/// - `name` is truncated to [`NAME_CAP`] bytes on a char boundary (the device's route-name cap).
///
/// The version byte is written as `0` and patched to [`RIDE_VERSION`] as the **final** write —
/// the commit point. A save interrupted anywhere earlier fails [`RideInfo::read`].
pub fn track_to_ride(
    src: &dyn ByteSource,
    name: &str,
    stats: &RideStats,
    sink: &mut dyn ByteSink,
) -> Result<(), Error> {
    let total = (src.len() as usize) / TRACK_RECORD_LEN;

    let mut end = name.len().min(NAME_CAP);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name[..end];

    // The first record's t_ms dates the ride and anchors every offset.
    let t0 = if total > 0 {
        let mut rec = [0u8; TRACK_RECORD_LEN];
        src.read_at(0, &mut rec)?;
        decode_record(&rec).t_ms
    } else {
        stats.anchor_ms
    };
    let start_time = stats.unix_at_anchor.wrapping_sub(stats.anchor_ms.wrapping_sub(t0) / 1000);

    // v2 header, version held back as 0 (patched below, after the body landed).
    let mut head = [0u8; RIDE_HEADER_LEN_V2 + NAME_CAP];
    head[1..3].copy_from_slice(&(name.len() as u16).to_le_bytes());
    head[3..3 + name.len()].copy_from_slice(name.as_bytes());
    let f = 3 + name.len();
    head[f..f + 4].copy_from_slice(&start_time.to_le_bytes());
    head[f + 4..f + 8].copy_from_slice(&stats.distance_m.to_le_bytes());
    head[f + 8..f + 12].copy_from_slice(&stats.moving_time_s.to_le_bytes());
    head[f + 12..f + 14].copy_from_slice(&stats.avg_speed_cms.to_le_bytes());
    head[f + 14..f + 16].copy_from_slice(&stats.climb_m.to_le_bytes());
    head[f + 16..f + 20].copy_from_slice(&(total as u32).to_le_bytes());
    // v2 sensor summary tail (sentinel for an absent quantity); byte 23 is a reserved 0 pad.
    head[f + 20] = stats.avg_hr.unwrap_or(RIDE_HR_NONE);
    head[f + 21] = stats.max_hr.unwrap_or(RIDE_HR_NONE);
    head[f + 22] = stats.avg_cadence.unwrap_or(RIDE_CAD_NONE);
    head[f + 23] = 0;
    head[f + 24..f + 26].copy_from_slice(&stats.avg_power.unwrap_or(RIDE_PWR_NONE).to_le_bytes());
    head[f + 26..f + 28].copy_from_slice(&stats.max_power.unwrap_or(RIDE_PWR_NONE).to_le_bytes());
    sink.write(&head[..f + 28])?;

    let mut buf = [0u8; BLOCK_RECORDS * TRACK_RECORD_LEN];
    let mut out = [0u8; BLOCK_RECORDS * RIDE_POINT_LEN_V2];
    let mut done = 0usize;
    while done < total {
        let n = (total - done).min(BLOCK_RECORDS);
        let bytes = &mut buf[..n * TRACK_RECORD_LEN];
        src.read_at((done * TRACK_RECORD_LEN) as u32, bytes)?;
        for i in 0..n {
            let mut rec = [0u8; TRACK_RECORD_LEN];
            rec.copy_from_slice(&bytes[i * TRACK_RECORD_LEN..(i + 1) * TRACK_RECORD_LEN]);
            let p = decode_record(&rec);
            let o = i * RIDE_POINT_LEN_V2;
            out[o..o + 4].copy_from_slice(&(p.t_ms.wrapping_sub(t0) / 1000).to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&p.lat.saturating_mul(10).to_le_bytes());
            out[o + 8..o + 12].copy_from_slice(&p.lon.saturating_mul(10).to_le_bytes());
            out[o + 12..o + 14].copy_from_slice(&p.ele.to_le_bytes());
            out[o + 14] = p.hr.unwrap_or(RIDE_HR_NONE);
            out[o + 15] = p.cadence.unwrap_or(RIDE_CAD_NONE);
            out[o + 16..o + 18].copy_from_slice(&p.power.unwrap_or(RIDE_PWR_NONE).to_le_bytes());
        }
        sink.write(&out[..n * RIDE_POINT_LEN_V2])?;
        done += n;
    }

    // The body is down — the one-write commit point.
    sink.patch_at(0, &[RIDE_VERSION])
}
