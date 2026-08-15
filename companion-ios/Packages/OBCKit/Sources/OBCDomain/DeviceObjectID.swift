import Foundation

/// A **device object id** (`obc-ble-interface-spec.md` §4.1): the wire `u16` the device uses
/// to name a route, ride, or trip. Values below `0xFF00` are durable for the stored object's life;
/// `0xFF00...0xFFFE` are session-only side-load identities, and `0xFFFF` is the fresh-object
/// sentinel. Neither session identities nor the sentinel may be persisted as library links.
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

    /// Whether this is a durable object identity that may be persisted as a library link. The
    /// session-only side-load band and fresh-object sentinel are both deliberately excluded.
    public var isPersistable: Bool { raw < 0xFF00 }

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
