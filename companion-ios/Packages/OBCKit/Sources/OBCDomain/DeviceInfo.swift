import Foundation

/// Identity of a connected OBC device — the semantic mirror of the GATT **DIS**
/// (Device Information Service) plus the wire `protocol_version`.
///
/// **B-S0 skeleton.** The fields track DIS (see `companion-ios/OBCProtocol.md` →
/// *Control plane*); `B1` finalizes the type as it wires `BLETransport`. New
/// fields are defaulted so the scaffold's two-arg call sites keep compiling.
/// Kept a plain `Sendable` value type so it crosses the `DeviceTransport`
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
    /// The device's **store epoch** (v2 `protocolVersion` read: `version u16 ·
    /// store_epoch u32`) — a TRNG nonce the device changes only on an id-era reset
    /// (a full-chip reflash, factory reset, or torn id-marks line). `nil` when the
    /// read carried no epoch: a v1 device (a 2-byte read, taking the #303 mismatch
    /// path) or a short/torn v2 read. **Deliberately not defaulted to `0`** — `0`
    /// is a legal epoch, so a missing epoch must read as "unknown", never a
    /// fabricated value. The (serial, epoch) library scoping that consumes it is
    /// V5 (#769); this field is the wire fact it stands on.
    public let storeEpoch: UInt32?

    public init(
        name: String,
        firmwareVersion: String,
        hardwareVersion: String = "",
        serial: String = "",
        protocolVersion: UInt16 = OBCProtocol.version,
        storeEpoch: UInt32? = nil
    ) {
        self.name = name
        self.firmwareVersion = firmwareVersion
        self.hardwareVersion = hardwareVersion
        self.serial = serial
        self.protocolVersion = protocolVersion
        self.storeEpoch = storeEpoch
    }
}
