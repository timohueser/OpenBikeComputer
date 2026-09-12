import Foundation

/// Identity of a connected OBC device — the semantic mirror of the GATT **DIS**
/// (Device Information Service) plus the wire `protocol_version`.
///
/// **B-S0 skeleton.** The fields track DIS (see `companion-ios/OBCProtocol.md` →
/// *Control plane*); `B1` finalizes the type as it wires `BLETransport`. New
/// fields are defaulted so the scaffold's two-arg call sites keep compiling.
/// Kept a plain `Sendable` value type so it crosses the `DeviceLink`
/// boundary freely.
public struct DeviceInfo: Equatable, Sendable {
    /// User-facing device name. Renamable via `DeviceConfig.name` (H3) — the
    /// name shown here reflects the last-read config. See `OBCProtocol.md` →
    /// *Delta 1*.
    public let name: String
    /// Firmware revision string (DIS 0x2A26).
    public let firmwareVersion: String
    /// Hardware revision string (DIS 0x2A27).
    public let hardwareVersion: String
    /// Serial number string (DIS 0x2A25).
    public let serial: String
    /// Wire `protocol_version` the device reports. The app compares this against
    /// `OBCProtocol.version`; a mismatch surfaces as `DeviceError.protocolMismatch`
    /// (never a crash). See `OBCProtocol.md` → *Versioning*.
    public let protocolVersion: UInt16
    /// The full StoreId learned from the first v4 LIST, as 32 lowercase hex digits.
    public let storeID: String?
    /// The map-format version when a transport explicitly supplies it. Unknown on BLE v4.
    public let obcmVersion: UInt8?
    /// Optional capabilities when a transport explicitly supplies them. Unknown on BLE v4.
    public let featureBits: UInt32?

    /// Weather stays unavailable unless the device announces the contract.
    public var supportsWeather: Bool {
        guard let featureBits else { return false }
        return featureBits & OBCProtocol.featureWeather != 0
    }

    public init(
        name: String,
        firmwareVersion: String,
        hardwareVersion: String = "",
        serial: String = "",
        protocolVersion: UInt16 = OBCProtocol.version,
        storeID: String? = nil,
        obcmVersion: UInt8? = nil,
        featureBits: UInt32? = nil
    ) {
        self.name = name
        self.firmwareVersion = firmwareVersion
        self.hardwareVersion = hardwareVersion
        self.serial = serial
        self.protocolVersion = protocolVersion
        self.storeID = storeID
        self.obcmVersion = obcmVersion
        self.featureBits = featureBits
    }
}
