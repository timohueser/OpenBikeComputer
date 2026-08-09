import Foundation
import OBCWeatherWire

/// The frozen MET `symbol_code` → canonical condition mapping (WX1 decision record).
///
/// Frozen means: this table is the contract, and a code it does not recognise maps to
/// `.unavailable` rather than to a guess. MET adds symbols occasionally; showing "cloudy" for a
/// symbol we have never seen would be inventing weather, while `.unavailable` is a truthful gap the
/// UI already knows how to render.
///
/// `_day`, `_night` and `_polartwilight` are daylight variants of the *same* condition, so the
/// suffix is stripped before matching. Matching is then by substring, in precedence order, which is
/// what makes MET's compound codes (`lightsleetshowersandthunder`) resolve the way the table says:
/// thunder wins over its precipitation, and the precipitation phase (sleet, snow) wins over the
/// showers/rain shape.
public enum METSymbolMapping {
    public static func condition(for symbolCode: String) -> OBCWeatherCondition? {
        let trimmed = symbolCode.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        guard !trimmed.isEmpty else { return nil }  // an empty code is malformed, not "unknown"
        var base = trimmed
        for variant in ["_day", "_night", "_polartwilight"] where base.hasSuffix(variant) {
            base = String(base.dropLast(variant.count))
        }
        guard !base.isEmpty else { return nil }

        // MET has no accepted mapping to `hail` or `wind`; those stay unavailable rather than
        // being inferred from a gust value the symbol never claimed.
        if base.contains("andthunder") { return .thunderstorm }
        if base.contains("sleet") { return .sleet }
        if base.contains("snow") { return .snow }
        if base.contains("showers") { return .showers }
        if base == "lightrain" { return .drizzle }
        if base.contains("rain") { return .rain }
        if base.hasPrefix("clearsky") { return .clear }
        if base.hasPrefix("fair") { return .mostlyClear }
        if base.hasPrefix("partlycloudy") { return .partlyCloudy }
        if base == "cloudy" { return .overcast }
        if base == "fog" { return .fog }
        return .unavailable
    }
}
