//! Recorded-ride v6: verbatim 20-byte samples followed by one fixed summary footer.
//!
//! A recording appends [`crate::track::RECORD_LEN`]-byte samples directly to its final object.
//! Finalize appends [`FOOTER_LEN`] bytes once. There is no leading header and no point rewrite:
//! bytes `0..point_count * SAMPLE_LEN` are exactly the bytes produced by
//! [`crate::track::encode_record`]. The fixed footer can be fetched alone at
//! `object_len - FOOTER_LEN` for a ride-list row.

use core::num::NonZeroU64;

use crate::bike::BikeType;
use crate::io::DecodeError;

pub const MAGIC: [u8; 4] = *b"OBRF";
pub const VERSION: u8 = 6;
pub const FOOTER_LEN: usize = 154;
pub const NAME_CAP: usize = 48;
pub const SAMPLE_LEN: usize = crate::track::RECORD_LEN;

pub use crate::track::{CAD_NONE, HR_NONE, PWR_NONE};
/// The footer's energy sentinel: the ride has no power data.
pub const KJ_NONE: u32 = u32::MAX;

const NAME_AT: usize = 42;
const TRIP_AT: usize = NAME_AT + NAME_CAP;
const TRIP_NAME_AT: usize = TRIP_AT + 12;
const LIMITS_AT: usize = TRIP_NAME_AT + NAME_CAP;

/// A footer name field: UTF-8, clipped at the last character boundary that fits [`NAME_CAP`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Name {
    len: u8,
    bytes: [u8; NAME_CAP],
}

impl Name {
    pub const EMPTY: Name = Name { len: 0, bytes: [0; NAME_CAP] };

    pub fn new(name: &str) -> Name {
        let mut end = name.len().min(NAME_CAP);
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut bytes = [0; NAME_CAP];
        bytes[..end].copy_from_slice(&name.as_bytes()[..end]);
        Name { len: end as u8, bytes }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len as usize]).unwrap_or("")
    }

    /// A zero-padded field is canonical only when every byte past `len` is zero.
    fn decode(len: u8, field: &[u8]) -> Result<Name, DecodeError> {
        let used = len as usize;
        if used > NAME_CAP || field[used..].iter().any(|&v| v != 0) {
            return Err(DecodeError::Layout);
        }
        core::str::from_utf8(&field[..used]).map_err(|_| DecodeError::Layout)?;
        let mut bytes = [0; NAME_CAP];
        bytes.copy_from_slice(field);
        Ok(Name { len, bytes })
    }
}

impl Default for Name {
    fn default() -> Name {
        Name::EMPTY
    }
}

/// The trip day a ride started on (`obc-ble-interface-spec.md` §7.7 names the trip key).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TripRef {
    /// Nonzero, so an `Option<TripRef>` costs no tag.
    key: NonZeroU64,
    day_index: u8,
    day_count: u8,
}

impl TripRef {
    /// `None` unless the key is nonzero, because 0 means "no trip" on the wire, and the day lies
    /// inside the trip.
    pub const fn new(key: u64, day_index: u8, day_count: u8) -> Option<TripRef> {
        match NonZeroU64::new(key) {
            Some(key) if day_index < day_count => Some(TripRef { key, day_index, day_count }),
            _ => None,
        }
    }

    pub const fn key(&self) -> u64 {
        self.key.get()
    }

    /// 0-based.
    pub const fn day_index(&self) -> u8 {
        self.day_index
    }

    /// The trip's day count when the ride started.
    pub const fn day_count(&self) -> u8 {
        self.day_count
    }
}

/// The rider's effort limits, as the settings store them: `0` is not set, and that metric then has
/// no zones. The zone edges are fixed percentages of these, so a ride keeps only the limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EffortLimits {
    /// Maximum heart rate, bpm.
    pub max_hr: u8,
    /// Functional threshold power, W.
    pub ftp_w: u16,
}

/// The fixed summary at the end of every finished ride object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
    pub start_time: u32,
    pub distance_m: u32,
    pub moving_time_s: u32,
    pub avg_speed_cms: u16,
    pub climb_m: u16,
    /// Dead-banded like `climb_m`.
    pub descent_m: u16,
    pub point_count: u32,
    pub avg_hr: Option<u8>,
    pub max_hr: Option<u8>,
    pub avg_cadence: Option<u8>,
    pub avg_power: Option<u16>,
    pub max_power: Option<u16>,
    /// The ride's energy from power, or `None` when it has no power data.
    pub energy_kj: Option<u32>,
    /// The bike type that was current when the ride started.
    pub bike: BikeType,
    /// The effort limits that were in force when the ride started.
    pub limits: EffortLimits,
    name: Name,
    trip: Option<TripRef>,
    trip_name: Name,
}

impl Footer {
    /// Build a footer for a ride on `BikeType::Road` with no descent, no energy, no effort limits
    /// and no trip; long names are clipped.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: &str,
        start_time: u32,
        distance_m: u32,
        moving_time_s: u32,
        avg_speed_cms: u16,
        climb_m: u16,
        point_count: u32,
        avg_hr: Option<u8>,
        max_hr: Option<u8>,
        avg_cadence: Option<u8>,
        avg_power: Option<u16>,
        max_power: Option<u16>,
    ) -> Footer {
        Footer {
            start_time,
            distance_m,
            moving_time_s,
            avg_speed_cms,
            climb_m,
            descent_m: 0,
            point_count,
            avg_hr,
            max_hr,
            avg_cadence,
            avg_power,
            max_power,
            energy_kj: None,
            bike: BikeType::Road,
            limits: EffortLimits::default(),
            name: Name::new(name),
            trip: None,
            trip_name: Name::EMPTY,
        }
    }

    /// The validated UTF-8 ride name.
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn trip(&self) -> Option<TripRef> {
        self.trip
    }

    /// The trip's name; empty without a trip.
    pub fn trip_name(&self) -> &str {
        self.trip_name.as_str()
    }

    /// Set the trip day and the trip's name. Without a trip the name is dropped.
    pub fn set_trip(&mut self, trip: Option<TripRef>, name: Name) {
        self.trip = trip;
        self.trip_name = if trip.is_some() { name } else { Name::EMPTY };
    }
}

/// Encode the normative footer.
pub fn encode_footer(footer: &Footer) -> [u8; FOOTER_LEN] {
    let mut b = [0u8; FOOTER_LEN];
    b[0..4].copy_from_slice(&MAGIC);
    b[4] = VERSION;
    b[5] = footer.name.len;
    b[6..8].copy_from_slice(&(FOOTER_LEN as u16).to_le_bytes());
    b[8..12].copy_from_slice(&footer.start_time.to_le_bytes());
    b[12..16].copy_from_slice(&footer.distance_m.to_le_bytes());
    b[16..20].copy_from_slice(&footer.moving_time_s.to_le_bytes());
    b[20..22].copy_from_slice(&footer.avg_speed_cms.to_le_bytes());
    b[22..24].copy_from_slice(&footer.climb_m.to_le_bytes());
    b[24..26].copy_from_slice(&footer.descent_m.to_le_bytes());
    b[26..30].copy_from_slice(&footer.point_count.to_le_bytes());
    b[30] = footer.avg_hr.unwrap_or(HR_NONE);
    b[31] = footer.max_hr.unwrap_or(HR_NONE);
    b[32] = footer.avg_cadence.unwrap_or(CAD_NONE);
    // byte 33 is reserved and remains zero, aligning the following u16 values.
    b[34..36].copy_from_slice(&footer.avg_power.unwrap_or(PWR_NONE).to_le_bytes());
    b[36..38].copy_from_slice(&footer.max_power.unwrap_or(PWR_NONE).to_le_bytes());
    b[38..42].copy_from_slice(&footer.energy_kj.unwrap_or(KJ_NONE).to_le_bytes());
    b[NAME_AT..TRIP_AT].copy_from_slice(&footer.name.bytes);
    if let Some(trip) = footer.trip {
        b[TRIP_AT..TRIP_AT + 8].copy_from_slice(&trip.key().to_le_bytes());
        b[TRIP_AT + 8] = trip.day_index;
        b[TRIP_AT + 9] = trip.day_count;
        b[TRIP_AT + 11] = footer.trip_name.len;
        b[TRIP_NAME_AT..LIMITS_AT].copy_from_slice(&footer.trip_name.bytes);
    }
    b[TRIP_AT + 10] = footer.bike as u8;
    b[LIMITS_AT] = footer.limits.max_hr;
    // The next byte is reserved and remains zero, aligning FTP.
    b[LIMITS_AT + 2..FOOTER_LEN].copy_from_slice(&footer.limits.ftp_w.to_le_bytes());
    b
}

/// Decode and validate one footer read. Object-length validation is deliberately separate because
/// a list row reads only these bytes; a whole-object reader must additionally call
/// [`checked_object_len`] and compare it with the catalog length.
pub fn decode_footer(b: &[u8; FOOTER_LEN]) -> Result<Footer, DecodeError> {
    if b[0..4] != MAGIC {
        return Err(DecodeError::Layout);
    }
    if b[4] != VERSION {
        return Err(DecodeError::Version);
    }
    if u16::from_le_bytes([b[6], b[7]]) as usize != FOOTER_LEN || b[33] != 0 || b[LIMITS_AT + 1] != 0 {
        return Err(DecodeError::Layout);
    }
    let name = Name::decode(b[5], &b[NAME_AT..TRIP_AT])?;
    let bike = BikeType::from_u8(b[TRIP_AT + 10]).ok_or(DecodeError::Layout)?;
    let key = u64::from_le_bytes(b[TRIP_AT..TRIP_AT + 8].try_into().unwrap());
    let trip_name = Name::decode(b[TRIP_AT + 11], &b[TRIP_NAME_AT..LIMITS_AT])?;
    let trip = TripRef::new(key, b[TRIP_AT + 8], b[TRIP_AT + 9]);
    // Without a trip, every trip byte is zero.
    let no_trip = key == 0 && b[TRIP_AT + 8] == 0 && b[TRIP_AT + 9] == 0 && trip_name.len == 0;
    if trip.is_none() && !no_trip {
        return Err(DecodeError::Layout);
    }

    Ok(Footer {
        start_time: u32::from_le_bytes(b[8..12].try_into().unwrap()),
        distance_m: u32::from_le_bytes(b[12..16].try_into().unwrap()),
        moving_time_s: u32::from_le_bytes(b[16..20].try_into().unwrap()),
        avg_speed_cms: u16::from_le_bytes(b[20..22].try_into().unwrap()),
        climb_m: u16::from_le_bytes(b[22..24].try_into().unwrap()),
        descent_m: u16::from_le_bytes(b[24..26].try_into().unwrap()),
        point_count: u32::from_le_bytes(b[26..30].try_into().unwrap()),
        avg_hr: opt(b[30], HR_NONE),
        max_hr: opt(b[31], HR_NONE),
        avg_cadence: opt(b[32], CAD_NONE),
        avg_power: opt(u16::from_le_bytes(b[34..36].try_into().unwrap()), PWR_NONE),
        max_power: opt(u16::from_le_bytes(b[36..38].try_into().unwrap()), PWR_NONE),
        energy_kj: opt(u32::from_le_bytes(b[38..42].try_into().unwrap()), KJ_NONE),
        bike,
        limits: EffortLimits { max_hr: b[LIMITS_AT], ftp_w: u16::from_le_bytes([b[LIMITS_AT + 2], b[LIMITS_AT + 3]]) },
        name,
        trip,
        trip_name,
    })
}

/// Exact length of a finished ride object: verbatim samples plus its fixed footer.
pub fn checked_object_len(point_count: u32) -> Result<u64, DecodeError> {
    u64::from(point_count)
        .checked_mul(SAMPLE_LEN as u64)
        .and_then(|n| n.checked_add(FOOTER_LEN as u64))
        .ok_or(DecodeError::Bounds)
}

#[inline]
fn opt<T: PartialEq>(v: T, sentinel: T) -> Option<T> {
    (v != sentinel).then_some(v)
}

const _: () = assert!(SAMPLE_LEN == 20);
const _: () = assert!(core::mem::size_of::<Option<TripRef>>() == core::mem::size_of::<TripRef>());
const _: () = assert!(FOOTER_LEN == LIMITS_AT + 4);

#[cfg(test)]
mod tests {
    use super::*;

    const TRIP: TripRef = TripRef::new(0x0123_4567_89AB_CDEF, 1, 3).unwrap();

    fn example() -> Footer {
        let mut footer = Footer::new(
            "Sensor Ride",
            1_751_449_700,
            42_500,
            9_000,
            472,
            810,
            3,
            Some(142),
            Some(176),
            Some(85),
            Some(210),
            Some(480),
        );
        footer.descent_m = 640;
        footer.energy_kj = Some(756);
        footer.bike = BikeType::Gravel;
        footer.limits = EffortLimits { max_hr: 185, ftp_w: 250 };
        footer.set_trip(Some(TRIP), Name::new("Alpen Traverse"));
        footer
    }

    #[test]
    fn footer_round_trip_pins_layout() {
        let footer = example();
        let bytes = encode_footer(&footer);
        assert_eq!(&bytes[..8], b"OBRF\x06\x0b\x9a\0");
        assert_eq!(&bytes[22..30], &[0x2A, 3, 0x80, 2, 3, 0, 0, 0], "climb, descent, point count");
        assert_eq!(&bytes[34..42], &[210, 0, 224, 1, 0xF4, 2, 0, 0], "power, then energy");
        assert_eq!(&bytes[90..102], &[0xEF, 0xCD, 0xAB, 0x89, 0x67, 0x45, 0x23, 0x01, 1, 3, 1, 14]);
        assert_eq!(&bytes[150..], &[185, 0, 250, 0], "max HR, reserved, FTP");
        assert_eq!(decode_footer(&bytes), Ok(footer));
        assert_eq!(footer.name(), "Sensor Ride");
        assert_eq!(footer.trip_name(), "Alpen Traverse");
        assert_eq!(checked_object_len(3), Ok(3 * 20 + 154));
    }

    #[test]
    fn no_power_data_stays_apart_from_zero_kj() {
        for energy_kj in [None, Some(0)] {
            let footer = Footer { energy_kj, ..example() };
            assert_eq!(decode_footer(&encode_footer(&footer)), Ok(footer));
        }
    }

    #[test]
    fn a_ride_without_a_trip_keeps_every_trip_byte_zero() {
        let mut footer = example();
        footer.set_trip(None, Name::new("ignored"));
        let bytes = encode_footer(&footer);
        assert!(bytes[90..100].iter().chain(&bytes[101..150]).all(|&v| v == 0));
        assert_eq!(bytes[100], BikeType::Gravel as u8);
        assert_eq!(decode_footer(&bytes).unwrap().trip(), None);
        assert_eq!(TripRef::new(0, 0, 1), None, "key 0 is never a trip");
    }

    #[test]
    fn committed_v6_vector_uses_the_production_footer_codec() {
        let object = include_bytes!("../../../specs/vectors/ride-v6.bin");
        assert_eq!(object.len() as u64, checked_object_len(3).unwrap());
        let footer: &[u8; FOOTER_LEN] = object[object.len() - FOOTER_LEN..].try_into().unwrap();
        let decoded = decode_footer(footer).unwrap();
        assert_eq!(decoded.name(), "Sensor Ride");
        assert_eq!(decoded.point_count, 3);
        assert_eq!(decoded.bike, BikeType::Gravel);
        assert_eq!(decoded.trip(), Some(TRIP));
        assert_eq!(decoded.trip_name(), "Alpen Traverse");
        assert_eq!(decoded.limits, EffortLimits { max_hr: 185, ftp_w: 250 });
        assert_eq!(encode_footer(&decoded), *footer);
    }

    #[test]
    fn footer_rejects_noncanonical_fixed_bytes() {
        let bytes = encode_footer(&example());
        // Magic, version, length, reserved, name padding, a day past the count, a bike type past
        // the four, trip-name padding, and the reserved byte after max HR.
        for (offset, value) in [(0, 0), (4, 5), (6, 150), (33, 1), (89, 1), (98, 3), (100, 4), (149, 1), (151, 1)] {
            let mut bad = bytes;
            bad[offset] = value;
            assert!(decode_footer(&bad).is_err(), "offset {offset}");
        }
        let mut no_trip = example();
        no_trip.set_trip(None, Name::EMPTY);
        let no_trip = encode_footer(&no_trip);
        // A day index, a day count or a trip name without a trip key.
        for offset in [98, 99, 101] {
            let mut bad = no_trip;
            bad[offset] = 1;
            assert!(decode_footer(&bad).is_err(), "offset {offset}");
        }
    }

    #[test]
    fn long_name_is_clipped_at_utf8_boundary() {
        let name = std::format!("a{}", "ü".repeat(30));
        let footer = Footer::new(&name, 0, 0, 0, 0, 0, 0, None, None, None, None, None);
        assert_eq!(footer.name().len(), 47);
        assert_eq!(footer.name(), std::format!("a{}", "ü".repeat(23)));
    }
}
