import Foundation

/// How often the device raises a scheduled weather request (WX3 / #1188, spec §11).
///
/// The **wire is the raw value**; the minutes are *derived*, so `.off` needs no sentinel minute
/// value and can never be mistaken for "every 0 minutes". Mirrors `obc_ble::WeatherRefresh`
/// field-for-field — it rides two places on the wire: the trailing Config byte (§7.3) and byte 6 of
/// the request context (§11), so the phone learns the device's setting without a second read.
public enum WeatherRefresh: UInt8, Equatable, Sendable, CaseIterable {
    case off = 0
    case every15 = 1
    case every30 = 2
    case every60 = 3
    case every120 = 4

    /// The device default (epic #1185 locks 30 minutes) — and what an **absent** Config field
    /// means. Deliberately not `.off`: see `DeviceConfig.weatherRefreshRaw`.
    public static let deviceDefault: WeatherRefresh = .every30

    /// The interval in minutes; `nil` for `.off`, which has no interval at all rather than a zero
    /// one — a caller that scheduled on a `0` would spin.
    public var minutes: Int? {
        switch self {
        case .off: nil
        case .every15: 15
        case .every30: 30
        case .every60: 60
        case .every120: 120
        }
    }

    /// Decode a refresh byte, or `nil` for a value this build does not know.
    ///
    /// An unknown value is **not** something to paper over with `deviceDefault`: it means the peer
    /// named an interval this build cannot honour, and silently substituting 30 minutes would
    /// report a setting back to the rider that was never applied.
    ///
    /// What a caller *does* with that `nil` is **direction-dependent**, and the asymmetry is the
    /// whole of spec §11.8 (mirrors `obc_ble::WeatherRefresh::from_u8`):
    ///
    /// - **Phone → device, a Config write** is the one direction that must **refuse**: see
    ///   `DeviceConfig.weatherRefreshToApply()`. The device cannot honour an interval it does not
    ///   know, and storing anything else would tell the rider their choice was applied.
    /// - **Device → phone**, both read directions (the request-context read, a Config read), must
    ///   **tolerate**: an unknown value there is a *newer firmware naming an interval this app
    ///   predates*. Treating it as fatal would mean appending a fifth interval — an ordinary enum
    ///   append — silently killed weather on every shipped app, and locked it out of Config badly
    ///   enough that it could no longer even rename the device. Those readers take
    ///   `DeviceConfig.knownWeatherRefresh` / `WeatherRequestContext.refresh`, which report
    ///   *unknown* exactly as an unrecognised `reason` bit is reported: ignored, not fatal.
    public init?(wireByte: UInt8) {
        self.init(rawValue: wireByte)
    }
}
