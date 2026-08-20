//! Recorded-ride v3: verbatim 20-byte samples followed by one fixed summary footer.
//!
//! A recording appends [`crate::track::RECORD_LEN`]-byte samples directly to its final object.
//! Finalize appends [`FOOTER_LEN`] bytes once. There is no leading header and no point rewrite:
//! bytes `0..point_count * SAMPLE_LEN` are exactly the bytes produced by
//! [`crate::track::encode_record`]. The fixed footer can be fetched alone at
//! `object_len - FOOTER_LEN` for a ride-list row.

use crate::io::DecodeError;

pub const MAGIC: [u8; 4] = *b"OBRF";
pub const VERSION: u8 = 3;
pub const FOOTER_LEN: usize = 84;
pub const NAME_CAP: usize = 48;
pub const SAMPLE_LEN: usize = crate::track::RECORD_LEN;

pub use crate::track::{CAD_NONE, HR_NONE, PWR_NONE};

/// The fixed summary at the end of every finished ride object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Footer {
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
    name_len: u8,
    name: [u8; NAME_CAP],
}

impl Footer {
    /// Build a footer, clipping a long UTF-8 name at the last character boundary that fits.
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
        let mut end = name.len().min(NAME_CAP);
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut stored_name = [0; NAME_CAP];
        stored_name[..end].copy_from_slice(&name.as_bytes()[..end]);
        Footer {
            start_time,
            distance_m,
            moving_time_s,
            avg_speed_cms,
            climb_m,
            point_count,
            avg_hr,
            max_hr,
            avg_cadence,
            avg_power,
            max_power,
            name_len: end as u8,
            name: stored_name,
        }
    }

    /// The validated UTF-8 ride name.
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("")
    }
}

/// Encode the normative 84-byte footer.
pub fn encode_footer(footer: &Footer) -> [u8; FOOTER_LEN] {
    let mut b = [0u8; FOOTER_LEN];
    b[0..4].copy_from_slice(&MAGIC);
    b[4] = VERSION;
    b[5] = footer.name_len;
    b[6..8].copy_from_slice(&(FOOTER_LEN as u16).to_le_bytes());
    b[8..12].copy_from_slice(&footer.start_time.to_le_bytes());
    b[12..16].copy_from_slice(&footer.distance_m.to_le_bytes());
    b[16..20].copy_from_slice(&footer.moving_time_s.to_le_bytes());
    b[20..22].copy_from_slice(&footer.avg_speed_cms.to_le_bytes());
    b[22..24].copy_from_slice(&footer.climb_m.to_le_bytes());
    b[24..28].copy_from_slice(&footer.point_count.to_le_bytes());
    b[28] = footer.avg_hr.unwrap_or(HR_NONE);
    b[29] = footer.max_hr.unwrap_or(HR_NONE);
    b[30] = footer.avg_cadence.unwrap_or(CAD_NONE);
    // byte 31 is reserved and remains zero, aligning the following u16 values.
    b[32..34].copy_from_slice(&footer.avg_power.unwrap_or(PWR_NONE).to_le_bytes());
    b[34..36].copy_from_slice(&footer.max_power.unwrap_or(PWR_NONE).to_le_bytes());
    b[36..84].copy_from_slice(&footer.name);
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
    let name_len = b[5] as usize;
    if u16::from_le_bytes([b[6], b[7]]) as usize != FOOTER_LEN || name_len > NAME_CAP || b[31] != 0 {
        return Err(DecodeError::Layout);
    }
    core::str::from_utf8(&b[36..36 + name_len]).map_err(|_| DecodeError::Layout)?;
    if b[36 + name_len..FOOTER_LEN].iter().any(|&v| v != 0) {
        return Err(DecodeError::Layout);
    }

    let mut name = [0u8; NAME_CAP];
    name.copy_from_slice(&b[36..84]);
    Ok(Footer {
        start_time: u32::from_le_bytes(b[8..12].try_into().unwrap()),
        distance_m: u32::from_le_bytes(b[12..16].try_into().unwrap()),
        moving_time_s: u32::from_le_bytes(b[16..20].try_into().unwrap()),
        avg_speed_cms: u16::from_le_bytes(b[20..22].try_into().unwrap()),
        climb_m: u16::from_le_bytes(b[22..24].try_into().unwrap()),
        point_count: u32::from_le_bytes(b[24..28].try_into().unwrap()),
        avg_hr: opt_u8(b[28], HR_NONE),
        max_hr: opt_u8(b[29], HR_NONE),
        avg_cadence: opt_u8(b[30], CAD_NONE),
        avg_power: opt_u16(u16::from_le_bytes(b[32..34].try_into().unwrap()), PWR_NONE),
        max_power: opt_u16(u16::from_le_bytes(b[34..36].try_into().unwrap()), PWR_NONE),
        name_len: name_len as u8,
        name,
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
fn opt_u8(v: u8, sentinel: u8) -> Option<u8> {
    (v != sentinel).then_some(v)
}

#[inline]
fn opt_u16(v: u16, sentinel: u16) -> Option<u16> {
    (v != sentinel).then_some(v)
}

const _: () = assert!(SAMPLE_LEN == 20);
const _: () = assert!(FOOTER_LEN == 36 + NAME_CAP);

#[cfg(test)]
mod tests {
    use super::*;

    fn example() -> Footer {
        Footer::new(
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
        )
    }

    #[test]
    fn footer_round_trip_pins_layout() {
        let footer = example();
        let bytes = encode_footer(&footer);
        assert_eq!(&bytes[..8], b"OBRF\x03\x0bT\0");
        assert_eq!(decode_footer(&bytes), Ok(footer));
        assert_eq!(footer.name(), "Sensor Ride");
        assert_eq!(checked_object_len(3), Ok(3 * 20 + 84));
    }

    #[test]
    fn committed_v3_vector_uses_the_production_footer_codec() {
        let object = include_bytes!("../../../specs/vectors/ride-v3.bin");
        assert_eq!(object.len() as u64, checked_object_len(3).unwrap());
        let footer: &[u8; FOOTER_LEN] = object[object.len() - FOOTER_LEN..].try_into().unwrap();
        let decoded = decode_footer(footer).unwrap();
        assert_eq!(decoded.name(), "Sensor Ride");
        assert_eq!(decoded.point_count, 3);
        assert_eq!(encode_footer(&decoded), *footer);
    }

    #[test]
    fn footer_rejects_noncanonical_fixed_bytes() {
        let bytes = encode_footer(&example());
        for offset in [0, 4, 6, 31, 83] {
            let mut bad = bytes;
            bad[offset] ^= 1;
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
