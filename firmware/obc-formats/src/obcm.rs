//! OBCM map-format constants from `OBCM_Spec.md`.

use crate::io::{validate_prefix, DecodeError};

pub const MAGIC: [u8; 4] = *b"OBCM";
pub const VERSION: u8 = 10;
pub const HEADER_LEN: usize = 40;
pub const LOD_ENTRY_LEN: usize = 18;
pub const STYLE_RECORD_LEN: usize = 8;
pub const FEATURE_HEADER_LEN: usize = 12;

pub const FEATURE_FLAG_16BIT: u8 = 0x01;
pub const FEATURE_FLAG_POLYGON: u8 = 0x02;
pub const FEATURE_FLAG_HOLES: u8 = 0x04;
pub const STYLE_PRIORITY_MASK: u8 = 0x03;
pub const STYLE_DASHED_BIT: u8 = 0x04;
pub const STYLE_HAS_COLOR2_BIT: u8 = 0x08;

pub const BRANCH_BIT: u32 = 0x8000_0000;
pub const EMPTY_LEAF: u32 = 0x7FFF_FFFF;
pub const CHUNK_END: u8 = 0xFF;

pub const POI_CATEGORY_COUNT: u8 = 6;
pub const POI_RECORD_LEN: usize = 36;
pub const POI_NAME_LEN: usize = 24;
pub const POI_HOURS_REF_NONE: u16 = 0xFFFF;
pub const POI_HOURS_BLOB_LEN: usize = 29;
pub const POI_CHUNK_SIZE: usize = 512;
pub const POI_CAT_ENTRY_LEN: usize = 13;
pub const POI_DIR_POOL_FIELDS_LEN: usize = 6;

pub const NAV_DIR_LEN: usize = 28;
pub const NAV_CHUNK_SIZE: usize = 512;
pub const NAV_NODE_FIXED_LEN: usize = 13;
pub const NAV_NEIGHBOR_LEN: usize = 15;
pub const NAV_EDGE_FIXED_LEN: usize = 15;
pub const NAV_PROFILE_LEN: usize = 52;
pub const NAV_PROFILE_NAME_LEN: usize = 12;
pub const NAV_MAX_PROFILES: usize = 8;
pub const NAV_MAX_DEGREE: usize = 24;

pub fn validate_header_prefix(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_prefix(bytes, &MAGIC, VERSION, VERSION).map(|_| ())
}

const _: () = assert!(EMPTY_LEAF == !BRANCH_BIT);
const _: () = assert!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN <= NAV_CHUNK_SIZE);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_the_committed_obcm_fixture() {
        let fixture = include_bytes!("../../obc-sim/assets/grimsel-demo.obcm");
        validate_header_prefix(fixture).unwrap();
        assert_eq!(fixture[4], VERSION);
        assert_eq!(u32::from_le_bytes(fixture[21..25].try_into().unwrap()), HEADER_LEN as u32);
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(STYLE_RECORD_LEN, 1 + 1 + 2 + 1 + 1 + 2);
        assert_eq!(FEATURE_HEADER_LEN, 1 + 2 + 4 + 4 + 1);
        assert_eq!(POI_RECORD_LEN, 4 + 4 + 1 + 1 + POI_NAME_LEN + 2);
        assert_eq!(NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN + 32 + 8);
    }
}
