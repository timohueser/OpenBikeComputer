import Foundation

/// The frozen wire-protocol surface, in code form. The canonical reference is
/// `specs/obc-ble-interface-spec.md`; `companion-ios/OBCProtocol.md` contains
/// implementation notes for this package.
///
/// **Divergence policy:** the firmware `S0` freeze + `obc-ble-interface-spec.md`
/// are canonical. If this and firmware `S0` disagree, firmware `S0` wins and this
/// is corrected — never the other way round.
public enum OBCProtocol {
    /// The `protocol_version` this app build is written against, as read from DIS /
    /// OBC Control. `B1`'s connect path compares the device's reported
    /// `DeviceInfo.protocolVersion` against this; on mismatch it surfaces
    /// `DeviceError.protocolMismatch` (surface, don't crash).
    public static let version: UInt16 = 4

    /// The mismatch to surface for a device reporting `deviceVersion`, or `nil`
    /// when it matches this build (#303). Pure and total — never traps — so the
    /// connect path can compare without a force-unwrap or a decode against an
    /// incompatible object (`OBCProtocol.md` → *Versioning*).
    public static func versionMismatch(reportedBy deviceVersion: UInt16) -> DeviceError? {
        deviceVersion == version ? nil : .protocolMismatch(expected: version, found: deviceVersion)
    }

    /// The device implements the **Weather Request** contract (spec §11): the secondary service,
    /// the request context, protocol-v4 weather object and the Config refresh field — bit 0 of the identity
    /// read's trailing capability word (WX3 / #1188).
    ///
    /// One bit covers all four because they are useless apart: a phone that can read a request but
    /// cannot upload the answer has nothing to offer. The word is append-only in the same sense the
    /// read is, and **unknown bits are ignored** — a future firmware announcing a feature this
    /// build never heard of must not mask the one beside it.
    public static let featureWeather: UInt32 = 1 << 0
}
