import Foundation

/// The device's writable configuration — the semantic view of the OBC Control
/// `Config` characteristic / `config_blob`. Read with `readConfig()`, written
/// with `writeConfig(_:)`.
///
/// **Delta 1 — device name lives in `Config`.** Renaming the device (H3) is a
/// `writeConfig` with a changed `name`; there is no separate rename command. This
/// is a hard requirement on the wire contract — see `OBCProtocol.md` → *Delta 1*.
///
/// **B1 finalization:** carries the contract-mandated `name` plus the display
/// `units` the Settings screen (B8/G) edits. The rest of the config blob (display
/// prefs, sensor pairing, …) is B8's / firmware `S0`'s to grow — kept minimal here
/// so the config codec has a real, round-trippable shape without inventing fields.
public struct DeviceConfig: Equatable, Sendable {
    /// Unit system the device displays. Editable from Settings (G).
    public enum Units: UInt8, Equatable, Sendable, CaseIterable {
        case metric = 0
        case imperial = 1
    }

    /// User-facing device name. Writing this renames the device (H3).
    public var name: String
    /// Display unit system.
    public var units: Units
    /// How often the device raises a scheduled weather request (WX3 / #1188) — the trailing,
    /// optional Config field, held **as the raw byte** so a value this build does not recognise
    /// survives a round-trip and each direction can apply its own rule (spec §11.8). Mirrors
    /// `obc_ble::descriptor::Config::weather_refresh`.
    ///
    /// `nil` means the blob carried **no such byte** — and *absent* is not `.off`, nor does it mean
    /// the same thing in both directions:
    ///
    /// - **Reading** a device's Config, absent means the device is on its **default**
    ///   (`WeatherRefresh.deviceDefault`, 30 minutes) — see `effectiveWeatherRefresh`.
    /// - **Writing** a device's Config, absent means **leave the stored value untouched**; it is
    ///   not a request to reset anything. This is the load-bearing one: an app build predating WX3
    ///   round-trips Config to rename the device and writes the 3-byte-plus-name blob, and a device
    ///   that took that as "the rider chose the default" would reset a rider who deliberately chose
    ///   `.off` back to 30-minute wakeups.
    ///
    /// Read it through `knownWeatherRefresh`; apply a write through `weatherRefreshToApply()`.
    public var weatherRefreshRaw: UInt8?

    /// The ordinary initializer, taking an interval this build knows.
    public init(name: String, units: Units = .metric, weatherRefresh: WeatherRefresh? = nil) {
        self.init(name: name, units: units, weatherRefreshRaw: weatherRefresh?.rawValue)
    }

    /// The raw-byte initializer — how the codec rebuilds a Config off the wire, including a refresh
    /// byte this build cannot name. Deliberately *not* defaulted, so `DeviceConfig(name:)` still
    /// resolves unambiguously to the typed initializer above.
    public init(name: String, units: Units = .metric, weatherRefreshRaw: UInt8?) {
        self.name = name
        self.units = units
        self.weatherRefreshRaw = weatherRefreshRaw
    }

    /// The refresh interval **as a reader sees it**: `nil` when the field was absent *or* names an
    /// interval this build does not know (§11.8). Both collapse to "nothing this build can show",
    /// and neither is `.off`. Mirrors `obc_ble::descriptor::Config::known_refresh`.
    public var knownWeatherRefresh: WeatherRefresh? {
        weatherRefreshRaw.flatMap(WeatherRefresh.init(wireByte:))
    }

    /// The interval the device will actually use, as far as *this* build can tell: the explicit
    /// setting, or the documented default when the blob said nothing. Read this rather than
    /// defaulting a `nil` at each call site — that is exactly where an `?? .off` would slip in and
    /// silently disable weather.
    ///
    /// `nil` **only** when the device named an interval this build does not know. Collapsing that
    /// case into the default here would be the same lie in a friendlier wrapper: the app would show
    /// the rider "every 30 minutes" for a setting they never chose and cannot see.
    public var effectiveWeatherRefresh: WeatherRefresh? {
        guard let raw = weatherRefreshRaw else { return .deviceDefault }
        return WeatherRefresh(wireByte: raw)
    }

    /// Firmware S0 caps the device name at **48 UTF-8 bytes** (spec §7.3 /
    /// `OBCProtocol.md` → Delta 1). The `Config` codec truncates to this at
    /// encode and the rename UI limits to it, so an over-long name can never
    /// overflow the `u16` length field into a corrupt / undersized blob.
    public static let maxNameUTF8Bytes = 48
}

extension String {
    /// This string truncated to at most `maxUTF8Bytes` UTF-8 bytes on a
    /// **Character boundary** — never splitting a grapheme cluster (and so never
    /// a multi-byte UTF-8 sequence), which keeps the result valid UTF-8.
    public func truncatedToUTF8Bytes(_ maxUTF8Bytes: Int) -> String {
        guard utf8.count > maxUTF8Bytes else { return self }
        var result = ""
        var count = 0
        for character in self {
            let width = String(character).utf8.count
            if count + width > maxUTF8Bytes { break }
            result.append(character)
            count += width
        }
        return result
    }
}
