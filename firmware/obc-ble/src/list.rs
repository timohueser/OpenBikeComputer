//! The `routeList` / `rideList` object codecs (spec §7.4) — the CoC-downloaded catalogs that
//! outgrow the 512-byte ATT attribute cap. Shared shape: a 4-byte [`ListHeader`] + fixed
//! **72-byte** entries, so entry `k` sits at `4 + 72k` — O(1) indexing, no string scanning.
//!
//! The device encodes (its catalog scan → the wire); the app decodes. Both directions live here so
//! the round-trip is host-tested in one place and the shared `protocol-vectors/` fixture pins the
//! layout for the Swift mirror.

use crate::descriptor::DescriptorError;

/// Both list objects' fixed entry size (spec §7.4). Readers are told this by
/// [`ListHeader::entry_len`] — a future entry growth appends fields and bumps the header value,
/// and an old reader skips the tail it doesn't know.
pub const LIST_ENTRY_LEN: usize = 72;

/// The 4-byte header both list objects share (spec §7.4).
///
/// ```text
///   version    u8   = 1
///   entry_len  u8   = 72 (readers use this, not a constant, to skip entries)
///   count      u16
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListHeader {
    pub count: u16,
}

impl ListHeader {
    pub const ENCODED_LEN: usize = 4;
    /// The list-object version this codec writes.
    pub const VERSION: u8 = 1;

    pub fn encode(&self) -> [u8; Self::ENCODED_LEN] {
        let mut b = [0u8; Self::ENCODED_LEN];
        b[0] = Self::VERSION;
        b[1] = LIST_ENTRY_LEN as u8;
        b[2..4].copy_from_slice(&self.count.to_le_bytes());
        b
    }

    /// Decode a list header, rejecting an unknown version or a zero `entry_len` (a reader must be
    /// able to step entries by it). The returned `entry_len` may exceed [`LIST_ENTRY_LEN`] —
    /// forward compatibility: step by it, decode the prefix you know.
    pub fn decode(data: &[u8]) -> Result<(Self, usize), DescriptorError> {
        if data.len() < Self::ENCODED_LEN {
            return Err(DescriptorError::Truncated);
        }
        if data[0] != Self::VERSION {
            return Err(DescriptorError::UnknownStatus(data[0]));
        }
        let entry_len = data[1] as usize;
        if entry_len < LIST_ENTRY_LEN {
            return Err(DescriptorError::UnknownStatus(data[1]));
        }
        Ok((Self { count: u16::from_le_bytes([data[2], data[3]]) }, entry_len))
    }

    /// Byte offset of entry `k` in the encoded object (with this codec's entry length).
    pub const fn entry_offset(k: usize) -> usize {
        Self::ENCODED_LEN + k * LIST_ENTRY_LEN
    }

    /// The whole encoded object's size for `count` entries.
    pub const fn object_len(count: usize) -> usize {
        Self::ENCODED_LEN + count * LIST_ENTRY_LEN
    }
}

/// One `routeList` entry (spec §7.4) — from the stored OBCR header.
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
}

impl<'a> RouteListEntry<'a> {
    /// The name cap (§7.4, matches the OBCR route-name field).
    pub const MAX_NAME: usize = 48;

    pub fn encode(&self) -> [u8; LIST_ENTRY_LEN] {
        let mut b = [0u8; LIST_ENTRY_LEN];
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
        b
    }

    /// Decode one entry (the first [`LIST_ENTRY_LEN`] bytes of an entry slot — a longer future
    /// entry's tail is ignored, per the header's `entry_len` rule).
    pub fn decode(data: &'a [u8]) -> Result<Self, DescriptorError> {
        if data.len() < LIST_ENTRY_LEN {
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
        })
    }
}

/// One `rideList` entry (spec §7.4) — from the stored ride-object header. Encoded by the device at
/// A7; the codec lands with the list shape so the layouts are pinned together.
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
    /// The name cap (§7.4 — one byte shorter than the route's: the fixed fields take one more).
    pub const MAX_NAME: usize = 47;

    pub fn encode(&self) -> [u8; LIST_ENTRY_LEN] {
        let mut b = [0u8; LIST_ENTRY_LEN];
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
        if data.len() < LIST_ENTRY_LEN {
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
