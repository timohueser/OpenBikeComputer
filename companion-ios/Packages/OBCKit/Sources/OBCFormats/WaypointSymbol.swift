import Foundation
import OBCDomain

/// The canonical GPX/TCX symbol → ``WaypointCategory`` mapping (`OBCR_Spec.md`
/// §4.1) — the phone's half of a table the firmware also carries
/// (`firmware/obc-route/src/symbol.rs`). **Both must stay row-for-row identical**:
/// a route imported on the phone and the same file dropped on the device over USB
/// have to categorize the same way, and the shared `specs/vectors/` fixtures pin
/// exactly that.
///
/// A GPX `<wpt>` names its icon in `<sym>` (Garmin's symbol names, which most
/// planners copy) or `<type>` (RideWithGPS' and Komoot's POI class); a TCX course
/// point uses `PointType`. None of them is a registry, so the vocabularies below
/// are a **curation** from real exports, and two rules keep it safe:
///
/// - **Never drop a waypoint.** An unmapped symbol yields `nil` (generic) and the
///   waypoint imports, stores, and renders like any other.
/// - **Only the six.** Symbols with no honest home among the map's categories stay
///   generic rather than being forced into the nearest one — "Restroom", "Parking",
///   "Ferry", "Hospital", "First Aid", "Viewpoint" and "Summit" are deliberately
///   absent.
public enum WaypointSymbol {
    /// The category a source symbol means, or `nil` for **generic** (empty,
    /// unmapped, or too long to be a symbol at all).
    ///
    /// Matching is case- and separator-insensitive: the same class arrives as
    /// `Drinking Water`, `drinking_water` or `drinking-water` depending on who
    /// exported it, so both sides normalize to lowercase words joined by single
    /// spaces before comparing.
    public static func category(for symbol: String) -> WaypointCategory? {
        let normalized = normalize(symbol)
        guard !normalized.isEmpty else { return nil }
        return table[normalized]
    }

    /// The symbol a `<wpt>` carries: `<sym>` when it says something, else `<type>`
    /// (or `PointType`). Two tags for one idea — some exports write both, so the
    /// Garmin-style `sym` wins rather than competing with `type`.
    public static func symbol(sym: String?, type: String?) -> String {
        let sym = (sym ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if !sym.isEmpty { return sym }
        return (type ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Longest symbol worth normalizing. Every table key is far shorter; a longer
    /// value is freeform prose that could not have matched anyway.
    static let maximumLength = 32

    /// Fold a raw symbol to the table's spelling: ASCII-lowercase, every
    /// non-alphanumeric byte a word break, runs collapsed to one space, ends
    /// trimmed. Non-ASCII scalars are word breaks too — no key contains any.
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

    /// The curated vocabularies, in the spec's order. Sources: **G** = Garmin
    /// BaseCamp symbol names, **R** = RideWithGPS POI types, **K** = Komoot
    /// waypoint types, **O** = OSM-derived tags (what several planners emit
    /// verbatim when a POI came from OSM).
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
        // The six have no separate "food" class, and a rider filtering for
        // supplies wants the bakery *and* the café in one list — so eating and
        // shopping share Resupply.
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
        // Strictly the pharmacy counter. "Hospital" / "First Aid" / "Medical
        // Facility" stay generic: a rider filtering for a pharmacy is looking to
        // buy something, and a hospital row under that icon would mislead in both
        // directions.
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

    /// The vocabularies flattened for lookup — built once, keys already normalized
    /// (a test asserts they are, or a row would be unreachable and the spec table
    /// would lie).
    private static let table: [String: WaypointCategory] = {
        var table: [String: WaypointCategory] = [:]
        for entry in vocabularies {
            for symbol in entry.symbols { table[symbol] = entry.category }
        }
        return table
    }()
}
