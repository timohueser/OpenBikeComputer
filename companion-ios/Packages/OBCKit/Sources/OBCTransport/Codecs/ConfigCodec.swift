import Foundation
import OBCDomain

// Codecs/ holds the device object layouts: the byte formats the firmware owns, mapped to
// domain types. It sits outside `BLE/` so a device-format change touches a codec file and not
// the transport class, and everything here stays pure and host-testable with no CoreBluetooth.

enum ConfigObjectCodec {
    static func encode(_ config: DeviceConfig) -> Data {
        // Cap at the name limit on a Character boundary: an over-cap name would wrap the u16
        // length field into a corrupt blob the decoder misreads.
        let name = Data(config.name.truncatedToUTF8Bytes(DeviceConfig.maxNameUTF8Bytes).utf8)
        var data = Data()
        data.append(UInt8(name.count & 0xFF))
        data.append(UInt8((name.count >> 8) & 0xFF))
        data.append(name)
        data.append(config.units.rawValue)

        return data
    }

    static func decode(_ data: Data) throws -> DeviceConfig {
        guard data.count >= 2 else { throw DeviceError.readFailed }
        let b = data.startIndex
        let nameLen = Int(data[b]) | (Int(data[b + 1]) << 8)
        guard data.count >= 2 + nameLen + 1 else { throw DeviceError.readFailed }
        let name = String(decoding: data[(b + 2)..<(b + 2 + nameLen)], as: UTF8.self)
        let units = DeviceConfig.Units(rawValue: data[b + 2 + nameLen]) ?? .metric

        return DeviceConfig(name: name, units: units)
    }
}
