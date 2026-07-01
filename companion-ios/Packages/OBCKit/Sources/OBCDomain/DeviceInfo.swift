import Foundation

/// Minimal identity for a connected OBC device.
///
/// B0 scaffold placeholder — the full domain model (routes, rides, waypoints,
/// device state) lands in B1. Kept a plain `Sendable` value type so it crosses
/// the `DeviceTransport` boundary freely.
public struct DeviceInfo: Equatable, Sendable {
    public let name: String
    public let firmwareVersion: String

    public init(name: String, firmwareVersion: String) {
        self.name = name
        self.firmwareVersion = firmwareVersion
    }
}
