import Foundation

/// The frozen wire-protocol surface, in code form. `specs/obc-ble-interface-spec.md` is
/// canonical: if this and the firmware disagree, the firmware wins and this is corrected.
public enum OBCProtocol {
    /// The `protocol_version` this app build is written against. The connect path compares the
    /// device's reported version against it and surfaces `DeviceError.protocolMismatch`.
    public static let version: UInt16 = 4

    /// The mismatch to surface for a device reporting `deviceVersion`, or `nil` when it matches
    /// this build. Pure and total, so the connect path can compare with no force-unwrap.
    public static func versionMismatch(reportedBy deviceVersion: UInt16) -> DeviceError? {
        deviceVersion == version ? nil : .protocolMismatch(expected: version, found: deviceVersion)
    }

}
