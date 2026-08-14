import Foundation
import OBCDomain

/// The `setClock` command encoder (spec §4.4, cmd `5`; see `OBCProtocol.md`):
/// `cmd u8 = 5 · utc u32 LE · offset_min i16 LE`, the phone stamping the device's
/// **trusted wall clock** on every connect (epic #638). The device has no RTC; this
/// (and a GPS fix) is what marks its clock trusted for the boot — the retention
/// sweep's safety gate — so it is sent immediately after encryption, **before** the
/// first `ackRides` / reconcile write.
///
/// Encode-only + `commandResult` correlation, the same shape as
/// [`AckRidesCommand`](AckRidesCommand.swift). A device that predates the command
/// answers `commandResult(unknownCommand)`, which the transport surfaces as
/// `unsupported` (the capability gate S7 hides expiry UI behind).
///
/// Pinned byte-for-byte against `specs/vectors/command-set-clock.bin`
/// (`SetClockCommandTests`), so the app and firmware can't drift from spec §4.4.
public enum SetClockCommand {
    /// The `command` byte (spec §4.4).
    public static let commandByte: UInt8 = 5

    /// Encode a clock sample as the 7-byte `setClock` write, all little-endian.
    public static func encode(_ sample: WallClockSample) -> Data {
        var data = Data([commandByte])
        let utc = sample.utcSeconds
        data.append(UInt8(utc & 0xFF))
        data.append(UInt8((utc >> 8) & 0xFF))
        data.append(UInt8((utc >> 16) & 0xFF))
        data.append(UInt8((utc >> 24) & 0xFF))
        let offset = UInt16(bitPattern: sample.offsetMinutes)
        data.append(UInt8(offset & 0xFF))
        data.append(UInt8((offset >> 8) & 0xFF))
        return data
    }
}
