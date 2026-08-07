//! OBCM map-format constants from `OBCM_Spec.md`.

use crate::io::{validate_prefix, DecodeError};

pub const MAGIC: [u8; 4] = *b"OBCM";
pub const VERSION: u8 = 12;
pub const HEADER_LEN: usize = 40;
pub const LOD_ENTRY_LEN: usize = 18;
pub const STYLE_RECORD_LEN: usize = 8;

/// Width of the **compact** feature header (§5): `style, flags, pt_count u8, anchor u16 ×2`.
/// The common case — a feature of ≤ 255 vertices whose leaf-relative anchor fits `0..=65535`.
pub const FEATURE_HEADER_COMPACT_LEN: usize = 7;
/// Width of the **wide** feature header (§5, `FEATURE_FLAG_WIDE` set): `style, flags,
/// pt_count u16, anchor i32 ×2` — the escape for a big feature or a leaf spanning more than
/// 65 535 µdeg. Both layouts put `flags` at byte 1, so a reader knows the width before it needs it.
pub const FEATURE_HEADER_WIDE_LEN: usize = 12;

pub const FEATURE_FLAG_16BIT: u8 = 0x01;
pub const FEATURE_FLAG_POLYGON: u8 = 0x02;
pub const FEATURE_FLAG_HOLES: u8 = 0x04;
pub const FEATURE_FLAG_WIDE: u8 = 0x08;
pub const STYLE_PRIORITY_MASK: u8 = 0x03;
pub const STYLE_DASHED_BIT: u8 = 0x04;
pub const STYLE_HAS_COLOR2_BIT: u8 = 0x08;
/// Style-record flag bit 4 (§2, #1095): the style's `weight` is the on-screen stroke width in
/// device pixels, used **verbatim** — the renderer's zoom→width ramp is bypassed for it.
pub const STYLE_FIXED_WIDTH_BIT: u8 = 0x10;
/// Style-record flag bit 5 (§2, #1095): the style belongs to the suppressible **terrain layer**.
/// Written by the packer; the consumer is the device Settings toggle (#1096).
pub const STYLE_TERRAIN_LAYER_BIT: u8 = 0x20;
/// Style-record flag bits 6-7 (§2): still reserved, written `0`. Unlike a *feature*'s flags
/// (§5.2, [`FEATURE_FLAG_WIDE`] & friends), a reader MUST **ignore** style bits it does not
/// define rather than reject the record — that is what lets a bit be defined in place.
pub const STYLE_RESERVED_MASK: u8 = 0xC0;

pub const BRANCH_BIT: u32 = 0x8000_0000;
pub const EMPTY_LEAF: u32 = 0x7FFF_FFFF;
pub const CHUNK_END: u8 = 0xFF;

pub const POI_CATEGORY_COUNT: u8 = 6;
pub const POI_RECORD_LEN: usize = 36;
pub const POI_NAME_LEN: usize = 24;
pub const POI_HOURS_REF_NONE: u16 = 0xFFFF;
pub const POI_HOURS_BLOB_LEN: usize = 29;
pub const POI_HOURS_DAYS: usize = 7;
pub const POI_HOURS_SLOTS_PER_DAY: usize = 2;
pub const POI_HOURS_FLAG_SEASONAL: u8 = 0x01;
pub const POI_HOURS_FLAG_TRUNCATED: u8 = 0x02;
pub const POI_CHUNK_SIZE: usize = 512;
pub const POI_CAT_ENTRY_LEN: usize = 13;
pub const POI_DIR_POOL_FIELDS_LEN: usize = 6;

pub const NAV_DIR_LEN: usize = 28;
pub const NAV_CHUNK_SIZE: usize = 512;
pub const NAV_NODE_FIXED_LEN: usize = 13;
/// Width of one §8.3 adjacency entry. **17 in v12** (#1073): `id u32, dlat i16, dlon i16,
/// edge_id u32, cost_m u16, way_kind u8, ascent_m u16`.
pub const NAV_NEIGHBOR_LEN: usize = 17;
/// Offset of the v12 `Ascent M` field inside a §8.3 neighbor entry — the integrated climb of
/// riding the edge **from this record's node toward the neighbor**, so the two sides of an edge
/// carry different values (§8.3's one exception to "both sides agree").
pub const NAV_NEIGHBOR_ASCENT_OFF: usize = 15;
pub const NAV_EDGE_FIXED_LEN: usize = 15;
/// Width of one §8.6 profile record. **56 in v12** (#1073): the 52-byte v9 record plus
/// [`NAV_PROFILE_CLIMB_WEIGHT_OFF`] and three reserved bytes written `0`.
pub const NAV_PROFILE_LEN: usize = 56;
pub const NAV_PROFILE_NAME_LEN: usize = 12;
/// Offset of the v12 `Climb Weight` byte inside a §8.6 profile record: flat-metres-equivalent per
/// metre of ascent, `0` = climb-blind.
pub const NAV_PROFILE_CLIMB_WEIGHT_OFF: usize = 52;
/// Length of the reserved tail after `Climb Weight`; written `0`, ignored on read.
pub const NAV_PROFILE_RESERVED_LEN: usize = 3;
pub const NAV_MAX_PROFILES: usize = 8;
pub const NAV_MAX_DEGREE: usize = 24;

/// Padding inserted immediately before a populated §8.2 node index so the fixed 512-byte node
/// chunks following that index begin on a physical sector boundary. `index_offset` is the absolute
/// file offset the index would have without padding; the index itself may remain unaligned because
/// readers fetch its cache windows at aligned absolute offsets. The return is always `0..512`.
///
/// This is wire-compatible with every v12 reader: the directory carries the absolute index offset,
/// while node chunks start at `index_offset + index_len`. Keeping the arithmetic here makes the
/// standalone packer and streaming volume assembler agree exactly.
#[inline]
pub const fn nav_index_padding(index_offset: u64, index_len: u64) -> usize {
    let sector = NAV_CHUNK_SIZE as u64;
    let data_rem = (index_offset % sector + index_len % sector) % sector;
    ((sector - data_rem) % sector) as usize
}

/// The browsable POI categories from OBCM spec §7.4. Discriminants are stable wire ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PoiCategory {
    Water = 1,
    Campsite = 2,
    Accommodation = 3,
    Resupply = 4,
    Pharmacy = 5,
    BikeShop = 6,
}

impl PoiCategory {
    pub const ALL: [PoiCategory; POI_CATEGORY_COUNT as usize] = [
        PoiCategory::Water,
        PoiCategory::Campsite,
        PoiCategory::Accommodation,
        PoiCategory::Resupply,
        PoiCategory::Pharmacy,
        PoiCategory::BikeShop,
    ];

    #[inline]
    pub const fn id(self) -> u8 {
        self as u8
    }

    #[inline]
    pub const fn from_id(id: u8) -> Option<PoiCategory> {
        Some(match id {
            1 => PoiCategory::Water,
            2 => PoiCategory::Campsite,
            3 => PoiCategory::Accommodation,
            4 => PoiCategory::Resupply,
            5 => PoiCategory::Pharmacy,
            6 => PoiCategory::BikeShop,
            _ => return None,
        })
    }

    /// Stable device-facing category label. Distinct from a subtype fallback label.
    #[inline]
    pub const fn name(self) -> &'static str {
        match self {
            PoiCategory::Water => "Water",
            PoiCategory::Campsite => "Campsite",
            PoiCategory::Accommodation => "Lodging",
            PoiCategory::Resupply => "Resupply",
            PoiCategory::Pharmacy => "Pharmacy",
            PoiCategory::BikeShop => "Bike shop",
        }
    }
}

/// One append-only OBCM spec §7.4 subtype row.
#[derive(Debug, Clone, Copy)]
pub struct PoiSubtype {
    pub category: PoiCategory,
    pub label: &'static str,
}

const fn subtype(category: PoiCategory, label: &'static str) -> PoiSubtype {
    PoiSubtype { category, label }
}

/// Canonical subtype table, indexed by `subtype_id - 1`.
pub const POI_SUBTYPES: [PoiSubtype; 18] = [
    subtype(PoiCategory::Water, "Drinking water"),
    subtype(PoiCategory::Water, "Spring"),
    subtype(PoiCategory::Water, "Water tap"),
    subtype(PoiCategory::Water, "Water point"),
    subtype(PoiCategory::Campsite, "Campsite"),
    subtype(PoiCategory::Campsite, "Caravan site"),
    subtype(PoiCategory::Accommodation, "Hotel"),
    subtype(PoiCategory::Accommodation, "Hostel"),
    subtype(PoiCategory::Accommodation, "Guest house"),
    subtype(PoiCategory::Accommodation, "Motel"),
    subtype(PoiCategory::Accommodation, "Wilderness hut"),
    subtype(PoiCategory::Accommodation, "Alpine hut"),
    subtype(PoiCategory::Resupply, "Supermarket"),
    subtype(PoiCategory::Resupply, "Convenience"),
    subtype(PoiCategory::Resupply, "Bakery"),
    subtype(PoiCategory::Resupply, "Marketplace"),
    subtype(PoiCategory::Pharmacy, "Pharmacy"),
    subtype(PoiCategory::BikeShop, "Bike shop"),
];

#[inline]
pub fn poi_subtype_row(subtype_id: u8) -> Option<&'static PoiSubtype> {
    if subtype_id == 0 {
        return None;
    }
    POI_SUBTYPES.get(subtype_id as usize - 1)
}

#[inline]
pub fn poi_category_of(subtype_id: u8) -> Option<PoiCategory> {
    poi_subtype_row(subtype_id).map(|row| row.category)
}

#[inline]
pub fn poi_label_of(subtype_id: u8) -> Option<&'static str> {
    poi_subtype_row(subtype_id).map(|row| row.label)
}

pub fn validate_header_prefix(bytes: &[u8]) -> Result<(), DecodeError> {
    validate_prefix(bytes, &MAGIC, VERSION, VERSION).map(|_| ())
}

const _: () = assert!(EMPTY_LEAF == !BRANCH_BIT);
const _: () = assert!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN <= NAV_CHUNK_SIZE);
const _: () = assert!(POI_HOURS_BLOB_LEN == 1 + POI_HOURS_DAYS * POI_HOURS_SLOTS_PER_DAY * 2);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constants_match_the_committed_obcm_fixture() {
        let fixture = include_bytes!("../../../apps/obc-sim/assets/grimsel-demo.obcm");
        validate_header_prefix(fixture).unwrap();
        assert_eq!(fixture[4], VERSION);
        assert_eq!(u32::from_le_bytes(fixture[21..25].try_into().unwrap()), HEADER_LEN as u32);
    }

    #[test]
    fn record_widths_pin_spec_arithmetic() {
        assert_eq!(STYLE_RECORD_LEN, 1 + 1 + 2 + 1 + 1 + 2);
        assert_eq!(FEATURE_HEADER_COMPACT_LEN, 1 + 1 + 1 + 2 + 2);
        assert_eq!(FEATURE_HEADER_WIDE_LEN, 1 + 1 + 2 + 4 + 4);
        assert_eq!(POI_RECORD_LEN, 4 + 4 + 1 + 1 + POI_NAME_LEN + 2);
        // v12 §8.6: the v9 record (name + two multiplier tables) plus climb weight + reserved.
        assert_eq!(NAV_PROFILE_LEN, NAV_PROFILE_NAME_LEN + 32 + 8 + 1 + NAV_PROFILE_RESERVED_LEN);
        assert_eq!(NAV_PROFILE_CLIMB_WEIGHT_OFF, NAV_PROFILE_NAME_LEN + 32 + 8);
        // v12 §8.3: the v9 entry plus the directional `Ascent M`.
        assert_eq!(NAV_NEIGHBOR_LEN, 4 + 2 + 2 + 4 + 2 + 1 + 2);
        assert_eq!(NAV_NEIGHBOR_ASCENT_OFF, NAV_NEIGHBOR_LEN - 2);
        // The §8.3 degree-cap derivation: 13 + 17 × 24 = 421 ≤ 512, so the pinned nav chunk still
        // holds a cap-degree record whole.
        assert_eq!(NAV_NODE_FIXED_LEN + NAV_MAX_DEGREE * NAV_NEIGHBOR_LEN, 421);
    }

    #[test]
    fn nav_index_padding_aligns_the_first_data_chunk_without_overflow() {
        assert_eq!(nav_index_padding(84, 4), 424);
        assert_eq!(nav_index_padding(508, 4), 0);
        assert_eq!(nav_index_padding(500, 20), 504);
        let near_max = u64::MAX - 3;
        let pad = nav_index_padding(near_max, 12);
        assert!(pad < NAV_CHUNK_SIZE);
        assert_eq!(((near_max % 512) + (pad as u64 % 512) + 12) % 512, 0);
    }

    #[test]
    fn poi_id_tables_pin_the_append_only_contract() {
        assert_eq!(POI_SUBTYPES.len(), 18);
        assert_eq!(PoiCategory::ALL.map(PoiCategory::id), [1, 2, 3, 4, 5, 6]);
        for (index, row) in POI_SUBTYPES.iter().enumerate() {
            let subtype_id = (index + 1) as u8;
            assert_eq!(poi_subtype_row(subtype_id).map(|value| value.label), Some(row.label));
            assert_eq!(PoiCategory::from_id(row.category.id()), Some(row.category));
            assert!(row.label.len() <= 14);
            assert!(row.label.is_ascii() && row.label.bytes().all(|byte| (0x20..=0x7E).contains(&byte)));
        }
        assert!(poi_subtype_row(0).is_none());
        assert!(poi_subtype_row(CHUNK_END).is_none());
        assert!(poi_subtype_row(19).is_none());
        assert_eq!(PoiCategory::from_id(0), None);
        assert_eq!(PoiCategory::from_id(7), None);
    }
}
