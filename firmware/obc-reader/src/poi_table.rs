//! The canonical POI category/subtype table (spec §7.4) — the **single firmware source of truth**
//! for the POI id space, `no_std`.
//!
//! Both the packer (`obc-pack`'s `poi.rs`) and the on-device app (#425) mirror this table. The
//! subtype id → `(category, fallback label)` mapping lives *here* because `obc-reader` is the
//! bottom of the shared stack and both sides depend on it: `obc-pack`'s `POI_TABLE` derives each
//! row's category from [`category_of`] rather than restating it (only its OSM `key=value`
//! classification stays packer-side), and `obc-app` reads categories + labels from here for the
//! browser UI — so the id↔category↔label mapping is never maintained in two places.
//!
//! **Ids are normative and append-only** (spec §7.4): an existing subtype's category or the
//! subtype-id numbering itself must never change, or an old map's records decode as the wrong POI.
//! Subtype `0` is reserved; `0xFF` is the chunk end-of-record sentinel and can never be a subtype
//! id. Subtype ids are dense and 1-based, so `subtype - 1` indexes [`SUBTYPES`] directly.

/// The browsable POI categories (spec §7.4). The discriminants are the **stable category ids** —
/// never renumber. Kept as an explicit `#[repr(u8)]` enum so the app can match on a category
/// exhaustively while the ids stay pinned to the format.
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
    /// Every category, in id order — for the app's category list and the invariants test.
    pub const ALL: [PoiCategory; 6] = [
        PoiCategory::Water,
        PoiCategory::Campsite,
        PoiCategory::Accommodation,
        PoiCategory::Resupply,
        PoiCategory::Pharmacy,
        PoiCategory::BikeShop,
    ];

    /// The stable category id (spec §7.4). The same value as the `#[repr(u8)]` discriminant, in a
    /// `const fn` so it's usable in the `SUBTYPES` table below.
    #[inline]
    pub const fn id(self) -> u8 {
        self as u8
    }

    /// The category for a directory `category_id`, or `None` if it isn't one of the six (a corrupt
    /// directory). Used by the query to map a [`PoiCatEntry`](crate::PoiCatEntry) back to a
    /// category if ever needed.
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

    /// This category's display name (for the app's category list). Distinct from a *subtype*'s
    /// fallback label ([`label_of`]).
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

/// One subtype row (spec §7.4): the category it belongs to and the fallback label the device shows
/// when a POI has no stored name. The OSM `key=value` classification that *produced* the subtype
/// stays in the packer (`obc-pack`'s `poi.rs`) — the device never needs it.
#[derive(Debug, Clone, Copy)]
pub struct PoiSubtype {
    pub category: PoiCategory,
    /// Fallback label, printable ASCII, ≤ 14 bytes (fits one device list row).
    pub label: &'static str,
}

const fn sub(category: PoiCategory, label: &'static str) -> PoiSubtype {
    PoiSubtype { category, label }
}

/// The canonical subtype table (spec §7.4), indexed by `subtype - 1` (ids are dense and 1-based).
/// **Append-only** — a new POI kind adds a row at the end with the next subtype id; an existing
/// row's category/label position never moves. `obc-pack`'s `POI_TABLE` is pinned to this
/// row-for-row by a test in that crate.
pub const SUBTYPES: [PoiSubtype; 18] = [
    sub(PoiCategory::Water, "Drinking water"),         // 1  amenity=drinking_water
    sub(PoiCategory::Water, "Spring"),                 // 2  natural=spring
    sub(PoiCategory::Water, "Water tap"),              // 3  man_made=water_tap
    sub(PoiCategory::Water, "Water point"),            // 4  amenity=water_point
    sub(PoiCategory::Campsite, "Campsite"),            // 5  tourism=camp_site
    sub(PoiCategory::Campsite, "Caravan site"),        // 6  tourism=caravan_site
    sub(PoiCategory::Accommodation, "Hotel"),          // 7  tourism=hotel
    sub(PoiCategory::Accommodation, "Hostel"),         // 8  tourism=hostel
    sub(PoiCategory::Accommodation, "Guest house"),    // 9  tourism=guest_house
    sub(PoiCategory::Accommodation, "Motel"),          // 10 tourism=motel
    sub(PoiCategory::Accommodation, "Wilderness hut"), // 11 tourism=wilderness_hut
    sub(PoiCategory::Accommodation, "Alpine hut"),     // 12 tourism=alpine_hut
    sub(PoiCategory::Resupply, "Supermarket"),         // 13 shop=supermarket
    sub(PoiCategory::Resupply, "Convenience"),         // 14 shop=convenience
    sub(PoiCategory::Resupply, "Bakery"),              // 15 shop=bakery
    sub(PoiCategory::Resupply, "Marketplace"),         // 16 amenity=marketplace
    sub(PoiCategory::Pharmacy, "Pharmacy"),            // 17 amenity=pharmacy
    sub(PoiCategory::BikeShop, "Bike shop"),           // 18 shop=bicycle
];

/// Look up a subtype's row (`None` for the reserved `0`, the `0xFF` sentinel, or any id past the
/// table — i.e. a corrupt/newer-format record). Callers skip a `None` subtype rather than panic.
#[inline]
pub fn subtype_row(subtype: u8) -> Option<&'static PoiSubtype> {
    if subtype == 0 {
        return None;
    }
    SUBTYPES.get(subtype as usize - 1)
}

/// The category a subtype belongs to (`None` for an invalid subtype). The single mapping
/// `obc-pack`'s `POI_TABLE` derives its per-row category from, so the two can't drift.
#[inline]
pub fn category_of(subtype: u8) -> Option<PoiCategory> {
    subtype_row(subtype).map(|r| r.category)
}

/// A subtype's fallback label (`None` for an invalid subtype). The device shows this when a POI has
/// no stored name.
#[inline]
pub fn label_of(subtype: u8) -> Option<&'static str> {
    subtype_row(subtype).map(|r| r.label)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pin the table invariants (spec §7.4): subtype ids are contiguous `1..=18`, each maps to a
    /// valid category whose id matches its `#[repr(u8)]` discriminant, and every fallback label is
    /// printable ASCII ≤ 14 bytes (one device list row). Mirrors `obc-pack`'s `table_is_pinned`.
    #[test]
    fn table_invariants() {
        assert_eq!(SUBTYPES.len(), 18, "18 subtypes in v6 (append-only)");
        for (i, row) in SUBTYPES.iter().enumerate() {
            let subtype = (i + 1) as u8;
            // Subtype id → row is `subtype - 1`, so the round-trip must be exact.
            assert_eq!(subtype_row(subtype).map(|r| r.label), Some(row.label), "subtype {subtype} indexes its row");
            // The category id is one of the six, and `from_id` round-trips it.
            let cat = row.category;
            assert!(matches!(cat.id(), 1..=6), "subtype {subtype} category id {} out of 1..=6", cat.id());
            assert_eq!(PoiCategory::from_id(cat.id()), Some(cat), "category id round-trips");
            // Fallback labels fit a device row.
            assert!(row.label.len() <= 14, "subtype {subtype} label {:?} > 14 bytes", row.label);
            assert!(row.label.is_ascii() && row.label.bytes().all(|b| (0x20..=0x7E).contains(&b)), "printable ASCII");
        }
        // Reserved 0, the 0xFF sentinel, and a past-the-table id have no row (skipped, never a panic).
        assert!(subtype_row(0).is_none(), "subtype 0 is reserved");
        assert!(subtype_row(19).is_none(), "no subtype past the table");
        assert!(subtype_row(0xFF).is_none(), "0xFF is the chunk sentinel, never a subtype");
        assert!(category_of(0).is_none() && label_of(0xFF).is_none());
    }

    /// `PoiCategory::ALL` is the six categories in id order, and `from_id`/`id` are inverses across
    /// the whole enum — the app iterates `ALL` and keys directory entries by `id`.
    #[test]
    fn categories_pinned() {
        let ids: [u8; 6] = PoiCategory::ALL.map(|c| c.id());
        assert_eq!(ids, [1, 2, 3, 4, 5, 6]);
        for c in PoiCategory::ALL {
            assert_eq!(PoiCategory::from_id(c.id()), Some(c));
        }
        assert_eq!(PoiCategory::from_id(0), None);
        assert_eq!(PoiCategory::from_id(7), None);
    }
}
