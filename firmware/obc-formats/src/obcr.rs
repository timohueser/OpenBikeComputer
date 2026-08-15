//! OBCR route-format constants from `OBCR_Spec.md`.

use crate::io::{validate_prefix, DecodeError};

pub const MAGIC: &[u8; 4] = b"OBCR";
/// The one accepted version. v3 rewrote the waypoint record (category + signed lateral offset), so
/// v1/v2 files are **rejected** rather than read — a stored route re-imports from its GPX (the same
/// posture the OBCM v8→v9 bump took).
pub const VERSION_V3: u8 = 3;
pub const VERSION: u8 = VERSION_V3;
pub const VERSIONS: core::ops::RangeInclusive<u8> = VERSION_V3..=VERSION_V3;
/// The header's ride core — every field the geometry path needs.
pub const HEADER_LEN: usize = 112;
/// The whole header: the core plus the 16-byte waypoint extension (§1.1).
pub const HEADER_FULL_LEN: usize = 128;
pub const CHUNK_META_LEN: usize = 44;
pub const POINT_RECORD_LEN: usize = 6;
pub const NAME_CAP: usize = 48;
pub const WAYPOINT_LEN: usize = 44;
pub const WAYPOINT_NAME_CAP: usize = 24;
/// First byte of a waypoint record's name field (§4).
pub const WAYPOINT_NAME_OFF: usize = 20;
pub const WAYPOINT_ELE_NONE: i16 = i16::MIN;
/// The waypoint category byte for "no category" — the diamond every hand-placed waypoint renders
/// as. `1..=6` are the OBCM §7.4 `PoiCategory` wire ids; any other value reads as generic.
pub const WAYPOINT_CATEGORY_GENERIC: u8 = 0;

pub const fn is_supported_version(version: u8) -> bool {
    version == VERSION_V3
}

pub fn validate_header_prefix(bytes: &[u8]) -> Result<u8, DecodeError> {
    validate_prefix(bytes, MAGIC, VERSION_V3, VERSION_V3)
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
            assert_eq!(validate_header_prefix(fixture), Ok(VERSION_V3));
            assert!(fixture.len() >= HEADER_FULL_LEN);
            assert_eq!(u32::from_le_bytes(fixture[60..64].try_into().unwrap()), HEADER_FULL_LEN as u32);
        }
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(HEADER_FULL_LEN - HEADER_LEN, 16);
        assert_eq!(CHUNK_META_LEN, 4 * 6 + 2 * 2 + 4 * 4);
        assert_eq!(POINT_RECORD_LEN, 2 + 2 + 2);
        // dist_along · lon · lat · ele · category · name_len · lateral offset · 2 reserved · name
        assert_eq!(WAYPOINT_LEN, 4 + 4 + 4 + 2 + 1 + 1 + 2 + 2 + WAYPOINT_NAME_CAP);
        assert_eq!(WAYPOINT_NAME_OFF, WAYPOINT_LEN - WAYPOINT_NAME_CAP);
    }

    /// Old versions are rejected outright — the v3 bump is breaking by design.
    #[test]
    fn pre_v3_versions_are_rejected() {
        assert!(!is_supported_version(1));
        assert!(!is_supported_version(2));
        assert!(is_supported_version(VERSION_V3));
        let mut v2 = *b"OBCR\x02";
        assert_eq!(validate_header_prefix(&v2), Err(DecodeError::Version));
        v2[4] = VERSION_V3;
        assert_eq!(validate_header_prefix(&v2), Ok(VERSION_V3));
    }
}
