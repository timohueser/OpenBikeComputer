import Foundation

/// A **device object id** (`obc-ble-interface-spec.md` §4.1): the durable `u16`
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
/// `UInt16` keep their on-disk shape.
public struct DeviceObjectID: Hashable, Sendable, Codable {
    public let raw: UInt16

    public init(_ raw: UInt16) { self.raw = raw }

    public init(from decoder: Decoder) throws {
        raw = try decoder.singleValueContainer().decode(UInt16.self)
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        try container.encode(raw)
    }
}

extension DeviceObjectID: CustomStringConvertible {
    public var description: String { String(raw) }
}
