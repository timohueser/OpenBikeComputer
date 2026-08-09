import Foundation

protocol BLEDiscoveryStore: Sendable {
    func knownPeripheralID() -> UUID?
    func saveKnownPeripheralID(_ id: UUID)
    func clearKnownPeripheralID()
    func armWeatherRestoration(until deadline: Date)
    func weatherRestorationDeadline() -> Date?
    func clearWeatherRestoration()
    /// The upload leg's restoration intent (WX9): a pending direct connect that may relaunch the
    /// app. Separate from the read intent because the two legs have different budgets and either
    /// may be in flight when the process dies.
    func armWeatherUploadRestoration(until deadline: Date)
    func weatherUploadRestorationDeadline() -> Date?
    func clearWeatherUploadRestoration()
    /// The standing weather watch (WX9): survive relaunches, so a background wake re-arms the
    /// weather-only scan without waiting for a foreground session.
    func setWeatherWatchArmed(_ armed: Bool)
    func weatherWatchArmed() -> Bool
}

/// CoreBluetooth's bond keys remain iOS-owned. This store remembers only the opaque peripheral UUID
/// after an authenticated connection, the expiry of in-flight one-shot restoration intents, and
/// whether the standing weather watch is armed. Nothing here is secret, and nothing is any part of
/// a weather request's payload.
struct UserDefaultsBLEDiscoveryStore: BLEDiscoveryStore, @unchecked Sendable {
    private static let peripheralKey = "obc.ble.authenticatedPeripheralID"
    private static let restorationDeadlineKey = "obc.ble.weatherRequestDeadline"
    private static let uploadRestorationDeadlineKey = "obc.ble.weatherUploadDeadline"
    private static let weatherWatchKey = "obc.ble.weatherWatchArmed"

    private let defaults: UserDefaults

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func knownPeripheralID() -> UUID? {
        defaults.string(forKey: Self.peripheralKey).flatMap(UUID.init(uuidString:))
    }

    func saveKnownPeripheralID(_ id: UUID) {
        defaults.set(id.uuidString, forKey: Self.peripheralKey)
    }

    func clearKnownPeripheralID() {
        defaults.removeObject(forKey: Self.peripheralKey)
    }

    func armWeatherRestoration(until deadline: Date) {
        defaults.set(deadline, forKey: Self.restorationDeadlineKey)
    }

    func weatherRestorationDeadline() -> Date? {
        defaults.object(forKey: Self.restorationDeadlineKey) as? Date
    }

    func clearWeatherRestoration() {
        defaults.removeObject(forKey: Self.restorationDeadlineKey)
    }

    func armWeatherUploadRestoration(until deadline: Date) {
        defaults.set(deadline, forKey: Self.uploadRestorationDeadlineKey)
    }

    func weatherUploadRestorationDeadline() -> Date? {
        defaults.object(forKey: Self.uploadRestorationDeadlineKey) as? Date
    }

    func clearWeatherUploadRestoration() {
        defaults.removeObject(forKey: Self.uploadRestorationDeadlineKey)
    }

    func setWeatherWatchArmed(_ armed: Bool) {
        defaults.set(armed, forKey: Self.weatherWatchKey)
    }

    func weatherWatchArmed() -> Bool {
        defaults.bool(forKey: Self.weatherWatchKey)
    }
}
