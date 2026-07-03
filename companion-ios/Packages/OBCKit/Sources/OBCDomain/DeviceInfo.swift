import Foundation

/// Identity of a connected OBC device — the semantic mirror of the GATT **DIS**
/// (Device Information Service) plus the wire `protocol_version`.
///
/// Kept a plain `Sendable` value type so it crosses the `DeviceTransport`
/// boundary freely.
public struct DeviceInfo: Equatable, Sendable {
    /// User-facing device name; reflects the last-read `DeviceConfig.name`.
    public let name: String
    /// Firmware revision string (DIS 0x2A26).
    public let firmwareVersion: String
    /// Hardware revision string (DIS 0x2A27).
    public let hardwareVersion: String
    /// Serial number string (DIS 0x2A25).
    public let serial: String
    /// Wire `protocol_version` the device reports. Compared against
    /// `OBCProtocol.version`; a mismatch surfaces as `DeviceError.protocolMismatch`
    /// (never a crash).
    public let protocolVersion: UInt16

    public init(
        name: String,
        firmwareVersion: String,
        hardwareVersion: String = "",
        serial: String = "",
        protocolVersion: UInt16 = OBCProtocol.version
    ) {
        self.name = name
        self.firmwareVersion = firmwareVersion
        self.hardwareVersion = hardwareVersion
        self.serial = serial
        self.protocolVersion = protocolVersion
    }
}
