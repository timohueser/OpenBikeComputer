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
    /// The **OBCM map-format version** this device's firmware reads (the identity read's third
    /// field, E1 / #911) — `10` today. Not to be confused with `protocolVersion` beside it: that is
    /// the *wire* contract, a different number in a different sequence, and neither is derivable
    /// from the other. `OBCC_Spec.md` §6(c) is what consumes it — a host offering map artifacts must
    /// not offer one this firmware cannot read — which today is the web/desktop builder rather than
    /// this app; the app carries the field so the mirror stays honest about what the wire says, and
    /// so a device dashboard can show it.
    ///
    /// `nil` when the read carried no such byte: a firmware predating the field (a 6-byte read), or
    /// the store-less 2-byte read. **Deliberately not defaulted to `0`** — the same rule as
    /// `storeEpoch`: `0` would read as "supports OBCM v0" and refuse every real map, where `nil`
    /// correctly means unknown.
    public let obcmVersion: UInt8?

    public init(
        name: String,
        firmwareVersion: String,
        hardwareVersion: String = "",
        serial: String = "",
        protocolVersion: UInt16 = OBCProtocol.version,
        storeEpoch: UInt32? = nil,
        obcmVersion: UInt8? = nil
    ) {
        self.name = name
        self.firmwareVersion = firmwareVersion
        self.hardwareVersion = hardwareVersion
        self.serial = serial
        self.protocolVersion = protocolVersion
        self.storeEpoch = storeEpoch
        self.obcmVersion = obcmVersion
    }
}
