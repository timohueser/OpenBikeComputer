import Foundation
import OBCDomain

/// The bike type the rider chose last: an imported route starts with it. One `UserDefaults` key;
/// a missing or unknown value reads as Road. `@unchecked`: `UserDefaults` is thread-safe but is
/// not annotated `Sendable`.
public struct LastBikeTypeStore: @unchecked Sendable {
    private static let key = "obc.lastBikeType"
    private let defaults: UserDefaults

    public init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    public var value: BikeType {
        get { BikeType(rawValue: UInt8(clamping: defaults.integer(forKey: Self.key))) ?? .road }
        nonmutating set { defaults.set(Int(newValue.rawValue), forKey: Self.key) }
    }
}
