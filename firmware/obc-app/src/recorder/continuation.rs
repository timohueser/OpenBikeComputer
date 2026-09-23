//! The fixed continuation payload stored with a ride checkpoint.

use obc_formats::bike::BikeType;
use obc_formats::ride::TripRef;

pub const RIDE_RESUME_LEN: usize = 96;
const RESUME_MAGIC: [u8; 4] = *b"OBRC";
const RESUME_VERSION: u16 = 3;

pub fn encode(state: super::RideContinuation, start_time: Option<u32>) -> [u8; RIDE_RESUME_LEN] {
    const _: () = assert!(RIDE_RESUME_LEN == 96);
    let mut out = [0u8; RIDE_RESUME_LEN];
    out[0..4].copy_from_slice(&RESUME_MAGIC);
    out[4..6].copy_from_slice(&RESUME_VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&(RIDE_RESUME_LEN as u16).to_le_bytes());
    out[8..12].copy_from_slice(&start_time.unwrap_or(0).to_le_bytes());
    for (at, value) in
        [(12, state.ridden_m), (16, state.moving_m), (20, state.moving_s), (24, state.climb_m), (28, state.descent_m)]
    {
        out[at..at + 4].copy_from_slice(&value.to_bits().to_le_bytes());
    }
    out[32..40].copy_from_slice(&state.hr_ms_sum.to_le_bytes());
    out[40..44].copy_from_slice(&state.hr_ms.to_le_bytes());
    out[44..46].copy_from_slice(&state.max_hr.to_le_bytes());
    out[48..56].copy_from_slice(&state.power_ms_sum.to_le_bytes());
    out[56..60].copy_from_slice(&state.power_ms.to_le_bytes());
    out[60..62].copy_from_slice(&state.max_power.to_le_bytes());
    out[64..72].copy_from_slice(&state.cadence_ms_sum.to_le_bytes());
    out[72..76].copy_from_slice(&state.cadence_ms.to_le_bytes());
    out[76] = u8::from(start_time.is_some());
    if let Some(trip) = state.origin.trip {
        out[80..88].copy_from_slice(&trip.key().to_le_bytes());
        out[88] = trip.day_index();
        out[89] = trip.day_count();
    }
    out[90] = state.origin.bike as u8;
    if let Some(joules) = state.energy_j {
        out[91] = 1;
        out[92..96].copy_from_slice(&joules.to_le_bytes());
    }
    out
}

pub fn decode(bytes: &[u8; RIDE_RESUME_LEN]) -> Option<(super::RideContinuation, Option<u32>)> {
    if bytes[0..4] != RESUME_MAGIC
        || u16::from_le_bytes(bytes[4..6].try_into().ok()?) != RESUME_VERSION
        || u16::from_le_bytes(bytes[6..8].try_into().ok()?) as usize != RIDE_RESUME_LEN
        || bytes[46..48].iter().any(|byte| *byte != 0)
        || bytes[62..64].iter().any(|byte| *byte != 0)
        || bytes[76] > 1
        || bytes[77..80].iter().any(|byte| *byte != 0)
        || bytes[91] > 1
        || (bytes[91] == 0 && bytes[92..].iter().any(|byte| *byte != 0))
    {
        return None;
    }
    let key = u64::from_le_bytes(bytes[80..88].try_into().ok()?);
    let trip = TripRef::new(key, bytes[88], bytes[89]);
    if trip.is_none() && (key != 0 || bytes[88] != 0 || bytes[89] != 0) {
        return None;
    }
    let origin = super::RideOrigin { bike: BikeType::from_u8(bytes[90])?, trip };
    let f32_at = |at: usize| {
        let mut raw = [0u8; 4];
        raw.copy_from_slice(&bytes[at..at + 4]);
        f32::from_bits(u32::from_le_bytes(raw))
    };
    let state = super::RideContinuation {
        origin,
        ridden_m: f32_at(12),
        moving_m: f32_at(16),
        moving_s: f32_at(20),
        climb_m: f32_at(24),
        descent_m: f32_at(28),
        hr_ms_sum: u64::from_le_bytes(bytes[32..40].try_into().ok()?),
        hr_ms: u32::from_le_bytes(bytes[40..44].try_into().ok()?),
        max_hr: u16::from_le_bytes(bytes[44..46].try_into().ok()?),
        power_ms_sum: u64::from_le_bytes(bytes[48..56].try_into().ok()?),
        power_ms: u32::from_le_bytes(bytes[56..60].try_into().ok()?),
        max_power: u16::from_le_bytes(bytes[60..62].try_into().ok()?),
        cadence_ms_sum: u64::from_le_bytes(bytes[64..72].try_into().ok()?),
        cadence_ms: u32::from_le_bytes(bytes[72..76].try_into().ok()?),
        energy_j: (bytes[91] == 1).then(|| u32::from_le_bytes([bytes[92], bytes[93], bytes[94], bytes[95]])),
    };
    let finite_nonnegative = [state.ridden_m, state.moving_m, state.moving_s, state.climb_m, state.descent_m]
        .iter()
        .all(|value| value.is_finite() && *value >= 0.0);
    let start = u32::from_le_bytes(bytes[8..12].try_into().ok()?);
    let start = (bytes[76] == 1).then_some(start);
    finite_nonnegative.then_some((state, start))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_layout_retains_sensor_numerators_and_validates_reserved_fields() {
        let state = super::super::RideContinuation {
            origin: super::super::RideOrigin {
                bike: BikeType::Touring,
                trip: TripRef::new(0x0102_0304_0506_0708, 2, 5),
            },
            ridden_m: 1.0,
            moving_m: 2.0,
            moving_s: 3.0,
            climb_m: 4.0,
            descent_m: 5.0,
            hr_ms_sum: 6,
            hr_ms: 7,
            max_hr: 8,
            power_ms_sum: 9,
            power_ms: 10,
            max_power: 11,
            cadence_ms_sum: 12,
            cadence_ms: 13,
            energy_j: Some(14),
        };
        let bytes = encode(state, Some(0x01020304));
        assert_eq!(&bytes[..16], &[79, 66, 82, 67, 3, 0, 96, 0, 4, 3, 2, 1, 0, 0, 128, 63]);
        assert_eq!(&bytes[32..48], &[6, 0, 0, 0, 0, 0, 0, 0, 7, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(&bytes[48..64], &[9, 0, 0, 0, 0, 0, 0, 0, 10, 0, 0, 0, 11, 0, 0, 0]);
        assert_eq!(&bytes[64..77], &[12, 0, 0, 0, 0, 0, 0, 0, 13, 0, 0, 0, 1]);
        assert_eq!(&bytes[80..96], &[8, 7, 6, 5, 4, 3, 2, 1, 2, 5, 3, 1, 14, 0, 0, 0]);
        assert_eq!(decode(&bytes), Some((state, Some(0x01020304))));
        assert_eq!(decode(&encode(state, None)), Some((state, None)));
        let no_trip =
            super::super::RideContinuation { origin: super::super::RideOrigin::default(), energy_j: None, ..state };
        assert_eq!(decode(&encode(no_trip, None)), Some((no_trip, None)));
        for at in [0, 4, 6, 46, 62, 77] {
            let mut invalid = bytes;
            invalid[at] ^= 1;
            assert!(decode(&invalid).is_none());
        }
        // A start flag past 1, a day past the count, and a bike type past the four.
        // …and an energy flag past 1, or energy bytes without the flag.
        let mut no_flag = encode(no_trip, None);
        no_flag[95] = 1;
        assert!(decode(&no_flag).is_none());
        for (at, value) in [(76, 2), (88, 5), (90, 4), (91, 2)] {
            let mut invalid = bytes;
            invalid[at] = value;
            assert!(decode(&invalid).is_none());
        }
        for value in [f32::NAN, f32::INFINITY, -1.0] {
            let mut invalid = bytes;
            invalid[24..28].copy_from_slice(&value.to_bits().to_le_bytes());
            assert!(decode(&invalid).is_none());
        }
    }
}
