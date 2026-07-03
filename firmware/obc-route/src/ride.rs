//! The **ride object v1** — the compact tracked-ride layout a ride crosses the BLE link as
//! (`obc-ble-interface-spec.md` §7.2) *and* the durable per-ride file the device stores at
//! Finish (`/tracks/RD{id}.ORD`, issue #275). One layout, one truth: the stored file **is** the
//! wire object, so a BLE ride download is a verbatim byte stream with no encode on the transfer
//! path — exactly the route-detail discipline (S0 §1 principle 3, "objects are files the device
//! already speaks").
//!
//! Layout (little-endian; pinned by `protocol-vectors/ride-v1.bin` against the Swift
//! `RideObjectCodec`):
//!
//! ```text
//! Header (23 bytes + name):
//!   version      u8   = 1
//!   name_len     u16  · name UTF-8 (name_len bytes follow immediately)
//!   start_time   u32  unix seconds
//!   distance     u32  meters
//!   moving_time  u32  seconds
//!   avg_speed    u16  cm/s
//!   climb        u16  meters
//!   point_count  u32
//! Point record (14 bytes × point_count):
//!   t_offset  u32  seconds since start_time
//!   lat       i32  degrees × 1e7
//!   lon       i32  degrees × 1e7
//!   ele       i16  meters · i16::MIN = no elevation
//! ```
//!
//! The byte length is fully determined: `23 + name_len + 14 × point_count` — a decoder must
//! reject a payload whose length disagrees (spec §7.2), which is also this file's power-cut
//! guard (a torn write leaves a shorter file).
//!
//! [`track_to_ride`] is the Finish-time converter: one **streaming** pass over the recorded
//! `.obct` log (the same fixed-record array [`track_to_gpx`](crate::track_to_gpx) reads), no
//! resident whole-ride buffer. Note the coordinate translation: track records store integer
//! **microdegrees** in `lon, lat` order; ride points store **degrees × 1e7** in `lat, lon`
//! order. Like the OBCR writer, the converter holds the version byte back as `0` and patches it
//! in as the final write, so an interrupted save is rejected by every reader
//! ([`Error::BadVersion`]) instead of masquerading as a ride.

use heapless::String;

use crate::byte_io::{ByteSink, ByteSource, Error};
use crate::reader::NAME_CAP;
use crate::track::{decode_record, TRACK_RECORD_LEN};

/// The ride-object version this module writes (spec §7.2).
pub const RIDE_VERSION: u8 = 1;
/// Fixed header bytes (the name's `name_len` bytes ride between `name_len` and `start_time`).
pub const RIDE_HEADER_LEN: usize = 23;
/// One encoded point record.
pub const RIDE_POINT_LEN: usize = 14;
/// The point `ele` sentinel for "no elevation recorded".
pub const RIDE_ELE_NONE: i16 = i16::MIN;

/// The whole encoded object's size for a given name and point count.
pub const fn ride_object_len(name_len: usize, point_count: u32) -> u32 {
    (RIDE_HEADER_LEN + name_len) as u32 + RIDE_POINT_LEN as u32 * point_count
}

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
}

/// A stored ride object's header — what the BLE `rideList` entry serves (spec §7.4) without
/// touching the point records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideInfo {
    /// Truncated to [`NAME_CAP`] on a char boundary for display; the on-disk name may be longer
    /// (`name_len` is a `u16`), and the length validation always uses the on-disk length.
    pub name: String<NAME_CAP>,
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    pub point_count: u32,
}

impl RideInfo {
    /// Read + validate a stored ride object's header: the version byte (a held-back `0` — an
    /// interrupted save — is [`Error::BadVersion`] like any unknown version) and the
    /// fully-determined length (`23 + name_len + 14 × point_count` must equal the source's —
    /// spec §7.2's "a decoder must reject a payload whose length disagrees", and the torn-write
    /// guard). Point records are not touched.
    pub fn read(src: &dyn ByteSource) -> Result<RideInfo, Error> {
        let mut head = [0u8; 3];
        src.read_at(0, &mut head)?;
        if head[0] != RIDE_VERSION {
            return Err(Error::BadVersion);
        }
        let name_len = u16::from_le_bytes([head[1], head[2]]) as usize;
        // The 20 fixed tail bytes sit right after the name; a source too short is malformed.
        let mut tail = [0u8; 20];
        src.read_at(3 + name_len as u32, &mut tail).map_err(|_| Error::BadOffset)?;
        let point_count = u32::from_le_bytes([tail[16], tail[17], tail[18], tail[19]]);
        if src.len() != ride_object_len(name_len, point_count) {
            return Err(Error::BadOffset);
        }

        let mut name = String::new();
        let show = name_len.min(NAME_CAP);
        if show > 0 {
            let mut buf = [0u8; NAME_CAP];
            src.read_at(3, &mut buf[..show])?;
            let _ = name.push_str(utf8_prefix(&buf[..show]));
        }
        Ok(RideInfo {
            name,
            start_time: u32::from_le_bytes([tail[0], tail[1], tail[2], tail[3]]),
            distance_m: u32::from_le_bytes([tail[4], tail[5], tail[6], tail[7]]),
            moving_time_s: u32::from_le_bytes([tail[8], tail[9], tail[10], tail[11]]),
            avg_speed_cms: u16::from_le_bytes([tail[12], tail[13]]),
            climb_m: u16::from_le_bytes([tail[14], tail[15]]),
            point_count,
        })
    }
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

/// Convert a recorded `.obct` log (`src`, a flat array of [`TRACK_RECORD_LEN`]-byte records)
/// into a ride object v1 written to `sink` — the Finish-time sibling of
/// [`track_to_gpx`](crate::track_to_gpx), one streaming pass, no whole-ride buffer.
///
/// - `start_time` is the wall time of the **first record**: the anchor in `stats` back-dated by
///   the millis between that record and the anchor. An empty log dates itself at the anchor.
/// - `t_offset` is whole seconds since the first record (`t_ms` deltas are wrap-safe, like the
///   wall clock's).
/// - Coordinates translate µ° → 10⁻⁷ ° (× 10, no overflow: ±180 × 10⁷ fits `i32`) and swap into
///   the ride object's `lat, lon` order (track records are `lon, lat`).
/// - `ele` is carried verbatim — the log stamps every point (0 before the first baro sample), so
///   the device never writes [`RIDE_ELE_NONE`]; the sentinel exists for other encoders (the app).
/// - Segment breaks don't exist in the ride object; a trailing partial record is ignored (the
///   log stays valid at any 16-byte boundary, same as the GPX pass).
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

    // Header, version held back as 0 (patched below, after the body landed).
    let mut head = [0u8; RIDE_HEADER_LEN + NAME_CAP];
    head[1..3].copy_from_slice(&(name.len() as u16).to_le_bytes());
    head[3..3 + name.len()].copy_from_slice(name.as_bytes());
    let f = 3 + name.len();
    head[f..f + 4].copy_from_slice(&start_time.to_le_bytes());
    head[f + 4..f + 8].copy_from_slice(&stats.distance_m.to_le_bytes());
    head[f + 8..f + 12].copy_from_slice(&stats.moving_time_s.to_le_bytes());
    head[f + 12..f + 14].copy_from_slice(&stats.avg_speed_cms.to_le_bytes());
    head[f + 14..f + 16].copy_from_slice(&stats.climb_m.to_le_bytes());
    head[f + 16..f + 20].copy_from_slice(&(total as u32).to_le_bytes());
    sink.write(&head[..f + 20])?;

    let mut buf = [0u8; BLOCK_RECORDS * TRACK_RECORD_LEN];
    let mut out = [0u8; BLOCK_RECORDS * RIDE_POINT_LEN];
    let mut done = 0usize;
    while done < total {
        let n = (total - done).min(BLOCK_RECORDS);
        let bytes = &mut buf[..n * TRACK_RECORD_LEN];
        src.read_at((done * TRACK_RECORD_LEN) as u32, bytes)?;
        for i in 0..n {
            let mut rec = [0u8; TRACK_RECORD_LEN];
            rec.copy_from_slice(&bytes[i * TRACK_RECORD_LEN..(i + 1) * TRACK_RECORD_LEN]);
            let p = decode_record(&rec);
            let o = i * RIDE_POINT_LEN;
            out[o..o + 4].copy_from_slice(&(p.t_ms.wrapping_sub(t0) / 1000).to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&p.lat.saturating_mul(10).to_le_bytes());
            out[o + 8..o + 12].copy_from_slice(&p.lon.saturating_mul(10).to_le_bytes());
            out[o + 12..o + 14].copy_from_slice(&p.ele.to_le_bytes());
        }
        sink.write(&out[..n * RIDE_POINT_LEN])?;
        done += n;
    }

    // The body is down — the one-write commit point.
    sink.patch_at(0, &[RIDE_VERSION])
}
