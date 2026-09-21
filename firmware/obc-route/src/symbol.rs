//! The canonical GPX symbol to [`PoiCategory`] mapping. `OBCR_Spec.md` mirrors it row for row.
//!
//! A GPX `<wpt>` carries its icon as freeform text in `<sym>` or `<type>`. There is no registry,
//! so this table is a curation read off real exports, not a standard.
//!
//! Two rules keep it safe. An unmapped symbol yields `None`, which is Generic, so no waypoint is
//! ever dropped. And a symbol with no honest home among the six categories stays Generic instead
//! of being forced into the nearest one, so "Restroom", "Hospital" and "Summit" are all absent.
//!
//! Matching ignores case and separators, because the same class arrives as `Drinking Water`,
//! `drinking_water` or `drinking-water` depending on the exporter.

use heapless::String;
use obc_reader::PoiCategory;

/// Longest symbol that is normalised. Every table key is far shorter, so a longer `<sym>` is
/// freeform prose that could not have matched, and it degrades to Generic.
const NORM_CAP: usize = 32;

/// One curated symbol vocabulary: the category and the symbols that mean it, already normalised
/// (lowercase, single-space separated) so [`category_for_symbol`] compares directly.
struct SymbolRow {
    category: PoiCategory,
    symbols: &'static [&'static str],
}

/// The canonical table. Source letters: G = Garmin BaseCamp, R = RideWithGPS, K = Komoot,
/// O = OSM-derived tags.
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
        // The six have no separate food class, and a rider filtering for supplies wants the
        // bakery and the cafe in one list, so eating and shopping share Resupply.
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
        // Strictly the pharmacy counter. "Hospital" and "First Aid" stay Generic: a rider
        // filtering for a pharmacy wants to buy something.
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

/// The category a GPX `<sym>` or `<type>` value means, or `None` for Generic, which covers an
/// empty, unmapped or over-long symbol. It runs once per waypoint at import, never per frame.
pub(crate) fn category_for_symbol(symbol: &str) -> Option<PoiCategory> {
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

/// Fold a raw symbol to the table's spelling: ASCII lowercase, every non-alphanumeric byte a word
/// break, runs collapsed to one space, ends trimmed. No table key holds a non-ASCII byte, so an
/// accented symbol cannot match.
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

    /// Keys must already be in normal form, or a row would be unreachable.
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
