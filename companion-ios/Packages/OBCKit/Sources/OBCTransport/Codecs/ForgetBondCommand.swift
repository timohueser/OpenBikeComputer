import Foundation

/// `forgetBond` (spec §4.4, command 4): ask the authenticated device to clear its bond.
public enum ForgetBondCommand {
    public static let commandByte: UInt8 = 4

    public static func encode() -> Data { Data([commandByte]) }
}
