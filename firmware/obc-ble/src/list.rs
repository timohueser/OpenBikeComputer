//! The `routeList`, `rideList` and `tripList` object codecs: the catalogs that outgrow the 512-byte
//! ATT attribute cap and travel over the CoC. Each is a 6-byte [`ListHeader`] and fixed-size
//! entries, so entry `k` sits at `6 + entry_len·k`. The entry length differs per type and travels
//! in the header, so there is no shared entry-length constant.

use crate::descriptor::DescriptorError;

const ROUTE_ENTRY_LEN: usize = 76;
const RIDE_ENTRY_LEN: usize = 72;
const TRIP_ENTRY_LEN: usize = 76;

/// The header decoder's floor: the smallest entry length any list type uses. A reader steps by the
/// header's announced `entry_len`, so a longer entry decodes as the prefix the reader knows.
pub const MIN_LIST_ENTRY_LEN: usize = RIDE_ENTRY_LEN;

/// The 6-byte header every list object shares.
///
/// ```text
///   version    u8   = 2
///   entry_len  u8   the entry size; readers step by it, not by a constant
///   count      u16  entries in this object, after the catalog cap
///   total      u16  full catalog size before the cap; truncated iff total > count
/// ```
///
/// `total` makes a truncation visible on the wire: the device dropped `total - count` entries in
/// catalog order, and the app warns instead of answering "up to date".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListHeader {
    pub count: u16,
    /// Equal to `count` when nothing was dropped, greater when the object is truncated.
    pub total: u16,
}

impl ListHeader {
    pub const ENCODED_LEN: usize = 6;
    pub const VERSION: u8 = 2;

    pub fn encode(&self, entry_len: u8) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0] = Self::VERSION;
        b[1] = entry_len;
        b[2..4].copy_from_slice(&self.count.to_le_bytes());
        b[4..6].copy_from_slice(&self.total.to_le_bytes());
        b
    }

    /// Rejects an unknown version, or an `entry_len` below [`MIN_LIST_ENTRY_LEN`]. The returned
    /// `entry_len` can exceed the type's own: step by it and decode the prefix you know.
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

    /// True when the device dropped `total - count` entries at the cap.
    pub const fn is_truncated(&self) -> bool {
        self.total > self.count
    }

    pub const fn entry_offset(k: usize, entry_len: usize) -> usize {
        Self::ENCODED_LEN + k * entry_len
    }

    pub const fn object_len(count: usize, entry_len: usize) -> usize {
        Self::ENCODED_LEN + count * entry_len
    }

    /// The bounds-checked slot for entry `k`. `None` when the object is shorter than `count`
    /// claims: `decode` reads only the header and cannot police `count`.
    pub fn entry_slice(data: &[u8], k: usize, entry_len: usize) -> Option<&[u8]> {
        let off = Self::ENCODED_LEN + k * entry_len;
        data.get(off..off.checked_add(entry_len)?)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteListEntry<'a> {
    pub object_id: u16,
    pub byte_len: u32,
    pub distance_m: u32,
    pub ascent_m: u32,
    pub point_count: u32,
    pub waypoint_count: u16,
    /// UTF-8, at most [`RouteListEntry::MAX_NAME`] bytes; over-long input is cut at encode.
    pub name: &'a [u8],
    /// Whole-object CRC-32/IEEE of the stored OBCR bytes, the content fingerprint the app matches
    /// against its own record. `0` = unknown; the device fills it in at the first list build.
    pub crc32: u32,
}

impl<'a> RouteListEntry<'a> {
    /// Matches the OBCR route-name field.
    pub const MAX_NAME: usize = 48;
    pub const ENTRY_LEN: usize = ROUTE_ENTRY_LEN;
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

    /// Decodes the first [`ENTRY_LEN`](Self::ENTRY_LEN) bytes of a slot; a longer entry's tail is
    /// ignored.
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

/// One `rideList` entry, built from the stored ride-object header.
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
    /// UTF-8, at most [`RideListEntry::MAX_NAME`] bytes; over-long input is cut at encode.
    pub name: &'a [u8],
}

impl<'a> RideListEntry<'a> {
    /// One byte shorter than the route's: the fixed fields take one more.
    pub const MAX_NAME: usize = 47;
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

/// One `tripList` entry, built from the stored trip object. It mirrors [`RouteListEntry`], with the
/// same trailing whole-object `crc32`: a stage reorder changes neither `byte_len` nor `name`, so
/// only the CRC reveals it.
///
/// ```text
///   object_id         u16
///   reserved          u16  = 0
///   byte_len          u32  stored trip file size
///   total_distance_m  u32  summed over resolvable stages (device-computed)
///   total_ascent_m    u32  summed over resolvable stages
///   stage_count       u16  as stored (incl. dangling refs)
///   reserved          u16  = 0
///   name_len          u8   ≤ 48
///   name              char[48]  UTF-8, zero-padded
///   reserved          u8[3]  = 0
///   crc32             u32  whole-object CRC-32 of the stored trip bytes · 0 = unknown
/// ```
///
/// The totals sum the trip's resolvable stages only, while `stage_count` counts every stored stage,
/// so `stage_count` can exceed the number of stages the totals drew from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TripListEntry<'a> {
    pub object_id: u16,
    pub byte_len: u32,
    pub total_distance_m: u32,
    pub total_ascent_m: u32,
    pub stage_count: u16,
    /// UTF-8, at most [`TripListEntry::MAX_NAME`] bytes; over-long input is cut at encode.
    pub name: &'a [u8],
    /// Whole-object CRC-32/IEEE of the stored trip bytes. `0` = unknown; the device fills it in at
    /// the first list build.
    pub crc32: u32,
}

impl<'a> TripListEntry<'a> {
    /// Matches the trip object's name field.
    pub const MAX_NAME: usize = 48;
    pub const ENTRY_LEN: usize = TRIP_ENTRY_LEN;
    pub const CRC_UNKNOWN: u32 = 0;

    pub fn encode(&self) -> [u8; TRIP_ENTRY_LEN] {
        let mut b = [0u8; TRIP_ENTRY_LEN];
        b[0..2].copy_from_slice(&self.object_id.to_le_bytes());
        // b[2..4] reserved = 0.
        b[4..8].copy_from_slice(&self.byte_len.to_le_bytes());
        b[8..12].copy_from_slice(&self.total_distance_m.to_le_bytes());
        b[12..16].copy_from_slice(&self.total_ascent_m.to_le_bytes());
        b[16..18].copy_from_slice(&self.stage_count.to_le_bytes());
        // b[18..20] reserved = 0.
        let n = self.name.len().min(Self::MAX_NAME);
        b[20] = n as u8;
        b[21..21 + n].copy_from_slice(&self.name[..n]);
        // b[21 + n .. 69] zero padding; b[69..72] reserved = 0.
        b[72..76].copy_from_slice(&self.crc32.to_le_bytes());
        b
    }

    /// Decodes the first [`ENTRY_LEN`](Self::ENTRY_LEN) bytes of a slot; a longer entry's tail is
    /// ignored.
    pub fn decode(data: &'a [u8]) -> Result<Self, DescriptorError> {
        if data.len() < Self::ENTRY_LEN {
            return Err(DescriptorError::Truncated);
        }
        let name_len = (data[20] as usize).min(Self::MAX_NAME);
        Ok(Self {
            object_id: u16::from_le_bytes([data[0], data[1]]),
            byte_len: u32::from_le_bytes([data[4], data[5], data[6], data[7]]),
            total_distance_m: u32::from_le_bytes([data[8], data[9], data[10], data[11]]),
            total_ascent_m: u32::from_le_bytes([data[12], data[13], data[14], data[15]]),
            stage_count: u16::from_le_bytes([data[16], data[17]]),
            name: &data[21..21 + name_len],
            crc32: u32::from_le_bytes([data[72], data[73], data[74], data[75]]),
        })
    }
}
