//! Durable ride-object constants and primitive length arithmetic.

use crate::io::DecodeError;

pub const VERSION_V1: u8 = 1;
pub const VERSION_V2: u8 = 2;
pub const VERSION: u8 = VERSION_V2;
pub const HEADER_LEN_V1: usize = 23;
pub const HEADER_LEN_V2: usize = 31;
pub const POINT_LEN_V1: usize = 14;
pub const POINT_LEN_V2: usize = 18;
pub const ELE_NONE: i16 = i16::MIN;
pub const HR_NONE: u8 = 0xFF;
pub const CAD_NONE: u8 = 0xFF;
pub const PWR_NONE: u16 = 0xFFFF;

pub const fn is_supported_version(version: u8) -> bool {
    version == VERSION_V1 || version == VERSION_V2
}

pub const fn header_len(version: u8) -> usize {
    match version {
        VERSION_V1 => HEADER_LEN_V1,
        _ => HEADER_LEN_V2,
    }
}

pub const fn point_len(version: u8) -> usize {
    match version {
        VERSION_V1 => POINT_LEN_V1,
        _ => POINT_LEN_V2,
    }
}

pub const fn object_len(version: u8, name_len: usize, point_count: u32) -> u32 {
    (header_len(version) + name_len) as u32 + point_len(version) as u32 * point_count
}

/// Calculate the encoded object length without allowing the name, point bytes, or final sum to
/// wrap the format's `u32` length domain.
pub fn checked_object_len(version: u8, name_len: usize, point_count: u32) -> Result<u32, DecodeError> {
    let fixed = header_len(version).checked_add(name_len).ok_or(DecodeError::Bounds)?;
    if fixed > u32::MAX as usize {
        return Err(DecodeError::Bounds);
    }
    let points = (point_len(version) as u32).checked_mul(point_count).ok_or(DecodeError::Bounds)?;
    (fixed as u32).checked_add(points).ok_or(DecodeError::Bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lengths_match_committed_ride_fixtures() {
        for (fixture, version) in [
            (&include_bytes!("../../../specs/vectors/ride-v1.bin")[..], VERSION_V1),
            (&include_bytes!("../../../specs/vectors/ride-v2.bin")[..], VERSION_V2),
        ] {
            assert_eq!(fixture[0], version);
            let name_len = u16::from_le_bytes([fixture[1], fixture[2]]) as usize;
            let tail = 3 + name_len;
            let point_count = u32::from_le_bytes(fixture[tail + 16..tail + 20].try_into().unwrap());
            assert_eq!(fixture.len() as u32, object_len(version, name_len, point_count));
        }
    }

    #[test]
    fn versioned_widths_pin_the_documented_layout() {
        assert_eq!(HEADER_LEN_V2 - HEADER_LEN_V1, 8);
        assert_eq!(POINT_LEN_V2 - POINT_LEN_V1, 4);
        assert!(is_supported_version(VERSION_V1));
        assert!(is_supported_version(VERSION_V2));
        assert!(!is_supported_version(0));
    }

    #[test]
    fn checked_length_rejects_point_and_name_overflow() {
        assert_eq!(checked_object_len(VERSION_V2, 0, 2), Ok(object_len(VERSION_V2, 0, 2)));
        assert_eq!(checked_object_len(VERSION_V2, 0, 0x8000_0000), Err(DecodeError::Bounds));
        assert_eq!(checked_object_len(VERSION_V2, usize::MAX, 0), Err(DecodeError::Bounds));
    }
}
