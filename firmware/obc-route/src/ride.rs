//! Recorded-ride v3 summary access.
//!
//! The object begins with the existing 20-byte track samples and ends with one fixed 84-byte
//! footer. Recording therefore writes the final bytes directly; finalize is one footer append,
//! never a whole-ride conversion.

use heapless::String;

use obc_formats::{
    io::{ByteSource, DecodeError, Error},
    ride::{checked_object_len, decode_footer, encode_footer, Footer, FOOTER_LEN, MAGIC, NAME_CAP, VERSION},
};

/// The totals captured by the app, plus the wall-clock anchor used to date the first sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideStats {
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    /// Unix seconds that were true at [`anchor_ms`](RideStats::anchor_ms).
    pub unix_at_anchor: u32,
    /// The monotonic sample clock at which [`unix_at_anchor`](RideStats::unix_at_anchor) was read.
    pub anchor_ms: u32,
    /// Whether the anchor came from a real time source during this boot.
    pub clock_trusted: bool,
    pub avg_hr: Option<u8>,
    pub max_hr: Option<u8>,
    pub avg_cadence: Option<u8>,
    pub avg_power: Option<u16>,
    pub max_power: Option<u16>,
}

/// Encode the only finish-time payload write.
///
/// `first_t_ms` is the first recorded sample's monotonic timestamp, retained by the recorder. An
/// empty ride passes `None` and is dated at the wall-clock anchor. The subtraction is wrap-safe,
/// matching the sample clock's `u32` wrap behavior.
pub fn encode_summary_footer(
    name: &str,
    stats: &RideStats,
    point_count: u32,
    first_t_ms: Option<u32>,
) -> [u8; FOOTER_LEN] {
    let first_t_ms = first_t_ms.unwrap_or(stats.anchor_ms);
    let start_time = if stats.clock_trusted {
        stats.unix_at_anchor.wrapping_sub(stats.anchor_ms.wrapping_sub(first_t_ms) / 1000)
    } else {
        0
    };
    encode_footer(&Footer::new(
        name,
        start_time,
        stats.distance_m,
        stats.moving_time_s,
        stats.avg_speed_cms,
        stats.climb_m,
        point_count,
        stats.avg_hr,
        stats.max_hr,
        stats.avg_cadence,
        stats.avg_power,
        stats.max_power,
    ))
}

/// A finished ride's list/detail summary, decoded with one footer-sized random read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideInfo {
    pub version: u8,
    pub name: String<NAME_CAP>,
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    pub point_count: u32,
    pub avg_hr: Option<u8>,
    pub max_hr: Option<u8>,
    pub avg_cadence: Option<u8>,
    pub avg_power: Option<u16>,
    pub max_power: Option<u16>,
}

impl RideInfo {
    /// Read only the final 84 bytes, validate the v3 footer, then require the catalog/source length
    /// to be exactly `point_count × 20 + 84`.
    pub fn read(src: &dyn ByteSource) -> Result<RideInfo, Error> {
        let footer_at = src.len().checked_sub(FOOTER_LEN as u64).ok_or(Error::BadOffset)?;
        let mut bytes = [0u8; FOOTER_LEN];
        src.read_at(footer_at, &mut bytes)?;
        if bytes[..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        if bytes[4] != VERSION {
            return Err(Error::BadVersion);
        }
        let footer = decode_footer(&bytes).map_err(|e| match e {
            DecodeError::Version => Error::BadVersion,
            DecodeError::Bounds | DecodeError::Layout => Error::BadOffset,
        })?;
        if checked_object_len(footer.point_count).map_err(|_| Error::BadOffset)? != src.len() {
            return Err(Error::BadOffset);
        }

        let mut name = String::new();
        name.push_str(footer.name()).map_err(|_| Error::TooLarge)?;
        Ok(RideInfo {
            version: VERSION,
            name,
            start_time: footer.start_time,
            distance_m: footer.distance_m,
            moving_time_s: footer.moving_time_s,
            avg_speed_cms: footer.avg_speed_cms,
            climb_m: footer.climb_m,
            point_count: footer.point_count,
            avg_hr: footer.avg_hr,
            max_hr: footer.max_hr,
            avg_cadence: footer.avg_cadence,
            avg_power: footer.avg_power,
            max_power: footer.max_power,
        })
    }
}
