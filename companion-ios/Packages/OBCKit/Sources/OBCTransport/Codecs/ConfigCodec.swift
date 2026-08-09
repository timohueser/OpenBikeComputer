import Foundation
import OBCDomain

// Codecs/ is the home of the **device object layouts** (firmware-`S0`-owned byte
// formats ↔ domain types): the `Config` blob today, the compact-binary route
// encoder and ride decoder when their layouts land. Deliberately outside `BLE/`
// so a device-format change touches a codec file, never the transport class —
// and everything here stays pure + host-testable with no CoreBluetooth.

/// The `Config` object codec — `[nameLen: u16-LE][name UTF-8 ≤ 48 B][units: u8]`
/// plus the optional trailing `[weatherRefresh: u8]` (WX3 / #1188),
/// **ratified by firmware S0** (`obc-ble-interface-spec.md` §7.3; pinned against
/// `specs/vectors/config-v1.bin`). Append-only is the version mechanism: fields
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
        // A `nil` refresh writes the 3-byte-plus-name v1 blob, byte-identical to what a pre-WX3
        // build produced — which is what keeps `config-v1.bin` a meaningful pin, and what lets a
        // caller that never touched weather write Config back without asserting a setting.
        if let refresh = config.weatherRefresh { data.append(refresh.rawValue) }
        return data
    }

    static func decode(_ data: Data) throws -> DeviceConfig {
        guard data.count >= 2 else { throw DeviceError.readFailed }
        let b = data.startIndex
        let nameLen = Int(data[b]) | (Int(data[b + 1]) << 8)
        guard data.count >= 2 + nameLen + 1 else { throw DeviceError.readFailed }
        let name = String(decoding: data[(b + 2)..<(b + 2 + nameLen)], as: UTF8.self)
        let units = DeviceConfig.Units(rawValue: data[b + 2 + nameLen]) ?? .metric
        // The refresh byte is optional and positional: absent means the writer never mentioned
        // refresh (→ device default), present-but-unknown means it asked for an interval this build
        // cannot honour. The second is **malformed**, not a default — storing it as 30 minutes would
        // tell the rider a choice was applied that was in fact discarded. Bytes past it are ignored
        // (the append-only rule), so a future field cannot make this build refuse the blob.
        let refreshIndex = b + 3 + nameLen
        var weatherRefresh: WeatherRefresh?
        if refreshIndex < data.endIndex {
            guard let refresh = WeatherRefresh(wireByte: data[refreshIndex]) else {
                throw DeviceError.readFailed
            }
            weatherRefresh = refresh
        }
        return DeviceConfig(name: name, units: units, weatherRefresh: weatherRefresh)
    }
}
