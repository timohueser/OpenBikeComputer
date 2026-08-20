//! Recorded-track fixed-record constants and the fixed 20-byte record codec.
//!
//! The normative constants and the `encode_record`/`decode_record` primitives are the byte
//! authority; streaming GPX export from a validated finished ride-v3 object (`track_to_gpx`) stays
//! in `obc-route`. This crate owns only the record layout so a recorder or reader can encode/decode
//! one sample without depending on `obc-route`.

use obc_ports::TrackPoint;

pub const RECORD_LEN: usize = 20;
pub const FLAG_SEGMENT_START: u16 = 0x0001;
pub const HR_NONE: u8 = 0xFF;
pub const CAD_NONE: u8 = 0xFF;
pub const PWR_NONE: u16 = 0xFFFF;

/// Encode a point to its fixed 20-byte record (little-endian). Absent sensor values encode as
/// their sentinels ([`HR_NONE`] / [`CAD_NONE`] / [`PWR_NONE`]).
pub fn encode_record(p: &TrackPoint) -> [u8; RECORD_LEN] {
    let mut b = [0u8; RECORD_LEN];
    b[0..4].copy_from_slice(&p.lon.to_le_bytes());
    b[4..8].copy_from_slice(&p.lat.to_le_bytes());
    b[8..10].copy_from_slice(&p.ele.to_le_bytes());
    let flags = if p.segment_start { FLAG_SEGMENT_START } else { 0 };
    b[10..12].copy_from_slice(&flags.to_le_bytes());
    b[12..16].copy_from_slice(&p.t_ms.to_le_bytes());
    b[16] = p.hr.unwrap_or(HR_NONE);
    b[17] = p.cadence.unwrap_or(CAD_NONE);
    b[18..20].copy_from_slice(&p.power.unwrap_or(PWR_NONE).to_le_bytes());
    b
}

/// Decode one fixed 20-byte record. A sentinel sensor field decodes back to `None`.
pub fn decode_record(b: &[u8; RECORD_LEN]) -> TrackPoint {
    let lon = i32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let lat = i32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let ele = i16::from_le_bytes([b[8], b[9]]);
    let flags = u16::from_le_bytes([b[10], b[11]]);
    let t_ms = u32::from_le_bytes([b[12], b[13], b[14], b[15]]);
    let hr = (b[16] != HR_NONE).then_some(b[16]);
    let cadence = (b[17] != CAD_NONE).then_some(b[17]);
    let pwr = u16::from_le_bytes([b[18], b[19]]);
    let power = (pwr != PWR_NONE).then_some(pwr);
    TrackPoint { lon, lat, ele, t_ms, segment_start: flags & FLAG_SEGMENT_START != 0, hr, cadence, power }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_width_pins_the_documented_layout() {
        assert_eq!(RECORD_LEN, 4 + 4 + 2 + 2 + 4 + 1 + 1 + 2);
        assert_eq!(FLAG_SEGMENT_START, 1);
    }

    #[test]
    fn record_round_trips_through_encode_decode() {
        let p = TrackPoint {
            lon: -7_654_321,
            lat: 47_123_456,
            ele: -321,
            t_ms: 1_234_567,
            segment_start: true,
            hr: Some(142),
            cadence: Some(88),
            power: Some(240),
        };
        assert_eq!(decode_record(&encode_record(&p)), p);
        // Absent sensor fields round-trip back to None through the sentinels.
        let bare =
            TrackPoint { lon: 0, lat: 0, ele: 0, t_ms: 0, segment_start: false, hr: None, cadence: None, power: None };
        assert_eq!(decode_record(&encode_record(&bare)), bare);
    }
}
