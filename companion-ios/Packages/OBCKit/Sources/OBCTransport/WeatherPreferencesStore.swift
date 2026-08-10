import Foundation

/// Persistence seam for the **phone-side** weather preference (WX13): whether the standing weather
/// watch is armed.
///
/// One preference, and deliberately not two. The refresh *interval* is the device's — it lives in
/// the Config blob, the device schedules from it, and the app reads it back rather than mirroring
/// it (a phone-side copy would be a second source of truth that drifts the first time a rider
/// changes it on the device). What is genuinely phone-side is whether this phone is willing to keep
/// a background scan armed for the device's requests, which no device setting can express.
///
/// Sits beside ``BondStore`` and ``RetentionDefaultsStore`` for the same reason they do: a
/// preference the composition root reads at launch, before any screen exists, to decide what the
/// transport does.
public protocol WeatherPreferencesStore: Sendable {
    /// Whether the standing watch should be armed. Defaults to **on** for a first launch: the
    /// device asking for weather and the phone never hearing it is the failure mode riders cannot
    /// diagnose, and the scan is UUID-filtered and gated to a bonded peripheral.
    func loadWeatherWatchEnabled() -> Bool
    func saveWeatherWatchEnabled(_ enabled: Bool)
}

/// The real store: one key in `UserDefaults`, mirroring ``UserDefaultsRetentionDefaultsStore``.
/// `@unchecked`: `UserDefaults` is documented thread-safe but is not annotated `Sendable`.
public struct UserDefaultsWeatherPreferencesStore: WeatherPreferencesStore, @unchecked Sendable {
    private static let watchKey = "obc.weather.standingWatch"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func loadWeatherWatchEnabled() -> Bool {
        // An absent key is a first launch, which reads as the documented default rather than as
        // `false` — `bool(forKey:)` alone would silently ship every new install with weather
        // background delivery switched off.
        guard defaults.object(forKey: Self.watchKey) != nil else { return true }
        return defaults.bool(forKey: Self.watchKey)
    }

    public func saveWeatherWatchEnabled(_ enabled: Bool) {
        defaults.set(enabled, forKey: Self.watchKey)
    }
}

/// In-memory store — the default for mock/preview/test composition, so a scenario run starts from
/// the documented default instead of inheriting the previous run's choice.
public final class InMemoryWeatherPreferencesStore: WeatherPreferencesStore, @unchecked Sendable {
    private let lock = NSLock()
    private var watchEnabled: Bool

    public init(watchEnabled: Bool = true) {
        self.watchEnabled = watchEnabled
    }

    public func loadWeatherWatchEnabled() -> Bool { lock.withLock { watchEnabled } }

    public func saveWeatherWatchEnabled(_ enabled: Bool) {
        lock.withLock { watchEnabled = enabled }
    }
}
