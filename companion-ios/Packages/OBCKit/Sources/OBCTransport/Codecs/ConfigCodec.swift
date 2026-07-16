import Foundation
import OBCDomain

// Codecs/ is the home of the **device object layouts** (firmware-`S0`-owned byte
// formats ↔ domain types): the `Config` blob today, the compact-binary route
// encoder and ride decoder when their layouts land. Deliberately outside `BLE/`
// so a device-format change touches a codec file, never the transport class —
// and everything here stays pure + host-testable with no CoreBluetooth.

/// The `Config` object codec — `[nameLen: u16-LE][name UTF-8 ≤ 48 B][units: u8]`,
/// **ratified by firmware S0** (`obc-ble-interface-spec.md` §7.3; pinned against
/// `protocol-vectors/config-v1.bin`). Append-only is the version mechanism: fields
/// are never reordered or resized, and unknown trailing bytes are ignored.
enum ConfigObjectCodec {
    static func encode(_ config: DeviceConfig) -> Data {
        // Cap at the S0 name limit on a Character boundary: an over-cap name would
        // otherwise wrap the u16 length (≥ 65536 → a tiny/zero value) into a
        // corrupt blob the decoder misreads.
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
