import Testing
import OBCDomain
@testable import OBCFormats

/// The canonical symbol → category table (`OBCR_Spec.md` §4.1, #947). This is the
/// phone's half of a mapping the firmware also carries
/// (`firmware/obc-route/src/symbol.rs`); these cases mirror that crate's unit
/// tests so a row added on one side and forgotten on the other shows up as a
/// failure rather than as two devices disagreeing about the same GPX.
struct WaypointSymbolTests {
    @Test("each of the six categories has its curated vocabulary")
    func mapsTheCuratedVocabularies() {
        #expect(WaypointSymbol.category(for: "Water") == .water)
        #expect(WaypointSymbol.category(for: "Campground") == .campsite)
        #expect(WaypointSymbol.category(for: "Lodging") == .accommodation)
        #expect(WaypointSymbol.category(for: "Convenience Store") == .resupply)
        #expect(WaypointSymbol.category(for: "Pharmacy") == .pharmacy)
        #expect(WaypointSymbol.category(for: "Bike Shop") == .bikeShop)
    }

    @Test("matching ignores case and separators", arguments: [
        "Drinking Water", "drinking water", "DRINKING_WATER", "drinking-water", "  Drinking  Water ",
    ])
    func matchingIgnoresCaseAndSeparators(_ spelling: String) {
        #expect(WaypointSymbol.category(for: spelling) == .water)
    }

    @Test("an ampersand normalizes away")
    func normalizesPunctuation() {
        #expect(WaypointSymbol.category(for: "B&B") == .accommodation)
    }

    @Test("unmapped, empty and over-long symbols are generic", arguments: [
        "", "   ", "Turn left here", "Geocache", "Restroom", "Hospital", "Viewpoint", "Summit",
        "water water water water water water",  // past the length cap
    ])
    func unmappedSymbolsAreGeneric(_ symbol: String) {
        #expect(WaypointSymbol.category(for: symbol) == nil)
    }

    @Test("<sym> wins over <type>, and an empty <sym> falls through")
    func symTakesPrecedenceOverType() {
        #expect(WaypointSymbol.symbol(sym: "Water", type: "Campground") == "Water")
        #expect(WaypointSymbol.symbol(sym: "", type: "Campground") == "Campground")
        #expect(WaypointSymbol.symbol(sym: "  ", type: "Campground") == "Campground")
        #expect(WaypointSymbol.symbol(sym: nil, type: nil) == "")
    }

    /// The table is the spec's mirror: every key must already be in normal form,
    /// or the row is unreachable and §4.1 lies about what maps.
    @Test("every table key is already normalized and reachable")
    func everyKeyIsNormalized() {
        for entry in WaypointSymbol.vocabularies {
            for symbol in entry.symbols {
                #expect(WaypointSymbol.normalize(symbol) == symbol, "\(symbol) is not in normal form")
                #expect(WaypointSymbol.category(for: symbol) == entry.category, "\(symbol)")
            }
        }
    }

    /// The wire ids are the map's, and generic is `0` — the byte the OBCR record
    /// stores (`OBCR_Spec.md` §4).
    @Test("wire ids match the map's categories")
    func wireIDsMatchTheMap() {
        #expect(WaypointCategory.wireID(nil) == 0)
        #expect(WaypointCategory.water.rawValue == 1)
        #expect(WaypointCategory.bikeShop.rawValue == 6)
        #expect(WaypointCategory(wireID: 0) == nil, "0 is generic, not a category")
        #expect(WaypointCategory(wireID: 7) == nil, "an unknown id reads as generic")
        #expect(WaypointCategory.allCases.count == 6)
    }
}
