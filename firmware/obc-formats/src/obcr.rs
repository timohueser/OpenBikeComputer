//! OBCR route-format constants from `OBCR_Spec.md`.

use crate::io::{validate_prefix, DecodeError};

pub const MAGIC: &[u8; 4] = b"OBCR";
pub const VERSION_V1: u8 = 1;
pub const VERSION_V2: u8 = 2;
pub const VERSION: u8 = VERSION_V2;
pub const VERSIONS: core::ops::RangeInclusive<u8> = VERSION_V1..=VERSION_V2;
pub const HEADER_LEN: usize = 112;
pub const HEADER_V2_LEN: usize = 128;
pub const CHUNK_META_LEN: usize = 44;
pub const POINT_RECORD_LEN: usize = 6;
pub const NAME_CAP: usize = 48;
pub const WAYPOINT_LEN: usize = 40;
pub const WAYPOINT_NAME_CAP: usize = 24;
pub const WAYPOINT_ELE_NONE: i16 = i16::MIN;

pub const fn is_supported_version(version: u8) -> bool {
    version >= VERSION_V1 && version <= VERSION_V2
}

pub fn validate_header_prefix(bytes: &[u8]) -> Result<u8, DecodeError> {
    validate_prefix(bytes, MAGIC, VERSION_V1, VERSION_V2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_committed_route_fixtures() {
        for fixture in [
            &include_bytes!("../../../specs/vectors/route-plain.obcr")[..],
            &include_bytes!("../../../specs/vectors/route-waypoints.obcr")[..],
        ] {
            assert_eq!(validate_header_prefix(fixture), Ok(VERSION_V2));
            assert!(fixture.len() >= HEADER_V2_LEN);
            assert_eq!(u32::from_le_bytes(fixture[60..64].try_into().unwrap()), HEADER_V2_LEN as u32);
        }
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(HEADER_V2_LEN - HEADER_LEN, 16);
        assert_eq!(CHUNK_META_LEN, 4 * 6 + 2 * 2 + 4 * 4);
        assert_eq!(POINT_RECORD_LEN, 2 + 2 + 2);
        assert_eq!(WAYPOINT_LEN, 4 + 4 + 4 + 2 + 1 + 1 + WAYPOINT_NAME_CAP);
    }
}
