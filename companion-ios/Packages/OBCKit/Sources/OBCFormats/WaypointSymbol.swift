import Foundation
import OBCDomain

/// Maps GPX and TCX symbol names to ``WaypointCategory``. The firmware carries the
/// same table (`firmware/obc-route/src/symbol.rs`) and both must stay row-for-row
/// identical: one file must categorize the same way over USB and through the phone.
/// The vocabularies are curated from real exports, not a registry. An unmapped symbol
/// yields `nil` and the waypoint still imports as generic. A symbol with no honest
/// home among the six categories stays generic instead of taking the nearest one.
public enum WaypointSymbol {
    /// The category a source symbol means, or `nil` for generic.
    /// Matching is case- and separator-insensitive: `Drinking Water`,
    /// `drinking_water` and `drinking-water` normalize to the same key.
    public static func category(for symbol: String) -> WaypointCategory? {
        let normalized = normalize(symbol)
        guard !normalized.isEmpty else { return nil }
        return table[normalized]
    }

    /// The symbol a waypoint carries: `sym` when it says something, else `type` (or
    /// `PointType`). Some exports write both, and the Garmin-style `sym` wins.
    public static func symbol(sym: String?, type: String?) -> String {
        let sym = (sym ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !sym.isEmpty { return sym }
        return (type ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Longest symbol worth normalizing. A longer value is prose that cannot match.
    static let maximumLength = 32

    /// Fold a raw symbol to the table's spelling: ASCII-lowercase, non-alphanumeric
    /// bytes are word breaks, runs collapse to one space. Non-ASCII scalars are word
    /// breaks too; no key contains one.
    static func normalize(_ symbol: String) -> String {
        var out = ""
        var pendingSpace = false
        for scalar in symbol.unicodeScalars {
            if scalar.isASCII, CharacterSet.alphanumerics.contains(scalar) {
                if pendingSpace, !out.isEmpty { out.append(" ") }
                pendingSpace = false
                out.append(Character(scalar).lowercased())
                if out.utf8.count > maximumLength { return "" }  // unmapped ⇒ generic
            } else {
                pendingSpace = true
            }
        }
        return out
    }

    /// The curated vocabularies. Source tags: G = Garmin BaseCamp, R = RideWithGPS,
    /// K = Komoot, O = OSM tags that several planners emit verbatim.
    static let vocabularies: [(category: WaypointCategory, symbols: [String])] = [
        (.water, [
            "water",              // R, K
            "drinking water",     // G, O (`amenity=drinking_water`)
            "water source",       // G
            "water point",        // O
            "potable water",      // R
            "fountain",           // K
            "drinking fountain",  // R
            "spring",             // O
            "water tap",          // O
            "tap",                // O
            "well",               // O
        ]),
        (.campsite, [
            "campground",    // G
            "camping",       // R, K
            "campsite",      // O (`tourism=camp_site`)
            "camp site",     // O
            "camp",          // K
            "tent",          // K
            "caravan site",  // O
            "rv park",       // R
        ]),
        (.accommodation, [
            "lodging",         // G, R
            "hotel",           // G, O
            "hostel",          // O
            "motel",           // O
            "inn",             // K
            "guest house",     // O
            "guesthouse",      // O
            "bed and breakfast",
            "b b",  // "B&B" — the ampersand normalizes away
            "accommodation",
            "cabin",           // R
            "hut",             // K
            "alpine hut",      // O
            "wilderness hut",  // O
            "refuge",          // K
        ]),
        // There is no separate food class, and a rider looking for supplies wants the
        // bakery and the cafe in one list, so eating and shopping share Resupply.
        (.resupply, [
            "resupply",
            "convenience store",  // G, R
            "convenience",        // O
            "grocery",            // R
            "grocery store",      // R
            "supermarket",        // O
            "shopping center",    // G
            "shopping",           // R
            "store",              // K
            "market",             // K
            "marketplace",        // O
            "bakery",             // O
            "food",               // R, K
            "restaurant",         // G, O
            "fast food",          // G, O
            "pizza",              // G
            "diner",              // G
            "cafe",               // R, O
            "coffee",             // R
            "bar",                // G, R
            "pub",                // O
            "gas station",        // R — a filling station is a resupply stop on a long day
            "fuel",               // O
        ]),
        // Strictly the pharmacy counter. Hospital and First Aid stay generic: a
        // hospital row under this icon would mislead in both directions.
        (.pharmacy, [
            "pharmacy",   // R, O
            "chemist",    // O
            "drugstore",  // R
            "apothecary",
        ]),
        (.bikeShop, [
            "bike shop",       // R, K
            "bicycle shop",    // O (`shop=bicycle`)
            "bike store",      // R
            "cycle shop",      // K
            "cyclery",         // R
            "bike repair",     // K
            "bicycle repair",  // O
            "bike service",    // K
        ]),
    ]

    /// The vocabularies flattened for lookup. A test asserts every key is already
    /// normalized; an un-normalized row would be unreachable.
    private static let table: [String: WaypointCategory] = {
        var table: [String: WaypointCategory] = [:]
        for entry in vocabularies {
            for symbol in entry.symbols { table[symbol] = entry.category }
        }
        return table
    }()
}
