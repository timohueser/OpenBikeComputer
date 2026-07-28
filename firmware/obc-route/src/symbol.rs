//! The canonical GPX symbol → [`PoiCategory`] mapping (`OBCR_Spec.md` §4.1).
//!
//! A GPX `<wpt>` carries its icon as freeform text — `<sym>` (Garmin's symbol name, which most
//! planners copy) or `<type>` (RideWithGPS' and Komoot's POI class). There is no registry: the
//! vocabularies below were read off real Komoot / RideWithGPS / Garmin BaseCamp exports. So this
//! table is a **curation**, not a standard, and it is deliberately the only one in the tree — the
//! spec mirrors it row for row.
//!
//! Two rules keep it safe:
//!
//! - **Never drop a waypoint.** An unmapped symbol (a hand-placed "Turn left here", a planner we
//!   have never seen) yields `None` = Generic, and the waypoint stores and renders like any other.
//! - **Only the six.** Waypoints share the map's `PoiCategory` ids so one icon language covers
//!   both sources (#946). Symbols with no honest home among the six stay Generic rather than being
//!   forced into the nearest one — "Restroom", "Parking", "Ferry", "Hospital", "Viewpoint" and
//!   "Summit" are all deliberately absent.
//!
//! Matching is **case- and separator-insensitive**: the same class arrives as `Drinking Water`,
//! `drinking_water` and `drinking-water` depending on who exported it, so both sides normalise to
//! lowercase words joined by single spaces before comparing ([`normalize`]).

use heapless::String;
use obc_reader::PoiCategory;

/// Longest symbol we bother to normalise. Every table key is far shorter; a longer `<sym>` is
/// freeform prose, which could not have matched anyway, so it degrades to Generic.
const NORM_CAP: usize = 32;

/// One curated symbol vocabulary: the category and the symbols that mean it, already normalised
/// (lowercase, single-space separated) so [`category_for_symbol`] compares directly.
struct SymbolRow {
    category: PoiCategory,
    symbols: &'static [&'static str],
}

/// The canonical table. Sources: **G** = Garmin BaseCamp symbol names, **R** = RideWithGPS POI
/// types, **K** = Komoot waypoint types, **O** = OSM-derived tags (what several planners emit
/// verbatim when a POI came from OSM).
const SYMBOLS: [SymbolRow; 6] = [
    SymbolRow {
        category: PoiCategory::Water,
        symbols: &[
            "water",             // R, K
            "drinking water",    // G, O (`amenity=drinking_water`)
            "water source",      // G
            "water point",       // O
            "potable water",     // R
            "fountain",          // K
            "drinking fountain", // R
            "spring",            // O
            "water tap",         // O
            "tap",               // O
            "well",              // O
        ],
    },
    SymbolRow {
        category: PoiCategory::Campsite,
        symbols: &[
            "campground",   // G
            "camping",      // R, K
            "campsite",     // O (`tourism=camp_site`)
            "camp site",    // O
            "camp",         // K
            "tent",         // K
            "caravan site", // O
            "rv park",      // R
        ],
    },
    SymbolRow {
        category: PoiCategory::Accommodation,
        symbols: &[
            "lodging",     // G, R
            "hotel",       // G, O
            "hostel",      // O
            "motel",       // O
            "inn",         // K
            "guest house", // O
            "guesthouse",  // O
            "bed and breakfast",
            "b b", // "B&B" — the ampersand normalises away
            "accommodation",
            "cabin",          // R
            "hut",            // K
            "alpine hut",     // O
            "wilderness hut", // O
            "refuge",         // K
        ],
    },
    SymbolRow {
        // The six have no separate "food" class, and a rider filtering for supplies wants the
        // bakery *and* the café in one list — so eating and shopping share Resupply.
        category: PoiCategory::Resupply,
        symbols: &[
            "resupply",
            "convenience store", // G, R
            "convenience",       // O
            "grocery",           // R
            "grocery store",     // R
            "supermarket",       // O
            "shopping center",   // G
            "shopping",          // R
            "store",             // K
            "market",            // K
            "marketplace",       // O
            "bakery",            // O
            "food",              // R, K
            "restaurant",        // G, O
            "fast food",         // G, O
            "pizza",             // G
            "diner",             // G
            "cafe",              // R, O
            "coffee",            // R
            "bar",               // G, R
            "pub",               // O
            "gas station",       // R — a filling station is a resupply stop on a long day
            "fuel",              // O
        ],
    },
    SymbolRow {
        // Strictly the pharmacy counter. "Hospital" / "First Aid" / "Medical Facility" stay
        // Generic: a rider filtering for a pharmacy is looking to buy something, and a hospital
        // row under that icon would mislead in both directions.
        category: PoiCategory::Pharmacy,
        symbols: &[
            "pharmacy",  // R, O
            "chemist",   // O
            "drugstore", // R
            "apothecary",
        ],
    },
    SymbolRow {
        category: PoiCategory::BikeShop,
        symbols: &[
            "bike shop",      // R, K
            "bicycle shop",   // O (`shop=bicycle`)
            "bike store",     // R
            "cycle shop",     // K
            "cyclery",        // R
            "bike repair",    // K
            "bicycle repair", // O
            "bike service",   // K
        ],
    },
];

/// The category a GPX `<sym>` / `<type>` value means, or `None` for **Generic** — an empty,
/// unmapped, or over-long symbol. Case- and separator-insensitive (see the module docs).
///
/// Linear over ~70 short strings, run once per waypoint at import (≤ `MAX_WAYPOINTS` per route),
/// never per frame.
pub fn category_for_symbol(symbol: &str) -> Option<PoiCategory> {
    let norm = normalize(symbol);
    if norm.is_empty() {
        return None;
    }
    for row in &SYMBOLS {
        if row.symbols.contains(&norm.as_str()) {
            return Some(row.category);
        }
    }
    None
}

/// Fold a raw symbol to the table's spelling: ASCII-lowercase, every non-alphanumeric byte a word
/// break, runs collapsed to one space, ends trimmed. Non-ASCII bytes are word breaks too — no
/// table key contains any, so an accented symbol simply can't match, and the borrowed buffer keeps
/// the scan allocation-free.
fn normalize(symbol: &str) -> String<NORM_CAP> {
    let mut out: String<NORM_CAP> = String::new();
    let mut pending_space = false;
    for b in symbol.bytes() {
        if b.is_ascii_alphanumeric() {
            if pending_space && !out.is_empty() && out.push(' ').is_err() {
                return String::new(); // longer than any key: unmapped, so Generic
            }
            pending_space = false;
            if out.push(b.to_ascii_lowercase() as char).is_err() {
                return String::new();
            }
        } else {
            pending_space = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_curated_vocabularies() {
        assert_eq!(category_for_symbol("Water"), Some(PoiCategory::Water));
        assert_eq!(category_for_symbol("Campground"), Some(PoiCategory::Campsite));
        assert_eq!(category_for_symbol("Lodging"), Some(PoiCategory::Accommodation));
        assert_eq!(category_for_symbol("Convenience Store"), Some(PoiCategory::Resupply));
        assert_eq!(category_for_symbol("Pharmacy"), Some(PoiCategory::Pharmacy));
        assert_eq!(category_for_symbol("Bike Shop"), Some(PoiCategory::BikeShop));
    }

    #[test]
    fn matching_ignores_case_and_separators() {
        for spelling in ["Drinking Water", "drinking water", "DRINKING_WATER", "drinking-water", "  Drinking  Water "] {
            assert_eq!(category_for_symbol(spelling), Some(PoiCategory::Water), "{spelling}");
        }
        assert_eq!(category_for_symbol("B&B"), Some(PoiCategory::Accommodation));
    }

    #[test]
    fn unmapped_empty_and_overlong_symbols_are_generic() {
        for symbol in ["", "   ", "Turn left here", "Geocache", "Restroom", "Hospital", "Viewpoint", "Summit"] {
            assert_eq!(category_for_symbol(symbol), None, "{symbol}");
        }
        // Past NORM_CAP: no key is that long, so it can only be Generic.
        assert_eq!(category_for_symbol("water water water water water water"), None);
    }

    /// The table is the spec's mirror: keys must already be in normal form, or a row would be
    /// unreachable and the spec table would lie.
    #[test]
    fn every_key_is_already_normalized() {
        for row in &SYMBOLS {
            for symbol in row.symbols {
                assert_eq!(normalize(symbol).as_str(), *symbol, "{symbol} is not in normal form");
                assert_eq!(category_for_symbol(symbol), Some(row.category), "{symbol}");
            }
        }
    }
}
