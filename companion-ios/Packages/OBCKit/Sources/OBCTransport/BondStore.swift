import Foundation

/// What the app remembers about its bonded device. iOS owns the actual BLE bond
/// (there is no CoreBluetooth API to enumerate it), so the app keeps this small
/// record as its own source of truth for the launch branch (B2): present →
/// bonded → the quiet connecting state; absent → the pairing flow.
public struct BondRecord: Equatable, Sendable {
    /// The device name to greet with before the link is up ("Connecting to
    /// Trailhead…"). Updated on rename (H3) via a fresh `save`.
    public var deviceName: String

    public init(deviceName: String) {
        self.deviceName = deviceName
    }
}

/// Persistence seam for the bond record — a `DeviceTransport`-side concern, per
/// B2: **the launch branch never takes a CoreBluetooth detour** to ask about
/// bonds. Conformers: `UserDefaultsBondStore` (real) and OBCMock's
/// `MockBondStore` (scenario-driven, `#if DEBUG`).
public protocol BondStore: Sendable {
    /// The remembered bond, or nil when the app has never paired (or forgot).
    func load() -> BondRecord?
    /// Record a successful pairing (D4) — or refresh the name after a rename.
    func save(_ record: BondRecord)
    /// Forget the device (H2). iOS keeps the underlying bond until the user
    /// removes it in Settings; the app just stops assuming it.
    func clear()
}

/// The real store: one key in `UserDefaults`. Nothing secret lives here — the
/// bond's crypto material is iOS's, this is only "we have paired, with <name>".
/// `@unchecked`: `UserDefaults` is documented thread-safe but the iOS SDK
/// doesn't annotate it `Sendable`.
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
