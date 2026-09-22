//! The trip object: the phone-authored metadata object that describes a trip's days. A trip
//! references one route object per day, in ride order, and holds no route bytes. The layout is in
//! `obc-ble-interface-spec.md` §7.7 and pinned by `specs/vectors/trip-v3.bin`.
//!
//! The object length is fully determined by the header: `64 + 16·day_count` bytes. A decoder
//! rejects a payload of any other length with [`Error::BadOffset`], which is also the torn-write
//! guard, because a cut-short write leaves a shorter file.

use heapless::{String, Vec};

use obc_formats::io::{rd_u16, rd_u32, ByteSink, ByteSource, Error};
use obc_formats::obcr::NAME_CAP;

/// The trip-object version [`write_trip`] writes. The readers accept only this version.
pub const TRIP_VERSION: u8 = 3;
/// The fixed trip-object header length. The day records follow immediately.
pub const TRIP_HEADER_LEN: usize = 64;
/// The length of one day record.
pub const TRIP_DAY_LEN: usize = 16;

/// The device's resident cap on a trip's days. The wire format allows up to `u16::MAX`, and a
/// phone encoder is not bound by this cap, so [`TripMeta::read`] windows a longer trip instead of
/// overflowing.
pub const MAX_TRIP_DAYS: usize = 32;

/// One day of a trip: its route and where that route runs on the trip's main line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripDay {
    /// The day route's object id. A dangling id is carried verbatim; validation is the app's job.
    pub route: u64,
    /// Metres along the day route where it joins the main line.
    pub join_m: u32,
    /// Metres along the day route where it leaves the main line. A value at or past the route's
    /// end means the day ends on the line.
    pub leave_m: u32,
}

impl TripDay {
    /// A day that starts and ends on the main line.
    pub const fn whole(route: u64) -> TripDay {
        TripDay { route, join_m: 0, leave_m: u32::MAX }
    }

    fn decode(b: &[u8]) -> TripDay {
        TripDay {
            route: u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]),
            join_m: rd_u32(b, 8),
            leave_m: rd_u32(b, 12),
        }
    }

    fn encode(self, b: &mut [u8]) {
        b[0..8].copy_from_slice(&self.route.to_le_bytes());
        b[8..12].copy_from_slice(&self.join_m.to_le_bytes());
        b[12..16].copy_from_slice(&self.leave_m.to_le_bytes());
    }
}

/// The lightweight trip description, readable from the header alone, so a catalog scan is one
/// small read per file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripSummary {
    pub key: u64,
    pub name: String<NAME_CAP>,
    /// Days since 1970-01-01; 0 = no start date.
    pub start_date: u16,
    /// The day count as stored, even when it exceeds [`MAX_TRIP_DAYS`].
    pub day_count: u16,
}

impl TripSummary {
    /// Read and validate a stored trip object's header. Cheap enough to call per file when
    /// building the trip catalog.
    pub fn read(src: &dyn ByteSource) -> Result<TripSummary, Error> {
        read_header(src)
    }
}

/// A trip's resident metadata: its header fields and its day route ids in ride order. The
/// per-day line offsets stay on the medium; [`read_trip_day`] reads one day when it is needed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TripMeta {
    pub key: u64,
    pub name: String<NAME_CAP>,
    /// Days since 1970-01-01; 0 = no start date.
    pub start_date: u16,
    /// The day route ids in ride order. A dangling id is carried verbatim.
    pub day_routes: Vec<u64, MAX_TRIP_DAYS>,
    /// The stored `day_count` exceeded [`MAX_TRIP_DAYS`], so `day_routes` holds only the first
    /// [`MAX_TRIP_DAYS`] days.
    pub truncated: bool,
}

impl TripMeta {
    /// Read a stored trip object: the header plus the day route ids, windowed to
    /// [`MAX_TRIP_DAYS`].
    pub fn read(src: &dyn ByteSource) -> Result<TripMeta, Error> {
        let h = read_header(src)?;
        let take = (h.day_count as usize).min(MAX_TRIP_DAYS);
        // `read_header` already proved every stored day is present, so this read cannot run short.
        let mut buf = [0u8; TRIP_DAY_LEN * MAX_TRIP_DAYS];
        let bytes = &mut buf[..take * TRIP_DAY_LEN];
        if take > 0 {
            src.read_at(day_offset(0), bytes)?;
        }
        let day_routes = bytes.chunks_exact(TRIP_DAY_LEN).map(|b| TripDay::decode(b).route).collect();
        Ok(TripMeta {
            key: h.key,
            name: h.name,
            start_date: h.start_date,
            day_routes,
            truncated: h.day_count as usize > take,
        })
    }
}

/// Read day `k` of a stored trip object, validating the header first.
pub fn read_trip_day(src: &dyn ByteSource, k: u16) -> Result<TripDay, Error> {
    if k >= read_header(src)?.day_count {
        return Err(Error::BadOffset);
    }
    let mut day = [0u8; TRIP_DAY_LEN];
    src.read_at(day_offset(k as usize), &mut day)?;
    Ok(TripDay::decode(&day))
}

const fn day_offset(k: usize) -> u64 {
    (TRIP_HEADER_LEN + k * TRIP_DAY_LEN) as u64
}

fn read_header(src: &dyn ByteSource) -> Result<TripSummary, Error> {
    let mut h = [0u8; TRIP_HEADER_LEN];
    src.read_at(0, &mut h).map_err(|_| Error::BadOffset)?;
    if h[0] != TRIP_VERSION {
        return Err(Error::BadVersion);
    }
    let day_count = rd_u16(&h, 2);
    // Length is fully determined by the header, so any other size is torn or malformed.
    if src.len() != trip_object_len(day_count) {
        return Err(Error::BadOffset);
    }
    let name_len = (h[4] as usize).min(NAME_CAP);
    let mut name = String::new();
    let _ = name.push_str(utf8_prefix(&h[5..5 + name_len]));
    let key = u64::from_le_bytes([h[56], h[57], h[58], h[59], h[60], h[61], h[62], h[63]]);
    // Progress and ride records use key 0 for "no trip".
    if key == 0 {
        return Err(Error::BadOffset);
    }
    Ok(TripSummary { key, name, start_date: rd_u16(&h, 54), day_count })
}

/// The whole encoded object's size for a given day count: `64 + 16·day_count`.
pub const fn trip_object_len(day_count: u16) -> u64 {
    TRIP_HEADER_LEN as u64 + TRIP_DAY_LEN as u64 * day_count as u64
}

/// The longest valid-UTF-8 prefix of `b`. A byte-capped name can split a multi-byte char.
fn utf8_prefix(b: &[u8]) -> &str {
    match core::str::from_utf8(b) {
        Ok(s) => s,
        Err(e) => core::str::from_utf8(&b[..e.valid_up_to()]).unwrap_or(""),
    }
}

/// Write a trip object to `sink` in one streaming pass. `days` is truncated to `u16::MAX` and
/// `name` to [`NAME_CAP`] bytes on a char boundary.
pub fn write_trip(
    key: u64,
    name: &str,
    start_date: u16,
    days: &[TripDay],
    sink: &mut dyn ByteSink,
) -> Result<(), Error> {
    let mut end = name.len().min(NAME_CAP);
    while end > 0 && !name.is_char_boundary(end) {
        end -= 1;
    }
    let name = &name[..end];
    let days = &days[..days.len().min(u16::MAX as usize)];

    let mut head = [0u8; TRIP_HEADER_LEN];
    head[0] = TRIP_VERSION;
    head[2..4].copy_from_slice(&(days.len() as u16).to_le_bytes());
    head[4] = name.len() as u8;
    head[5..5 + name.len()].copy_from_slice(name.as_bytes());
    head[54..56].copy_from_slice(&start_date.to_le_bytes());
    head[56..64].copy_from_slice(&key.to_le_bytes());
    sink.write(&head)?;

    // Stream the days in blocks so the whole table is never resident.
    let mut buf = [0u8; TRIP_DAY_LEN * 16];
    for block in days.chunks(16) {
        for (day, out) in block.iter().zip(buf.chunks_exact_mut(TRIP_DAY_LEN)) {
            day.encode(out);
        }
        sink.write(&buf[..block.len() * TRIP_DAY_LEN])?;
    }
    Ok(())
}
