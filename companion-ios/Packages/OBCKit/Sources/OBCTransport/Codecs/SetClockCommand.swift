import Foundation
import OBCDomain

/// `setClock` (spec §4.4, command 5): stamp the device's trusted wall clock from the phone.
public enum SetClockCommand {
    public static let commandByte: UInt8 = 5

    /// `cmd u8 · utc u32 LE · offset_min i16 LE`.
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
