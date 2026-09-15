import Foundation

protocol BLEDiscoveryStore: Sendable {
    func knownPeripheralID() -> UUID?
    func saveKnownPeripheralID(_ id: UUID)
    func clearKnownPeripheralID()

}

struct UserDefaultsBLEDiscoveryStore: BLEDiscoveryStore, @unchecked Sendable {
    private static let peripheralKey = "obc.ble.authenticatedPeripheralID"

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

}
