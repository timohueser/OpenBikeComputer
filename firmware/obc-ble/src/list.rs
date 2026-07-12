//! The `routeList` / `rideList` object codecs — the CoC-downloaded catalogs that outgrow the
//! 512-byte ATT attribute cap. Shared shape: a 6-byte [`ListHeader`] + fixed entries, so entry `k`
//! sits at `6 + entry_len·k` — O(1) indexing, no string scanning. In protocol v2 the two list types
//! **differ in entry length** (`routeList` 76 bytes with its content CRC, `rideList` 72), so the
//! entry size is per-type ([`RouteListEntry::ENTRY_LEN`] / [`RideListEntry::ENTRY_LEN`]) and travels
//! on the wire in the header's `entry_len` byte — there is no single shared entry-length constant.

use crate::descriptor::DescriptorError;

/// `routeList` entry size (protocol v2): 72 v1 bytes + the trailing content `crc32`.
const ROUTE_ENTRY_LEN: usize = 76;
/// `rideList` entry size — unchanged from v1.
const RIDE_ENTRY_LEN: usize = 72;

/// The smallest entry length any list type uses (`rideList` at [`RideListEntry::ENTRY_LEN`]) — the
/// header decoder's floor sanity-check. A future entry growth appends fields and bumps the header's
/// `entry_len`; an old reader steps by the announced length and decodes the prefix it knows.
pub const MIN_LIST_ENTRY_LEN: usize = RIDE_ENTRY_LEN;

/// The 6-byte header both list objects share (protocol v2, epic #632 item 7).
///
/// ```text
///   version    u8   = 2
///   entry_len  u8   the entry size (76 routeList · 72 rideList); readers step by it, not a constant
///   count      u16  entries actually in this object (after the MAX_RIDES / MAX_ROUTES cap)
///   total      u16  full catalog size BEFORE the cap — truncated iff total > count
/// ```
///
/// `total` makes a >`MAX_RIDES` (or >`MAX_ROUTES`) truncation visible on the wire: the device
/// dropped `total - count` entries in FAT order, and the app surfaces a one-line warning instead of
/// silently answering "up to date".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListHeader {
    pub count: u16,
    /// Full catalog size before the `MAX_RIDES`/`MAX_ROUTES` cap. Equal to `count` when nothing was
    /// dropped; greater when the object is truncated.
    pub total: u16,
}

impl ListHeader {
    pub const ENCODED_LEN: usize = 6;
    pub const VERSION: u8 = 2;

    /// Encode the header. `entry_len` is the per-type entry size the entries that follow use
    /// ([`RouteListEntry::ENTRY_LEN`] / [`RideListEntry::ENTRY_LEN`]).
    pub fn encode(&self, entry_len: u8) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0] = Self::VERSION;
        b[1] = entry_len;
        b[2..4].copy_from_slice(&self.count.to_le_bytes());
        b[4..6].copy_from_slice(&self.total.to_le_bytes());
        b
    }

    /// Decode a list header, rejecting an unknown version or an `entry_len` below the smallest a list
    /// entry can be ([`MIN_LIST_ENTRY_LEN`]). The returned `entry_len` may *exceed* the type's own —
    /// forward compatibility: step by it, decode the prefix you know.
    pub fn decode(data: &[u8]) -> Result<(Self, usize), DescriptorError> {
        if data.len() < Self::ENCODED_LEN {
            return Err(DescriptorError::Truncated);
        }
        if data[0] != Self::VERSION {
            return Err(DescriptorError::UnknownStatus(data[0]));
        }
        let entry_len = data[1] as usize;
        if entry_len < MIN_LIST_ENTRY_LEN {
            return Err(DescriptorError::UnknownStatus(data[1]));
        }
        Ok((
            Self { count: u16::from_le_bytes([data[2], data[3]]), total: u16::from_le_bytes([data[4], data[5]]) },
            entry_len,
        ))
    }

    /// Whether this list is truncated — the device dropped `total - count` entries at the cap.
    pub const fn is_truncated(&self) -> bool {
        self.total > self.count
    }

    /// Byte offset of entry `k`, given the per-type `entry_len`.
    pub const fn entry_offset(k: usize, entry_len: usize) -> usize {
        Self::ENCODED_LEN + k * entry_len
    }

    /// The whole encoded object's size for `count` entries of `entry_len` bytes.
    pub const fn object_len(count: usize, entry_len: usize) -> usize {
        Self::ENCODED_LEN + count * entry_len
    }

    /// The bounds-checked slot for entry `k`, given the header's announced `entry_len`. `None` when
    /// the object is shorter than `count` claims — `decode` reads only the 6-byte header and can't
    /// police `count`; this guards the walk from slicing past the buffer. Pass the slice straight to
    /// `RouteListEntry`/`RideListEntry::decode`.
    pub fn entry_slice(data: &[u8], k: usize, entry_len: usize) -> Option<&[u8]> {
        let off = Self::ENCODED_LEN + k * entry_len;
        data.get(off..off.checked_add(entry_len)?)
    }
}

/// One `routeList` entry — from the stored OBCR header. **76 bytes in protocol v2** (up from 72):
/// the trailing `crc32` is the whole-object CRC-32 of the stored OBCR bytes, letting the app verify
/// *what* a linked id points at (identity-verified badges) and adopt an identical unlinked copy.
///
/// ```text
///   object_id       u16
///   reserved        u16  = 0
///   byte_len        u32  stored file size (upload/detail sizing)
///   distance_m      u32
///   ascent_m        u32
///   point_count     u32
///   waypoint_count  u16
///   name_len        u8   ≤ 48
///   name            char[48]  UTF-8, zero-padded
///   reserved        u8   = 0
///   crc32           u32  whole-object CRC-32 of the stored OBCR bytes · 0 = unknown
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteListEntry<'a> {
    pub object_id: u16,
    pub byte_len: u32,
    pub distance_m: u32,
    pub ascent_m: u32,
    pub point_count: u32,
    pub waypoint_count: u16,
    /// UTF-8, ≤ [`RouteListEntry::MAX_NAME`] bytes (the OBCR route-name cap); over-long input is
    /// truncated at encode.
    pub name: &'a [u8],
    /// Whole-object CRC-32/IEEE of the stored OBCR bytes — the content fingerprint the app matches
    /// against its `uploadedCRC32` record. `0` = unknown (e.g. a side-loaded file not yet
    /// fingerprinted; the device fills it lazily at first list build).
    pub crc32: u32,
}

impl<'a> RouteListEntry<'a> {
    /// The name cap (matches the OBCR route-name field).
    pub const MAX_NAME: usize = 48;
    /// This entry's on-wire size (protocol v2). Carried in the list header's `entry_len`.
    pub const ENTRY_LEN: usize = ROUTE_ENTRY_LEN;
    /// Sentinel for an unknown content CRC (side-loaded file not yet fingerprinted).
    pub const CRC_UNKNOWN: u32 = 0;

    pub fn encode(&self) -> [u8; ROUTE_ENTRY_LEN] {
        let mut b = [0u8; ROUTE_ENTRY_LEN];
        b[0..2].copy_from_slice(&self.object_id.to_le_bytes());
        // b[2..4] reserved = 0.
        b[4..8].copy_from_slice(&self.byte_len.to_le_bytes());
        b[8..12].copy_from_slice(&self.distance_m.to_le_bytes());
        b[12..16].copy_from_slice(&self.ascent_m.to_le_bytes());
        b[16..20].copy_from_slice(&self.point_count.to_le_bytes());
        b[20..22].copy_from_slice(&self.waypoint_count.to_le_bytes());
        let n = self.name.len().min(Self::MAX_NAME);
        b[22] = n as u8;
        b[23..23 + n].copy_from_slice(&self.name[..n]);
        // b[23 + n .. 71] zero padding; b[71] reserved = 0.
        b[72..76].copy_from_slice(&self.crc32.to_le_bytes());
        b
    }

    /// Decode one entry from the first [`ENTRY_LEN`](Self::ENTRY_LEN) bytes of a slot — a longer
    /// future entry's tail is ignored.
    pub fn decode(data: &'a [u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::ENTRY_LEN {
            return Err(DescriptorError::Truncated);
        }
        let name_len = (data[22] as usize).min(Self::MAX_NAME);
        Ok(Self {
            object_id: u16::from_le_bytes([data[0], data[1]]),
            byte_len: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            distance_m: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            ascent_m: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
            point_count: u32::from_le_bytes([data[16], data[17], data[18], data[19]]),
            waypoint_count: u16::from_le_bytes([data[20], data[21]]),
            name: &data[23..23 + name_len],
            crc32: u32::from_le_bytes([data[72], data[73], data[74], data[75]]),
        })
    }
}

/// One `rideList` entry — from the stored ride-object header.
///
/// ```text
///   object_id      u16
///   reserved       u16  = 0
///   byte_len       u32  stored file size
///   start_time     u32  unix seconds
///   distance_m     u32
///   moving_time_s  u32
///   avg_speed_cms  u16
///   climb_m        u16
///   name_len       u8   ≤ 47
///   name           char[47]  UTF-8, zero-padded
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RideListEntry<'a> {
    pub object_id: u16,
    pub byte_len: u32,
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    /// UTF-8, ≤ [`RideListEntry::MAX_NAME`] bytes; over-long input is truncated at encode.
    pub name: &'a [u8],
}

impl<'a> RideListEntry<'a> {
    /// One byte shorter than the route's — the fixed fields take one more.
    pub const MAX_NAME: usize = 47;
    /// This entry's on-wire size. Unchanged from v1 (72 bytes); `routeList` grew, `rideList` did
    /// not, which is why the entry length is now per-type. Carried in the header's `entry_len`.
    pub const ENTRY_LEN: usize = RIDE_ENTRY_LEN;

    pub fn encode(&self) -> [u8; RIDE_ENTRY_LEN] {
        let mut b = [0u8; RIDE_ENTRY_LEN];
        b[0..2].copy_from_slice(&self.object_id.to_le_bytes());
        // b[2..4] reserved = 0.
        b[4..8].copy_from_slice(&self.byte_len.to_le_bytes());
        b[8..12].copy_from_slice(&self.start_time.to_le_bytes());
        b[12..16].copy_from_slice(&self.distance_m.to_le_bytes());
        b[16..20].copy_from_slice(&self.moving_time_s.to_le_bytes());
        b[20..22].copy_from_slice(&self.avg_speed_cms.to_le_bytes());
        b[22..24].copy_from_slice(&self.climb_m.to_le_bytes());
        let n = self.name.len().min(Self::MAX_NAME);
        b[24] = n as u8;
        b[25..25 + n].copy_from_slice(&self.name[..n]);
        b
    }

    /// Decode one entry (prefix of an entry slot, like [`RouteListEntry::decode`]).
    pub fn decode(data: &'a [u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::ENTRY_LEN {
            return Err(DescriptorError::Truncated);
        }
        let name_len = (data[24] as usize).min(Self::MAX_NAME);
        Ok(Self {
            object_id: u16::from_le_bytes([data[0], data[1]]),
            byte_len: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            start_time: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            distance_m: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
            moving_time_s: u32::from_le_bytes([data[16], data[17], data[18], data[19]]),
            avg_speed_cms: u16::from_le_bytes([data[20], data[21]]),
            climb_m: u16::from_le_bytes([data[22], data[23]]),
            name: &data[25..25 + name_len],
        })
    }
}
