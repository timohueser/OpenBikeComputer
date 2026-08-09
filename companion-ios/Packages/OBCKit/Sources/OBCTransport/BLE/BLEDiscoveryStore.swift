import Foundation

protocol BLEDiscoveryStore: Sendable {
    func knownPeripheralID() -> UUID?
    func saveKnownPeripheralID(_ id: UUID)
    func clearKnownPeripheralID()
    func armWeatherRestoration(until deadline: Date)
    func weatherRestorationDeadline() -> Date?
    func clearWeatherRestoration()
}

/// CoreBluetooth's bond keys remain iOS-owned. This store remembers only the opaque peripheral UUID
/// after an authenticated connection and the expiry of an in-flight one-shot restoration intent.
/// Neither value is secret, and neither is any part of a weather request's payload.
struct UserDefaultsBLEDiscoveryStore: BLEDiscoveryStore, @unchecked Sendable {
    private static let peripheralKey = "obc.ble.authenticatedPeripheralID"
    private static let restorationDeadlineKey = "obc.ble.weatherRequestDeadline"

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
}
