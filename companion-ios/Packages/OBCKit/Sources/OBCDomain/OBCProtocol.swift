import Foundation

/// The frozen wire-protocol surface, in code form. This is the Swift companion to
/// `companion-ios/OBCProtocol.md`, which is the human-readable reference.
///
/// **Divergence policy:** the firmware `S0` freeze + `obc-ble-interface-spec.md`
/// are canonical. If this and firmware `S0` disagree, firmware `S0` wins and this
/// is corrected — never the other way round.
public enum OBCProtocol {
    /// The `protocol_version` this app build is written against, as read from DIS /
    /// OBC Control. `B1`'s connect path compares the device's reported
    /// `DeviceInfo.protocolVersion` against this; on mismatch it surfaces
    /// `DeviceError.protocolMismatch` (surface, don't crash).
    public static let version: UInt16 = 2

    /// The mismatch to surface for a device reporting `deviceVersion`, or `nil`
    /// when it matches this build (#303). Pure and total — never traps — so the
    /// connect path can compare without a force-unwrap or a decode against an
    /// incompatible object (`OBCProtocol.md` → *Versioning*).
    public static func versionMismatch(reportedBy deviceVersion: UInt16) -> DeviceError? {
        deviceVersion == version ? nil : .protocolMismatch(expected: version, found: deviceVersion)
    }
}
