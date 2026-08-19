import Foundation

/// A **device object id** (`FLAT_Store_Protocol.md` §3): the durable `u64`
/// the device's object store names its routes and rides by — assigned on
/// upload/record, stable across reboots for the life of the stored object.
///
/// Distinct from the app's **library identity** (`RouteID`) on purpose: the two
/// namespaces meet only at `PlannedRouteRecord.deviceObjectID` (the persisted
/// link between a library route and its device copy), and the transport's data
/// plane speaks this type exclusively — so passing a library id where a device
/// id belongs is a compile error, not a silent no-match (#359).
///
/// Encodes/decodes as a **bare number**, so persisted DTOs that stored the raw
/// number keep their on-disk shape while v4 ids can use the full store namespace.
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
