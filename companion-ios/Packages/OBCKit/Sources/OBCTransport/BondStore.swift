import Foundation

/// What the app remembers about its bonded device. iOS owns the actual BLE bond and has no API
/// to enumerate it, so this record is the app's own source of truth for the launch branch.
public struct BondRecord: Equatable, Sendable {
    /// The device name to greet with before the link is up. A rename saves a fresh record.
    public var deviceName: String

    public init(deviceName: String) {
        self.deviceName = deviceName
    }
}

/// Persistence seam for the bond record: the launch branch never takes a CoreBluetooth detour
/// to ask about bonds.
public protocol BondStore: Sendable {
    /// The remembered bond, or nil when the app has never paired (or forgot).
    func load() -> BondRecord?
    /// Record a successful pairing, or refresh the name after a rename.
    func save(_ record: BondRecord)
    /// Forget the device. iOS keeps the underlying bond until the user removes it in Settings;
    /// the app only stops assuming it.
    func clear()
}

/// The real store: one key in `UserDefaults`. Nothing secret lives here; the bond's crypto
/// material is iOS's. `@unchecked`: `UserDefaults` is thread-safe but is not annotated
/// `Sendable`.
public struct UserDefaultsBondStore: BondStore, @unchecked Sendable {
    private static let key = "obc.bondedDeviceName"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public func load() -> BondRecord? {
        defaults.string(forKey: Self.key).map(BondRecord.init(deviceName:))
    }

    public func save(_ record: BondRecord) {
        defaults.set(record.deviceName, forKey: Self.key)
    }

    public func clear() {
        defaults.removeObject(forKey: Self.key)
    }
}
