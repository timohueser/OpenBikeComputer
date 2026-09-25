import Foundation

/// Identity of a connected OBC device: the semantic mirror of the GATT Device Information
/// Service plus the wire `protocol_version`.
public struct DeviceInfo: Equatable, Sendable {
    /// The name to show before the device reports its own: the product name, which reads as a
    /// name anywhere in a sentence.
    public static let unnamed = "OBC"

    /// User-facing device name, as the last-read config reported it. A rename goes through
    /// `DeviceConfig.name`.
    public let name: String
    /// Firmware revision string (DIS 0x2A26).
    public let firmwareVersion: String
    /// Hardware revision string (DIS 0x2A27).
    public let hardwareVersion: String
    /// Serial number string (DIS 0x2A25).
    public let serial: String
    /// Wire `protocol_version` the device reports. A mismatch with `OBCProtocol.version`
    /// surfaces as `DeviceError.protocolMismatch`, never a crash.
    public let protocolVersion: UInt16
    /// The full StoreId learned from the first v4 LIST, as 32 lowercase hex digits.
    public let storeID: String?
    /// The map-format version when a transport explicitly supplies it. Unknown on BLE v4.
    public let obcmVersion: UInt8?

    public init(
        name: String,
        firmwareVersion: String,
        hardwareVersion: String = "",
        serial: String = "",
        protocolVersion: UInt16 = OBCProtocol.version,
        storeID: String? = nil,
        obcmVersion: UInt8? = nil
    ) {
        self.name = name
        self.firmwareVersion = firmwareVersion
        self.hardwareVersion = hardwareVersion
        self.serial = serial
        self.protocolVersion = protocolVersion
        self.storeID = storeID
        self.obcmVersion = obcmVersion
    }
}
