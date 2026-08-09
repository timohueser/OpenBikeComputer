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
        // The byte goes out **raw**, so a value read back from a newer device survives a
        // read-modify-write (a rename) instead of being flattened to something we do know.
        if let refresh = config.weatherRefreshRaw { data.append(refresh) }
        return data
    }

    static func decode(_ data: Data) throws -> DeviceConfig {
        guard data.count >= 2 else { throw DeviceError.readFailed }
        let b = data.startIndex
        let nameLen = Int(data[b]) | (Int(data[b + 1]) << 8)
        guard data.count >= 2 + nameLen + 1 else { throw DeviceError.readFailed }
        let name = String(decoding: data[(b + 2)..<(b + 2 + nameLen)], as: UTF8.self)
        let units = DeviceConfig.Units(rawValue: data[b + 2 + nameLen]) ?? .metric
        // The refresh byte is optional and positional, and is **never validated here**, whichever
        // value it carries: decoding is direction-blind, and §11.8 makes an unknown interval fatal
        // in exactly one direction. A device applying a write calls `weatherRefreshToApply()` and
        // refuses; this app reading a device calls `knownWeatherRefresh` and sees `nil`. Rejecting
        // the whole blob here would take the strict rule to both, which is how appending a fifth
        // interval would one day stop a shipped app from so much as renaming its device.
        //
        // Bytes past it are ignored (the append-only rule), so a future field cannot make this
        // build refuse the blob either.
        let refreshIndex = b + 3 + nameLen
        let weatherRefreshRaw = refreshIndex < data.endIndex ? data[refreshIndex] : nil
        return DeviceConfig(name: name, units: units, weatherRefreshRaw: weatherRefreshRaw)
    }
}

extension DeviceConfig {
    /// The refresh interval a **device must store for this write**, or a refusal — the strict half
    /// of §11.8, mirroring `obc_ble::descriptor::Config::refresh_to_apply`.
    ///
    /// `nil` means the writer said nothing about refresh, and the device MUST leave whatever it has
    /// stored alone rather than reset it to the default — an old app renaming the device must not
    /// switch a rider who deliberately chose `.off` back on. A throw means the writer named an
    /// interval the device cannot honour; substituting anything would report a setting the rider
    /// never chose back to them as if it had taken effect.
    ///
    /// This is the **only** place in the app an unknown refresh byte is fatal. It lives here rather
    /// than beside the stored byte in `OBCDomain` because the refusal it throws is the wire-level
    /// `WeatherRequestError.unknownRefresh`, the exact mirror of Rust's
    /// `DescriptorError::UnknownRefresh`.
    public func weatherRefreshToApply() throws -> WeatherRefresh? {
        guard let raw = weatherRefreshRaw else { return nil }
        guard let refresh = WeatherRefresh(wireByte: raw) else {
            throw WeatherRequestError.unknownRefresh(raw)
        }
        return refresh
    }
}
