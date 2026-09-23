//! Recorded-ride v5 summary access.
//!
//! The object begins with the existing 20-byte track samples and ends with one fixed 150-byte
//! footer. Recording therefore writes the final bytes directly; finalize is one footer append,
//! never a whole-ride conversion.

use heapless::String;

use obc_formats::{
    bike::BikeType,
    io::{ByteSource, DecodeError, Error},
    ride::{
        checked_object_len, decode_footer, encode_footer, Footer, Name, TripRef, FOOTER_LEN, MAGIC, NAME_CAP, VERSION,
    },
};

/// The totals captured by the app, plus the wall-clock anchor used to date the first sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RideStats {
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    pub descent_m: u16,
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
    /// The ride's energy from power; `None` without power data.
    pub energy_kj: Option<u32>,
    /// The bike type that was current when the ride started.
    pub bike: BikeType,
    /// The trip day the ride started on.
    pub trip: Option<TripRef>,
    pub trip_name: Name,
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
    let mut footer = Footer::new(
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
    );
    footer.descent_m = stats.descent_m;
    footer.energy_kj = stats.energy_kj;
    footer.bike = stats.bike;
    footer.set_trip(stats.trip, stats.trip_name);
    encode_footer(&footer)
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
    pub descent_m: u16,
    pub point_count: u32,
    pub avg_hr: Option<u8>,
    pub max_hr: Option<u8>,
    pub avg_cadence: Option<u8>,
    pub avg_power: Option<u16>,
    pub max_power: Option<u16>,
    pub energy_kj: Option<u32>,
    pub bike: BikeType,
    pub trip: Option<TripRef>,
    pub trip_name: String<NAME_CAP>,
}

impl RideInfo {
    /// Read only the final footer bytes, validate the footer, then require the catalog/source length
    /// to be exactly `point_count × 20 + FOOTER_LEN`.
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
        let mut trip_name = String::new();
        trip_name.push_str(footer.trip_name()).map_err(|_| Error::TooLarge)?;
        Ok(RideInfo {
            version: VERSION,
            name,
            start_time: footer.start_time,
            distance_m: footer.distance_m,
            moving_time_s: footer.moving_time_s,
            avg_speed_cms: footer.avg_speed_cms,
            climb_m: footer.climb_m,
            descent_m: footer.descent_m,
            point_count: footer.point_count,
            avg_hr: footer.avg_hr,
            max_hr: footer.max_hr,
            avg_cadence: footer.avg_cadence,
            avg_power: footer.avg_power,
            max_power: footer.max_power,
            energy_kj: footer.energy_kj,
            bike: footer.bike,
            trip: footer.trip(),
            trip_name,
        })
    }
}

/// How many bars the ride detail's HR and power graphs hold.
pub const RIDE_SERIES_BUCKETS: usize = 60;

/// What the ride detail shows beside the profile and the shape: the footer's descent, and the
/// whole ride's HR and power as bucket averages. Each bucket covers an equal share of the samples,
/// so a ride with fewer samples than [`RIDE_SERIES_BUCKETS`] has one bucket per sample. A bucket
/// with no reading of a sensor is 0 for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RideTrackFacts {
    pub descent_m: u16,
    hr: [u8; RIDE_SERIES_BUCKETS],
    power: [u16; RIDE_SERIES_BUCKETS],
    len: u8,
}

impl RideTrackFacts {
    pub const EMPTY: Self =
        RideTrackFacts { descent_m: 0, hr: [0; RIDE_SERIES_BUCKETS], power: [0; RIDE_SERIES_BUCKETS], len: 0 };

    pub fn hr(&self) -> &[u8] {
        &self.hr[..self.len as usize]
    }

    pub fn power(&self) -> &[u16] {
        &self.power[..self.len as usize]
    }
}

/// Streams samples, in order, into the buckets of a [`RideTrackFacts`].
pub(crate) struct SeriesFill {
    points: u32,
    bucket: usize,
    hr: (u32, u32),
    power: (u32, u32),
}

impl SeriesFill {
    pub(crate) fn start(out: &mut RideTrackFacts, info: &RideInfo) -> Self {
        *out = RideTrackFacts::EMPTY;
        out.descent_m = info.descent_m;
        out.len = (info.point_count as usize).min(RIDE_SERIES_BUCKETS) as u8;
        SeriesFill { points: info.point_count, bucket: 0, hr: (0, 0), power: (0, 0) }
    }

    /// Add sample `index` of the ride.
    pub(crate) fn push(&mut self, out: &mut RideTrackFacts, index: u32, hr: Option<u8>, power: Option<u16>) {
        let bucket = (u64::from(index) * u64::from(out.len) / u64::from(self.points)) as usize;
        if bucket != self.bucket {
            self.flush(out);
            self.bucket = bucket;
        }
        if let Some(hr) = hr {
            self.hr = (self.hr.0.saturating_add(u32::from(hr)), self.hr.1 + 1);
        }
        if let Some(power) = power {
            self.power = (self.power.0.saturating_add(u32::from(power)), self.power.1 + 1);
        }
    }

    pub(crate) fn finish(mut self, out: &mut RideTrackFacts) {
        if out.len > 0 {
            self.flush(out);
        }
    }

    fn flush(&mut self, out: &mut RideTrackFacts) {
        let avg = |(sum, n): (u32, u32)| sum.checked_div(n).unwrap_or(0);
        out.hr[self.bucket] = avg(self.hr) as u8;
        out.power[self.bucket] = avg(self.power) as u16;
        (self.hr, self.power) = ((0, 0), (0, 0));
    }
}
