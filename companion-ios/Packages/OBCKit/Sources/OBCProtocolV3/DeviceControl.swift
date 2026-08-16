import Foundation

/// §16's mount classification. Classes `1` through `6` reproduce the storage contract's table;
/// class `0` is the one case classification never sees, because no medium is present.
public enum MountClass: UInt8, Sendable, CaseIterable {
    case noCard = 0
    case unsupportedFilesystem = 1
    case initializing = 2
    case mounted = 3
    case mountedWithDegradedEntry = 4
    case recoveryFailedReadOnly = 5
    case mountedStoreWideDegraded = 6

    /// §16: "StoreId; zero unless mount class is `3`, `4`, or `6`."
    public var reportsStoreId: Bool {
        self == .mounted || self == .mountedWithDegradedEntry || self == .mountedStoreWideDegraded
    }
}

/// §16's 64-byte GetDeviceStatus response.
public struct DeviceStatus: Hashable, Sendable {
    public struct Flags: OptionSet, Sendable, Hashable {
        public let rawValue: UInt16
        public init(rawValue: UInt16) { self.rawValue = rawValue }
        public static let cardPresent = Flags(rawValue: 1 << 0)
        public static let developerUnlocked = Flags(rawValue: 1 << 1)
        static let defined: UInt16 = 0x0003
    }

    public static let payloadBytes = 64

    public let firmwareMajor: UInt16
    public let firmwareMinor: UInt16
    public let firmwarePatch: UInt16
    public let hardwareRevision: UInt16
    public let serial: DeviceSerial
    public let bootCount: UInt32
    public let uptimeSeconds: UInt64
    public let worstStackHighWaterBytes: UInt32
    public let flags: Flags
    public let mountClass: MountClass
    public let firmwareBuildNumber: UInt32
    public let storeId: StoreId

    public static func decode(_ bytes: [UInt8]) throws -> DeviceStatus {
        try requireExactPayload(bytes.count, payloadBytes, "GetDeviceStatus response")
        var reader = ByteReader(bytes, subject: "GetDeviceStatus response")
        let major = try reader.u16()
        let minor = try reader.u16()
        let patch = try reader.u16()
        let hardware = try reader.u16()
        let serial = DeviceSerial(unchecked: try reader.opaque16())
        let bootCount = try reader.u32()
        let uptime = try reader.u64()
        let stack = try reader.u32()
        let flagsRaw = try reader.u16()
        guard flagsRaw & ~Flags.defined == 0 else {
            throw WireFault.unsupportedFlags("GetDeviceStatus response: status flags \(flagsRaw)")
        }
        let mountRaw = try reader.u8()
        guard let mountClass = MountClass(rawValue: mountRaw) else {
            throw WireFault.unknownEnum("GetDeviceStatus response: mount class \(mountRaw)")
        }
        try reader.reserved(1, "GetDeviceStatus response offset 43")
        let build = try reader.u32()
        let storeId = StoreId(unchecked: try reader.opaque16())
        guard mountClass.reportsStoreId || storeId.isZero else {
            throw WireFault.reservedBits(
                "GetDeviceStatus response: StoreId in mount class \(mountRaw)")
        }
        return DeviceStatus(
            firmwareMajor: major, firmwareMinor: minor, firmwarePatch: patch,
            hardwareRevision: hardware, serial: serial, bootCount: bootCount,
            uptimeSeconds: uptime, worstStackHighWaterBytes: stack,
            flags: Flags(rawValue: flagsRaw), mountClass: mountClass, firmwareBuildNumber: build,
            storeId: storeId)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u16(firmwareMajor)
        writer.u16(firmwareMinor)
        writer.u16(firmwarePatch)
        writer.u16(hardwareRevision)
        writer.raw(serial.bytes)
        writer.u32(bootCount)
        writer.u64(uptimeSeconds)
        writer.u32(worstStackHighWaterBytes)
        writer.u16(flags.rawValue)
        writer.u8(mountClass.rawValue)
        writer.u8(0)
        writer.u32(firmwareBuildNumber)
        writer.raw(storeId.bytes)
        return writer.bytes
    }
}

/// §16's 56-byte config block. Whole and fixed: there is no absent field and no
/// absent-means-leave-untouched rule.
public struct DeviceConfigBlock: Hashable, Sendable {
    public struct UnitFlags: OptionSet, Sendable, Hashable {
        public let rawValue: UInt8
        public init(rawValue: UInt8) { self.rawValue = rawValue }
        public static let imperial = UnitFlags(rawValue: 1 << 0)
        public static let fahrenheit = UnitFlags(rawValue: 1 << 1)
        public static let twelveHourClock = UnitFlags(rawValue: 1 << 2)
        static let defined: UInt8 = 0x07
    }

    public enum WeatherRefresh: UInt8, Sendable, CaseIterable {
        case off = 0
        case fifteenMinutes = 1
        case thirtyMinutes = 2
        case sixtyMinutes = 3
        case oneHundredTwentyMinutes = 4
    }

    public static let payloadBytes = 56

    public let codecVersion: UInt8
    public let unitFlags: UnitFlags
    public let weatherRefresh: WeatherRefresh
    /// The exact name bytes; a zero length means the device advertises its factory default name
    /// rather than an empty one.
    public let nameBytes: [UInt8]

    /// SetConfig is a whole-block write, so a client reads the current block, edits it, and writes
    /// it back — which needs a public initializer. Over-long names are refused at `encoded()`
    /// rather than here, so the failure arrives with the rest of the framing bounds.
    public init(
        codecVersion: UInt8 = 1, unitFlags: UnitFlags, weatherRefresh: WeatherRefresh,
        nameBytes: [UInt8]
    ) {
        self.codecVersion = codecVersion
        self.unitFlags = unitFlags
        self.weatherRefresh = weatherRefresh
        self.nameBytes = nameBytes
    }

    public var name: String? {
        nameBytes.isEmpty ? nil : String(decoding: nameBytes, as: UTF8.self)
    }

    public static func decode(_ bytes: [UInt8]) throws -> DeviceConfigBlock {
        try requireExactPayload(bytes.count, payloadBytes, "config block")
        var reader = ByteReader(bytes, subject: "config block")
        let codecVersion = try reader.u8()
        let blockLength = try reader.u8()
        guard codecVersion == 1 else {
            throw WireFault.invalidCombination("config block: codec version \(codecVersion)")
        }
        guard blockLength == UInt8(payloadBytes) else {
            throw WireFault.invalidCombination("config block: block length \(blockLength)")
        }
        let flags = try reader.u16()
        guard flags == 0 else { throw WireFault.reservedBits("config block: flags") }
        let nameLength = Int(try reader.u8())
        let unitRaw = try reader.u8()
        let refreshRaw = try reader.u8()
        try reader.reserved(1, "config block byte 7")
        let nameField = Array(try reader.take(32))
        try reader.reserved(16, "config block byte 40")

        guard nameLength <= 32 else {
            throw WireFault.invalidCombination("config block: name length \(nameLength)")
        }
        guard unitRaw & ~UnitFlags.defined == 0 else {
            throw WireFault.unsupportedFlags("config block: unit flags \(unitRaw)")
        }
        guard let refresh = WeatherRefresh(rawValue: refreshRaw) else {
            throw WireFault.unknownEnum("config block: weather refresh \(refreshRaw)")
        }
        // §16: "a nonzero byte at or beyond the stated length is invalidDescriptor."
        guard nameField[nameLength...].allSatisfy({ $0 == 0 }) else {
            throw WireFault.reservedBits("config block: nonzero byte beyond the name length")
        }
        let nameBytes = Array(nameField[0..<nameLength])
        if !nameBytes.isEmpty {
            // §16: name bytes obey §2.2's text rules.
            _ = try WireText.validate(nameBytes, subject: "config block name")
        }
        return DeviceConfigBlock(
            codecVersion: codecVersion, unitFlags: UnitFlags(rawValue: unitRaw),
            weatherRefresh: refresh, nameBytes: nameBytes)
    }

    public func encoded() throws -> [UInt8] {
        try requireAtMost(nameBytes.count, 32, "config block: device name")
        var writer = ByteWriter()
        writer.u8(codecVersion)
        writer.u8(UInt8(Self.payloadBytes))
        writer.u16(0)
        writer.u8(try narrowU8(nameBytes.count, "config block: name length"))
        writer.u8(unitFlags.rawValue)
        writer.u8(weatherRefresh.rawValue)
        writer.u8(0)
        writer.raw(nameBytes)
        writer.zeros(32 - nameBytes.count)
        writer.zeros(16)
        return writer.bytes
    }
}

/// §16's SetClock source. Which sources a device trusts is its own policy.
public enum ClockSource: UInt8, Sendable, CaseIterable {
    case companion = 1
    case gps = 2
}

/// §16's 16-byte SetClock request.
public struct SetClockRequest: Hashable, Sendable {
    public let epochSeconds: Int64
    public let source: ClockSource

    public static func decode(_ bytes: [UInt8]) throws -> SetClockRequest {
        try requireExactPayload(bytes.count, 16, "SetClock")
        var reader = ByteReader(bytes, subject: "SetClock")
        let epoch = try reader.i64()
        let raw = try reader.u8()
        guard let source = ClockSource(rawValue: raw) else {
            throw WireFault.unknownEnum("SetClock: source \(raw)")
        }
        try reader.reserved(7, "SetClock offset 9")
        return SetClockRequest(epochSeconds: epoch, source: source)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.i64(epochSeconds)
        writer.u8(source.rawValue)
        writer.zeros(7)
        return writer.bytes
    }
}

/// §16's 16-byte SetClock response: the clock *after* the request, the source now trusted, and the
/// clock state. This is how a client learns whether a reissue was applied or refused.
public struct ClockStatus: Hashable, Sendable {
    public enum State: UInt8, Sendable, CaseIterable {
        case untrusted = 0
        case trusted = 1
    }

    public let epochSeconds: Int64
    /// Zero when no source is trusted yet.
    public let rawSource: UInt8
    public let state: State

    public var source: ClockSource? { ClockSource(rawValue: rawSource) }

    public static func decode(_ bytes: [UInt8]) throws -> ClockStatus {
        try requireExactPayload(bytes.count, 16, "SetClock response")
        var reader = ByteReader(bytes, subject: "SetClock response")
        let epoch = try reader.i64()
        let rawSource = try reader.u8()
        guard rawSource <= ClockSource.gps.rawValue else {
            throw WireFault.unknownEnum("SetClock response: source \(rawSource)")
        }
        let stateRaw = try reader.u8()
        guard let state = State(rawValue: stateRaw) else {
            throw WireFault.unknownEnum("SetClock response: clock state \(stateRaw)")
        }
        try reader.reserved(6, "SetClock response offset 10")
        return ClockStatus(epochSeconds: epoch, rawSource: rawSource, state: state)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.i64(epochSeconds)
        writer.u8(rawSource)
        writer.u8(state.rawValue)
        writer.zeros(6)
        return writer.bytes
    }
}

/// §16's 8-byte ForgetBond request. BLE-only: on any other link kind the device answers
/// `unsupportedCapability/opcode`, which follows from the cleared command-flag bit rather than from
/// the request's contents.
public struct ForgetBondRequest: Hashable, Sendable {
    public enum Scope: UInt8, Sendable, CaseIterable {
        case thisBond = 1
        case everyBond = 2
    }

    public let scope: Scope

    public static func decode(_ bytes: [UInt8]) throws -> ForgetBondRequest {
        try requireExactPayload(bytes.count, 8, "ForgetBond")
        var reader = ByteReader(bytes, subject: "ForgetBond")
        let raw = try reader.u8()
        guard let scope = Scope(rawValue: raw) else {
            throw WireFault.unknownEnum("ForgetBond: scope \(raw)")
        }
        try reader.reserved(7, "ForgetBond offset 1")
        return ForgetBondRequest(scope: scope)
    }

    public func encoded() throws -> [UInt8] {
        var writer = ByteWriter()
        writer.u8(scope.rawValue)
        writer.zeros(7)
        return writer.bytes
    }
}

/// §16's ResetStore echo. The confirmation is checked before anything is deleted, so the check is a
/// device-side predicate over the request and the device's own reported state, not a codec rule.
public enum ResetStoreEcho {
    /// §16: the echo "MUST equal the StoreId the device currently reports"; the all-zero form is
    /// admitted only in the two classes that report no StoreId at all — initializing `2` and
    /// recovery-failed `5`.
    public static func validate(echo: StoreId, currentStoreId: StoreId, mountClass: MountClass) throws {
        let reportsStoreId = mountClass.reportsStoreId
        if reportsStoreId {
            guard echo == currentStoreId, !echo.isZero else {
                throw WireFault.invalidCombination(
                    "ResetStore: echoed StoreId does not match the reported one")
            }
        } else {
            guard echo.isZero else {
                throw WireFault.invalidCombination(
                    "ResetStore: nonzero echo in a class that reports no StoreId")
            }
        }
    }
}
