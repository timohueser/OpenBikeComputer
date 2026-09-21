import Foundation

/// A device object id: the durable `u64` the device's object store names its routes and rides
/// by, assigned on upload or record and stable across reboots for the life of the object.
///
/// It is distinct from the app's library identity (`RouteID`) on purpose, so passing a library
/// id where a device id belongs is a compile error, not a silent no-match. It encodes and
/// decodes as a bare number.
public struct DeviceObjectID: Hashable, Sendable, Codable {
    public let raw: UInt64

    public init<T: BinaryInteger>(_ raw: T) { self.raw = UInt64(raw) }

    public init(from decoder: Decoder) throws {
        raw = try decoder.singleValueContainer().decode(UInt64.self)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(raw)
    }
}

extension DeviceObjectID: CustomStringConvertible {
    public var description: String { String(raw) }
}
