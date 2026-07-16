//! The **trip object** (v1) — the tiny metadata object that groups planned routes into one named
//! unit (`obc-ble-interface-spec.md` §7.7). A trip references route object ids in ride order; it
//! never contains route bytes, so membership edits never touch a route payload. The reference
//! firmware stores each trip as `TP{id}.OBT` beside the `RT{id}.OBR` route files.
//!
//! Layout (little-endian; pinned by `protocol-vectors/trip-v1.bin` against the Swift trip codec):
//!
//! ```text
//! Header (56 bytes):
//!   version      u8   = 1                 [0]
//!   reserved     u8   = 0                 [1]
//!   stage_count  u16                      [2..4]
//!   name_len     u8   ≤ 48                [4]
//!   name         char[48]  UTF-8, zero-padded   [5..53]
//!   reserved     u8[3]  = 0               [53..56]
//! Stages (2 bytes × stage_count):
//!   stage_id     u16   route object id, ride order   [56..]
//! ```
//!
//! The object length is fully determined by its header: `56 + 2·stage_count` bytes — a decoder
//! rejects a payload whose length disagrees ([`Error::BadOffset`]), which is also this file's
//! torn-write guard (a cut-short write leaves a shorter file). The reader is version-gated like the
//! OBCR reader ([`Error::BadVersion`] on anything but v1).
//!
//! Two reads, mirroring [`RouteSummary`](crate::RouteSummary) / [`TripMeta`]:
//! - [`TripSummary::read`] — the header alone (name + true `stage_count`), for a catalog scan.
//! - [`TripMeta::read`] — header **and** stage ids, windowed to [`MAX_TRIP_STAGES`] the way
//!   [`RouteReader::load_waypoints`](crate::RouteReader::load_waypoints) windows a longer waypoint
//!   section: a phone-side encoder isn't bound by the device's resident cap, so a trip past it reads
//!   its first [`MAX_TRIP_STAGES`] ids (`truncated` flags the drop) rather than overflowing.
//!
//! [`write_trip`] is the streaming writer: the fixed header then the stage ids, one pass, no
//! placeholder-header dance (the object is small and its length is header-determined).

use heapless::{String, Vec};

use obc_formats::io::{rd_u16, ByteSink, ByteSource, Error};
use obc_formats::obcr::NAME_CAP;

/// The trip-object version [`write_trip`] writes (spec §7.7). [`TripMeta::read`] / [`TripSummary::read`]
/// accept only this version.
pub const TRIP_VERSION: u8 = 1;
/// The fixed trip-object header length (spec §7.7) — the stage ids follow immediately.
pub const TRIP_HEADER_LEN: usize = 56;

/// The device's resident cap on a trip's stages — the [`TripMeta`] stage table's `heapless::Vec`
/// bound. The wire *format* allows up to `u16::MAX` stages (a phone encoder isn't bound by this), so
/// [`TripMeta::read`] windows + truncates a longer trip rather than overflowing, exactly as
/// [`RouteReader::load_waypoints`](crate::RouteReader::load_waypoints) does for waypoints. A trip
/// can reference at most one route per stage, so this stays comfortably past the route catalog cap.
#[cfg(not(feature = "nrf-mem"))]
pub const MAX_TRIP_STAGES: usize = 32;
#[cfg(feature = "nrf-mem")]
pub const MAX_TRIP_STAGES: usize = 16;

/// The lightweight trip description — readable from the header alone (no stage table), so a catalog
/// scan is one small read per file. Mirrors [`RouteSummary`](crate::RouteSummary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripSummary {
    pub name: String<NAME_CAP>,
    /// The stage count **as stored** — the true figure from the header, even when it exceeds
    /// [`MAX_TRIP_STAGES`] (what [`TripMeta::read`] would window to).
    pub stage_count: u16,
}

impl TripSummary {
    /// Read + validate a stored trip object's header into a summary — cheap enough to call per file
    /// when building the trip catalog. Version-gated ([`Error::BadVersion`]) and length-checked
    /// against the header-determined size (`56 + 2·stage_count`), the torn-write guard.
    pub fn read(src: &dyn ByteSource) -> Result<TripSummary, Error> {
        let h = read_header(src)?;
        Ok(TripSummary { name: h.name, stage_count: h.stage_count })
    }
}

/// A trip's full metadata: its name and the ordered route object ids it references. The stage table
/// is windowed to [`MAX_TRIP_STAGES`]; `truncated` flags a trip whose stored `stage_count` exceeded
/// the cap (the summary keeps the true count).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripMeta {
    pub name: String<NAME_CAP>,
    /// The route object ids this trip references, ride order — dangling ids (a member route deleted
    /// individually) are carried verbatim; validation is the app's job, not the codec's.
    pub stage_ids: Vec<u16, MAX_TRIP_STAGES>,
    /// The stored `stage_count` exceeded [`MAX_TRIP_STAGES`], so `stage_ids` holds only the first
    /// [`MAX_TRIP_STAGES`] of them.
    pub truncated: bool,
}

impl TripMeta {
    /// Read a stored trip object: the header (same validation as [`TripSummary::read`]) plus the
    /// stage ids, windowed to [`MAX_TRIP_STAGES`]. A trip whose stored `stage_count` exceeds the cap
    /// reads its first [`MAX_TRIP_STAGES`] ids with `truncated = true`.
    pub fn read(src: &dyn ByteSource) -> Result<TripMeta, Error> {
        let h = read_header(src)?;
        let want = h.stage_count as usize;
        let take = want.min(MAX_TRIP_STAGES);
        let mut stage_ids = Vec::new();
        // The stage table sits right after the fixed header; one small read pulls the whole windowed
        // slice (≤ 2·MAX_TRIP_STAGES bytes). The length check in `read_header` already proved every
        // stored stage is present, so this read cannot run short.
        let mut buf = [0u8; 2 * MAX_TRIP_STAGES];
        let bytes = &mut buf[..take * 2];
        if take > 0 {
            src.read_at(TRIP_HEADER_LEN as u32, bytes)?;
        }
        for k in 0..take {
            // Infallible: the loop count equals the pushed count, both ≤ MAX_TRIP_STAGES.
            let _ = stage_ids.push(rd_u16(bytes, k * 2));
        }
        Ok(TripMeta { name: h.name, stage_ids, truncated: want > take })
    }
}

/// Parsed trip-object header fields, shared by [`TripSummary::read`] and [`TripMeta::read`].
struct Header {
    stage_count: u16,
    name: String<NAME_CAP>,
}

fn read_header(src: &dyn ByteSource) -> Result<Header, Error> {
    let mut h = [0u8; TRIP_HEADER_LEN];
    src.read_at(0, &mut h).map_err(|_| Error::BadOffset)?;
    if h[0] != TRIP_VERSION {
        return Err(Error::BadVersion);
    }
    let stage_count = rd_u16(&h, 2);
    // Length is fully determined by the header — a payload of any other size is torn or malformed.
    if src.len() != trip_object_len(stage_count) {
        return Err(Error::BadOffset);
    }
    let name_len = (h[4] as usize).min(NAME_CAP);
    let mut name = String::new();
    let _ = name.push_str(utf8_prefix(&h[5..5 + name_len]));
    Ok(Header { stage_count, name })
}

/// The whole encoded object's size for a given stage count: `56 + 2·stage_count`.
pub const fn trip_object_len(stage_count: u16) -> u32 {
    TRIP_HEADER_LEN as u32 + 2 * stage_count as u32
}

/// The longest valid-UTF-8 prefix of `b` — a byte-capped name may have split a multi-byte char.
fn utf8_prefix(b: &[u8]) -> &str {
    match core::str::from_utf8(b) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&b[..e.valid_up_to()]).unwrap_or(""),
    }
}

/// Write a **trip object v1** (spec §7.7) to `sink`: the fixed 56-byte header then the stage ids,
/// one streaming pass. `stages` is truncated to `u16::MAX` (the format's own cap) and `name` to
/// [`NAME_CAP`] bytes on a char boundary (the device's route-name cap). No placeholder-header dance
/// — the length is header-determined, so a torn write simply fails [`TripSummary::read`]'s length
/// check rather than masquerading as a valid trip.
pub fn write_trip(name: &str, stages: &[u16], sink: &mut dyn ByteSink) -> Result<(), Error> {
    let mut end = name.len().min(NAME_CAP);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name[..end];
    let stage_count = stages.len().min(u16::MAX as usize);

    let mut head = [0u8; TRIP_HEADER_LEN];
    head[0] = TRIP_VERSION;
    // head[1] reserved = 0
    head[2..4].copy_from_slice(&(stage_count as u16).to_le_bytes());
    head[4] = name.len() as u8;
    head[5..5 + name.len()].copy_from_slice(name.as_bytes());
    // head[53..56] reserved = 0
    sink.write(&head)?;

    // Stream the stage ids in blocks so the whole table never has to be resident to write.
    const BLOCK: usize = 64;
    let mut buf = [0u8; BLOCK * 2];
    let mut done = 0usize;
    while done < stage_count {
        let n = (stage_count - done).min(BLOCK);
        for i in 0..n {
            buf[i * 2..i * 2 + 2].copy_from_slice(&stages[done + i].to_le_bytes());
        }
        sink.write(&buf[..n * 2])?;
        done += n;
    }
    Ok(())
}
