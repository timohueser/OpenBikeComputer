import Foundation
import Testing
import OBCDomain
@testable import OBCTransport
#if canImport(CoreBluetooth)
import CoreBluetooth
#endif

// The Weather Request contract (spec §11, WX3 / #1188) — the Swift half of the mirror. The firmware
// suite `obc-ble/tests/weather_request.rs` pins the same behaviours against the same bytes; the two
// compatibility suites at the bottom are #1188's acceptance criteria and are written as the bytes
// each side actually puts on the wire, never through a shared helper that could make both sides
// wrong in the same direction.

/// A fully-populated context — every optional group present, so a round-trip exercises every field.
private func fullContext() -> WeatherRequestContext {
    WeatherRequestContext(
        validity: [.position, .bearing, .speed, .bundle, .route],
        reason: [.scheduled, .retry],
        refreshRaw: WeatherRefresh.every30.rawValue,
        requestID: 0xDEAD_BEEF,
        // Freiburg im Breisgau, in the OBCW header's microdegrees.
        latitudeMicrodegrees: 47_999_008,
        longitudeMicrodegrees: 7_842_104,
        fixUTCSeconds: 1_800_000_000,
        bearingWireDegrees: 217,
        speedDeciMetersPerSecond: 58,
        routeWireID: 4242,
        bundleWireGeneration: 7,
        bundleWireGeneratedAtSeconds: 1_799_996_400,
        bundleWireCRC32: 0x1234_5678
    )
}

@Suite("Weather request context codec")
struct WeatherRequestContextTests {
    #if canImport(CoreBluetooth)
    @Test func transportUUIDsArePinned() {
        #expect(GATT.weatherRequestService.uuidString == "B3B60000-33B4-4F02-A5FF-E5954D54B5AA")
        #expect(GATT.weatherRequestContext.uuidString == "B3B60001-33B4-4F02-A5FF-E5954D54B5AA")
    }
    #endif

    @Test func contextRoundTripsEveryField() throws {
        let context = fullContext()
        let bytes = context.encode()
        #expect(bytes.count == WeatherRequestContext.encodedLength)
        #expect(try WeatherRequestContext(decoding: bytes) == context)
    }

    @Test func contextDeclaresItsOwnLengthInByteOne() {
        let bytes = fullContext().encode()
        #expect(bytes[0] == WeatherRequestContext.currentVersion)
        #expect(Int(bytes[1]) == WeatherRequestContext.encodedLength)
    }

    /// Spot-check the offsets the spec table names, so a field reorder cannot pass as a round-trip.
    @Test func contextIsLittleEndianAtThePinnedOffsets() {
        let bytes = [UInt8](fullContext().encode())
        #expect(Array(bytes[2..<4]) == [0x1F, 0x00], "validity at 2 (bits 0…4)")
        #expect(Array(bytes[4..<6]) == [0x05, 0x00], "reason at 4 (scheduled | retry)")
        #expect(bytes[6] == WeatherRefresh.every30.rawValue, "refresh at 6")
        #expect(Array(bytes[8..<12]) == [0xEF, 0xBE, 0xAD, 0xDE], "request_id at 8")
        #expect(Array(bytes[12..<16]) == le32(UInt32(bitPattern: 47_999_008)), "lat_udeg at 12")
        #expect(Array(bytes[16..<20]) == le32(UInt32(bitPattern: 7_842_104)), "lon_udeg at 16")
        #expect(Array(bytes[20..<28]) == le64(UInt64(bitPattern: 1_800_000_000)), "fix_utc at 20")
        #expect(Array(bytes[28..<30]) == [217, 0], "bearing_deg at 28")
        #expect(Array(bytes[30..<32]) == [58, 0], "speed_deci_ms at 30")
        #expect(Array(bytes[32..<34]) == [0x92, 0x10], "route_id at 32")
        #expect(Array(bytes[36..<40]) == le32(7), "bundle_generation at 36")
        #expect(Array(bytes[40..<48]) == le64(UInt64(bitPattern: 1_799_996_400)), "bundle_generated_at at 40")
        #expect(Array(bytes[48..<52]) == le32(0x1234_5678), "bundle_crc32 at 48")
    }

    @Test func reservedBytesAreWrittenZero() {
        let bytes = [UInt8](fullContext().encode())
        #expect(bytes[7] == 0, "reserved0")
        #expect(Array(bytes[34..<36]) == [0, 0], "reserved1")
    }

    /// A read taken out of turn must not imply a fix, a bundle, or a reason to act.
    @Test func theEmptyRestingValueClaimsNothing() throws {
        let decoded = try WeatherRequestContext(decoding: WeatherRequestContext.empty.encode())
        #expect(decoded == WeatherRequestContext.empty)
        #expect(decoded.validity.isEmpty)
        #expect(decoded.reason.isEmpty)
        #expect(decoded.fix == nil)
        #expect(decoded.bundle == nil)
        #expect(decoded.routeID == nil)
        #expect(decoded.bearingDegrees == nil)
        #expect(decoded.speedMetersPerSecond == nil)
        #expect(decoded.refresh == .deviceDefault, "the resting value still reports the real setting")
    }

    /// Every short read is refused rather than half-decoded. `obc_ble` folds both refusals into one
    /// `Truncated`; Swift splits them (`.truncated` = not even the prefix, `.invalidLength` = byte 1
    /// and the read disagree) and the *rejection* is what both sides pin.
    @Test func everyTruncationIsRejectedRatherThanHalfDecoded() {
        let bytes = fullContext().encode()
        for length in 0..<WeatherRequestContext.encodedLength {
            let short = bytes.prefix(length)
            let expected: WeatherRequestError =
                length < WeatherRequestContext.minimumEncodedLength ? .truncated : .invalidLength
            #expect(throws: expected, "a \(length)-byte read must not decode") {
                try WeatherRequestContext(decoding: short)
            }
        }
        #expect(throws: Never.self) { try WeatherRequestContext(decoding: bytes) }
    }

    /// A writer that claims fewer bytes than v1 defines is not an *old* writer — v1 is the first
    /// version — so it is malformed rather than something to decode leniently.
    @Test func aDeclaredLengthBelowV1IsRejected() {
        var bytes = [UInt8](fullContext().encode())
        bytes[1] = UInt8(WeatherRequestContext.encodedLength - 1)
        #expect(throws: WeatherRequestError.invalidLength) {
            try WeatherRequestContext(decoding: Data(bytes))
        }
    }

    /// The writer said 60 bytes and 52 arrived: a short *read*, not a short value.
    @Test func aDeclaredLengthLongerThanTheReadIsRejected() {
        var bytes = [UInt8](fullContext().encode())
        bytes[1] = 60
        #expect(throws: WeatherRequestError.invalidLength) {
            try WeatherRequestContext(decoding: Data(bytes))
        }
    }

    /// The append-only promise in the direction that matters: tomorrow's firmware appends a field,
    /// and today's shipped app keeps reading the request it understands.
    @Test func aFutureLongerContextStillDecodes() throws {
        let context = fullContext()
        var future = [UInt8](context.encode()) + [UInt8](repeating: 0xAA, count: 8)
        future[1] = UInt8(future.count)  // the future writer declares its own longer length…
        future[0] = 2                    // …and its own version

        let decoded = try WeatherRequestContext(decoding: Data(future))
        #expect(decoded.version == 2, "the version is reported, not normalised away")
        #expect(decoded.requestID == context.requestID)
        #expect(decoded.bundle?.crc32 == context.bundleWireCRC32, "the last v1 field survives the append")
    }

    @Test func unknownValidityAndReasonBitsAreIgnoredNotRejected() throws {
        var context = fullContext()
        context.validity.insert(WeatherRequestValidity(rawValue: 1 << 15))
        context.reason.insert(WeatherRequestReason(rawValue: 1 << 14))

        let decoded = try WeatherRequestContext(decoding: context.encode())
        #expect(decoded.fix != nil, "a known bit still reads through an unknown neighbour")
        #expect(decoded.reason.contains(.scheduled))
        #expect(decoded.validity == context.validity, "unknown bits are preserved verbatim")
    }

    /// Absence is a cleared flag, never a sentinel — an unset position must not be mistakable for
    /// the equator, and an unset bundle must not read as generation 0.
    @Test func absentGroupsReadAsAbsentNotAsZeroWithMeaning() throws {
        // Wire values that would be dangerous if read without their flag: 0,0 is a real point in the
        // Gulf of Guinea, and generation 0 is a real first bundle.
        let context = WeatherRequestContext(
            validity: [],
            reason: [.noBundle],
            latitudeMicrodegrees: 0,
            longitudeMicrodegrees: 0,
            bundleWireGeneration: 0,
            bundleWireCRC32: 0
        )
        let decoded = try WeatherRequestContext(decoding: context.encode())
        #expect(decoded.fix == nil, "no fix must not read as the equator")
        #expect(decoded.bundle == nil, "no bundle must not read as generation 0")
        #expect(decoded.routeID == nil)
        #expect(decoded.reason.contains(.noBundle))
    }

    /// Cold start indoors: no GPS yet, but the rider opened Weather. It remains well-formed for
    /// diagnostics/retry even though the companion returns `noPosition` until a device fix exists.
    @Test func aContextWithNoFixStillCarriesADiagnosticRequest() throws {
        let context = WeatherRequestContext(reason: [.urgent, .noBundle], requestID: 1)
        let decoded = try WeatherRequestContext(decoding: context.encode())
        #expect(decoded.requestID == 1)
        #expect(decoded.reason.contains(.urgent))
        #expect(decoded.fix == nil)
    }

    @Test func populatedGroupsDecodeThroughTheirAccessors() throws {
        let decoded = try WeatherRequestContext(decoding: fullContext().encode())
        let fix = try #require(decoded.fix)
        #expect(fix.latitudeMicrodegrees == 47_999_008)
        #expect(fix.latitude == 47.999008)
        #expect(fix.longitude == 7.842104)
        #expect(fix.utc == Date(timeIntervalSince1970: 1_800_000_000))
        #expect(decoded.bearingDegrees == 217)
        #expect(decoded.speedMetersPerSecond == 5.8)
        #expect(decoded.routeID == 4242)
        let bundle = try #require(decoded.bundle)
        #expect(bundle.generation == 7)
        #expect(bundle.generatedAt == Date(timeIntervalSince1970: 1_799_996_400))
        #expect(bundle.crc32 == 0x1234_5678)
    }
}

@Suite("Weather unchanged command")
struct WeatherUnchangedCommandTests {
    @Test func encodingIsTheFixedLittleEndianSevenByteShape() {
        #expect(
            WeatherUnchangedCommand.encode(
                requestID: 0x7856_3412, retryAfterSeconds: 120
            ) == Data([7, 0x12, 0x34, 0x56, 0x78, 120, 0]))
        #expect(WeatherRequestReason.hourlyOnly.rawValue == 1 << 5)
    }
}

@Suite("Weather refresh interval")
struct WeatherRefreshTests {
    @Test(arguments: [
        (WeatherRefresh.off, UInt8(0), Int?.none),
        (.every15, 1, 15),
        (.every30, 2, 30),
        (.every60, 3, 60),
        (.every120, 4, 120),
    ])
    func theFiveValuesMapToTheirByteAndMinutes(refresh: WeatherRefresh, byte: UInt8, minutes: Int?) {
        #expect(refresh.rawValue == byte)
        #expect(WeatherRefresh(wireByte: byte) == refresh)
        #expect(refresh.minutes == minutes)
    }

    @Test func offHasNoIntervalRatherThanAZeroOne() {
        #expect(WeatherRefresh.off.minutes == nil, "a caller scheduling on a 0 would spin")
        #expect(WeatherRefresh.deviceDefault == .every30, "epic #1185 locks 30 minutes")
    }

    /// An unknown byte is an interval this build cannot honour — reporting the default back would
    /// tell the rider a setting was applied that was in fact discarded.
    @Test func anOutOfRangeValueIsRejectedRatherThanDefaulted() {
        for byte in UInt8(5)...UInt8(255) {
            #expect(WeatherRefresh(wireByte: byte) == nil, "\(byte) must not decode")
        }
    }

    /// §11.8's read direction, on the context. Appending a fifth interval is an ordinary enum
    /// append; under a direction-blind reject it would instead break every *shipped* app against
    /// new firmware — the context read would throw and weather would be permanently dead. So an
    /// unrecognised refresh byte rides through verbatim and reads as *unknown*, exactly like an
    /// unrecognised `reason` bit.
    @Test func anUnknownRefreshByteReadsAsUnknownRatherThanFailing() throws {
        var bytes = [UInt8](fullContext().encode())
        bytes[6] = 9

        let decoded = try WeatherRequestContext(decoding: Data(bytes))
        #expect(decoded.refresh == nil, "unknown — and specifically not Off and not the default")
        #expect(decoded.refresh != .off)
        #expect(decoded.refresh != .deviceDefault)
        #expect(decoded.refreshRaw == 9, "the byte survives verbatim")
        #expect([UInt8](decoded.encode()) == bytes, "and round-trips unchanged")
        // Everything else in the read still decodes — one unknown byte must not cost the whole
        // request, which is the entire point of tolerating it.
        #expect(decoded.requestID == fullContext().requestID)
        #expect(decoded.fix != nil)
        #expect(decoded.bundle?.crc32 == 0x1234_5678)
    }

    @Test func aKnownRefreshByteReadsAsThatInterval() throws {
        #expect(try WeatherRequestContext(decoding: fullContext().encode()).refresh == .every30)
    }
}

@Suite("Weather bundle object type")
struct WeatherBundleObjectTypeTests {
    @Test func theWeatherBundleTypeIsTwenty() throws {
        #expect(ObjectType.weatherBundle.rawValue == 20)
        #expect(ObjectType(rawValue: 20) == .weatherBundle)

        // …and it survives a real 12-byte descriptor round-trip at the singleton id.
        let control = TransferControl(
            op: .upload, type: .weatherBundle, objectID: 0, totalLen: 46_000, crc32: 0xFEED_FACE
        )
        let bytes = control.encode()
        #expect(bytes[bytes.startIndex + 1] == 20)
        #expect(try TransferControl(decoding: bytes) == control)
    }

    /// The sensor band stays reserved (M4) and 21 is not allocated — a byte the app does not know
    /// must not silently become a type it does.
    @Test func theReservedAndUnallocatedBandsStillReject() {
        for reserved in UInt8(11)...UInt8(15) {
            #expect(ObjectType(rawValue: reserved) == nil, "\(reserved) stays reserved for sensors")
        }
        #expect(ObjectType(rawValue: 21) == nil)
    }
}

// MARK: - Compatibility: #1188's acceptance criteria

@Suite("Weather capability compatibility")
struct WeatherCapabilityCompatibilityTests {
    /// The identity read as `BLETransport.deviceInfo()` decodes it — transcribed rather than called,
    /// because the transport's copy needs a live CoreBluetooth read. Kept byte-for-byte in step with
    /// it; `ProtocolVectorTests` pins both against the checked-in vectors.
    private func decodeIdentity(_ bytes: Data) -> (version: UInt16, epoch: UInt32?, obcm: UInt8?, features: UInt32?) {
        let b = bytes.startIndex
        let version = bytes.count >= 2 ? UInt16(bytes[b]) | (UInt16(bytes[b + 1]) << 8) : OBCProtocol.version
        let epoch: UInt32? = bytes.count >= 6
            ? UInt32(bytes[b + 2]) | (UInt32(bytes[b + 3]) << 8)
                | (UInt32(bytes[b + 4]) << 16) | (UInt32(bytes[b + 5]) << 24)
            : nil
        let obcm: UInt8? = bytes.count >= 7 ? bytes[b + 6] : nil
        let features: UInt32? = bytes.count >= 11
            ? UInt32(bytes[b + 7]) | (UInt32(bytes[b + 8]) << 8)
                | (UInt32(bytes[b + 9]) << 16) | (UInt32(bytes[b + 10]) << 24)
            : nil
        return (version, epoch, obcm, features)
    }

    /// A legacy extended identity fixture carried forward for weather compatibility checks.
    /// features = weather.
    private var newFirmwareRead: Data {
        Data([0x04, 0x00, 0xD4, 0xC3, 0xB2, 0xA1, 12, 0x01, 0x00, 0x00, 0x00])
    }

    /// **Old app ↔ new firmware.** A shipped app decodes the identity read with the pre-WX3 rules
    /// (`>= 6` bytes, take byte 6 if present, ignore the rest) and must survive the widened read —
    /// same version, same epoch, same map version, and **no mismatch path**.
    @Test func oldAppReadsTheWidenedIdentityReadWithoutNoticing() {
        let wire = newFirmwareRead
        #expect(wire.count == 11, "a weather device serves the full read")

        // The pre-WX3 decoder, transcribed — it never looks past byte 6.
        let b = wire.startIndex
        let oldVersion = UInt16(wire[b]) | (UInt16(wire[b + 1]) << 8)
        let oldEpoch = UInt32(wire[b + 2]) | (UInt32(wire[b + 3]) << 8)
            | (UInt32(wire[b + 4]) << 16) | (UInt32(wire[b + 5]) << 24)
        let oldObcm: UInt8? = wire.count >= 7 ? wire[b + 6] : nil

        #expect(oldVersion == OBCProtocol.version, "no protocol bump — no mismatch banner")
        #expect(OBCProtocol.versionMismatch(reportedBy: oldVersion) == nil)
        #expect(oldEpoch == 0xA1B2_C3D4)
        #expect(oldObcm == 12)
    }

    /// **New app ↔ old firmware.** A 7-byte (pre-WX3) and a 6-byte (pre-E1) read must decode as
    /// *no weather capability* — absent, never fabricated — and must not offer weather.
    @Test func newAppReadsOldFirmwareAsHavingNoWeather() {
        let preWX3 = Data([0x02, 0x00, 0x09, 0x00, 0x00, 0x00, 12])
        let decodedPreWX3 = decodeIdentity(preWX3)
        #expect(decodedPreWX3.obcm == 12)
        #expect(decodedPreWX3.features == nil, "absent, never 0")
        #expect(!DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: decodedPreWX3.features).supportsWeather)

        let preE1 = Data([0x02, 0x00, 0x09, 0x00, 0x00, 0x00])
        let decodedPreE1 = decodeIdentity(preE1)
        #expect(decodedPreE1.epoch == 9, "the epoch is present, so the ack gate stays open")
        #expect(decodedPreE1.obcm == nil)
        #expect(decodedPreE1.features == nil)
        #expect(!DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: decodedPreE1.features).supportsWeather)
    }

    @Test func allFourIdentityReadLengthsDecode() {
        #expect(decodeIdentity(newFirmwareRead).features == OBCProtocol.featureWeather, "11 bytes")
        #expect(decodeIdentity(newFirmwareRead.prefix(7)).obcm == 12, "7 bytes")
        #expect(decodeIdentity(newFirmwareRead.prefix(6)).epoch == 0xA1B2_C3D4, "6 bytes")

        // 2 bytes: no mounted card, so no era to name — and no room for the bytes after it.
        let noStore = decodeIdentity(newFirmwareRead.prefix(2))
        #expect(noStore.version == OBCProtocol.version)
        #expect(noStore.epoch == nil, "ack fail-closed — never epoch 0, which is a legal era")
        #expect(noStore.obcm == nil)
        #expect(noStore.features == nil)
    }

    /// 8, 9 and 10 bytes are a broken read of a `u32`, not a smaller capability set. Decoding the
    /// bytes that arrived could claim a feature the device never announced.
    @Test func aPartialCapabilityWordNeverClaimsWeather() {
        for length in 8..<11 {
            let decoded = decodeIdentity(newFirmwareRead.prefix(length))
            #expect(decoded.features == nil, "\(length) bytes must not yield a partial word")
            let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: decoded.features)
            #expect(!info.supportsWeather, "\(length) bytes must not claim weather")
        }
    }

    @Test func unknownFeatureBitsAreIgnored() {
        // Bit 31 is a capability this build has never heard of; it must not mask bit 0 beside it.
        let exotic = Data([0x02, 0x00, 0x09, 0x00, 0x00, 0x00, 12, 0x01, 0x00, 0x00, 0x80])
        let decoded = decodeIdentity(exotic)
        #expect(decoded.features == (OBCProtocol.featureWeather | 1 << 31))
        #expect(DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: decoded.features).supportsWeather)
    }

    @Test func anAbsentCapabilityWordIsNotAFabricatedZero() {
        let unknown = DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: nil)
        let announced = DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: 0)
        #expect(unknown.featureBits == nil, "a diagnostic must be able to tell these apart")
        #expect(announced.featureBits == 0)
        #expect(!unknown.supportsWeather)
        #expect(!announced.supportsWeather)
    }
}

@Suite("Config weather refresh compatibility")
struct ConfigWeatherRefreshTests {
    /// **Old app ↔ new firmware.** A 3-byte-plus-name blob (what an app predating WX3 writes to
    /// rename the device) must read as *refresh unspecified* — the device default — and never as
    /// `.off`, which would silently disable weather on a rename.
    @Test func anOldAppsConfigWriteDoesNotDisableWeather() throws {
        // The pre-WX3 blob, written out by hand rather than encoded, so this test still means
        // something if the encoder changes.
        let oldBlob = Data([0x08, 0x00] + Array("OBC-1A2B".utf8) + [0x00])
        #expect(oldBlob.count == 2 + 8 + 1)

        let decoded = try ConfigObjectCodec.decode(oldBlob)
        #expect(decoded.weatherRefreshRaw == nil, "unspecified — no byte at all")
        #expect(decoded.knownWeatherRefresh == nil)
        #expect(decoded.knownWeatherRefresh != .off, "absent is not Off")
        #expect(decoded.effectiveWeatherRefresh == .every30)
        #expect(ConfigObjectCodec.encode(decoded) == oldBlob, "and it round-trips byte-exactly")
    }

    /// The **write**-direction meaning of *absent*, which is not the read-direction meaning: leave
    /// the stored value alone. An old app renaming the device sends the 3-byte-plus-name blob, and
    /// a device that read that as "the rider chose the default" would reset a rider who
    /// deliberately chose `.off` back to 30-minute wakeups.
    @Test func anAbsentRefreshOnAWriteMeansLeaveTheStoredValueAlone() throws {
        let renameOnly = ConfigObjectCodec.encode(DeviceConfig(name: "Alps"))
        let decoded = try ConfigObjectCodec.decode(renameOnly)
        #expect(try decoded.weatherRefreshToApply() == nil, "no refusal, and nothing to store")
        #expect(decoded.knownWeatherRefresh == nil)
    }

    /// **New app ↔ old firmware.** The new app appends the refresh byte; a firmware predating it
    /// ignores the trailing byte under the append-only rule and stores everything before it.
    @Test func configRoundTripsWithTheRefreshField() throws {
        let config = DeviceConfig(name: "Timo's OBC", units: .imperial, weatherRefresh: .every120)
        let blob = ConfigObjectCodec.encode(config)
        #expect(blob.count == 2 + 10 + 1 + 1)
        #expect(blob[blob.endIndex - 1] == 4, "the appended refresh byte")
        #expect(try ConfigObjectCodec.decode(blob) == config)

        // The old firmware's decoder, transcribed: it never looks past `units`.
        let b = blob.startIndex
        let nameLen = Int(blob[b]) | (Int(blob[b + 1]) << 8)
        #expect(String(decoding: blob[(b + 2)..<(b + 2 + nameLen)], as: UTF8.self) == "Timo's OBC")
        #expect(blob[b + 2 + nameLen] == 1, "units still land where the old layout put them")
    }

    /// §11.8's asymmetry, both halves, on one blob. The *write* direction refuses an interval the
    /// device cannot honour; the *read* direction reports it as unknown and keeps going. A single
    /// direction-blind rule cannot be right for both: rejecting the read is what would let a future
    /// fifth interval stop a shipped app from so much as renaming its device, and tolerating the
    /// write is what would tell a rider their choice was applied when it was discarded.
    @Test func anUnknownRefreshByteIsRefusedOnAWriteAndToleratedOnARead() throws {
        var blob = [UInt8](ConfigObjectCodec.encode(DeviceConfig(name: "OBC", weatherRefresh: .off)))
        blob[blob.count - 1] = 200

        let decoded = try ConfigObjectCodec.decode(Data(blob))
        #expect(decoded.weatherRefreshRaw == 200, "the byte survives verbatim")
        #expect(decoded.knownWeatherRefresh == nil, "a reader sees unknown, not Off and not the default")
        #expect(decoded.effectiveWeatherRefresh == nil, "and it is never dressed up as the default")
        #expect(ConfigObjectCodec.encode(decoded) == Data(blob), "a rename must not flatten it")
        #expect(throws: WeatherRequestError.unknownRefresh(200), "a device asked to adopt it must refuse") {
            try decoded.weatherRefreshToApply()
        }
    }

    /// A future field appended after `weatherRefresh` must not make this build refuse the blob.
    @Test func configStillAcceptsATrailingBytePastTheFieldsWeKnow() throws {
        let config = DeviceConfig(name: "OBC", weatherRefresh: .every15)
        let blob = ConfigObjectCodec.encode(config) + Data([0x77])
        #expect(try ConfigObjectCodec.decode(blob) == config)
    }

    @Test func everyRefreshValueSurvivesTheBlob() throws {
        for refresh in WeatherRefresh.allCases {
            let config = DeviceConfig(name: "OBC", weatherRefresh: refresh)
            let decoded = try ConfigObjectCodec.decode(ConfigObjectCodec.encode(config))
            #expect(decoded == config)
            #expect(try decoded.weatherRefreshToApply() == refresh, "a known interval is applicable")
        }
    }
}

// MARK: - The discovery state machine (payload-independent, kept from the WX3 spike)

@Suite("BLE discovery intent ownership")
struct BLEDiscoveryIntentPolicyTests {
    private let known = UUID(uuidString: "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE")!
    private let wrong = UUID(uuidString: "11111111-2222-3333-4444-555555555555")!

    @Test func foregroundScansForControlAndWeatherAndAcceptsEitherAdvertisement() {
        var policy = BLEDiscoveryIntentPolicy()
        #expect(policy.requestForeground() == .scan)
        #expect(policy.scanServices == [.control, .weatherRequest])
        #expect(policy.discovered(peripheralID: wrong, knownPeripheralID: nil) == .connect(owner: .foreground))
    }

    @Test func weatherAcceptsOnlyKnownAuthenticatedPeripheralAndDeduplicatesAdvertisements() {
        var policy = BLEDiscoveryIntentPolicy()
        #expect(policy.requestWeather(knownPeripheralID: known, connectedPeripheralID: nil) == .scan)
        #expect(policy.scanServices == [.weatherRequest])
        #expect(policy.discovered(peripheralID: wrong, knownPeripheralID: known) == .ignore)
        #expect(policy.discovered(peripheralID: known, knownPeripheralID: known) == .connect(owner: .weatherRequest))
        #expect(policy.discovered(peripheralID: known, knownPeripheralID: known) == .ignore)
    }

    @Test func weatherReadReusesForegroundConnectionAndNeverDisconnectsIt() {
        var policy = BLEDiscoveryIntentPolicy()
        _ = policy.requestForeground()
        _ = policy.discovered(peripheralID: known, knownPeripheralID: known)
        policy.didConnect(peripheralID: known)

        #expect(policy.requestWeather(knownPeripheralID: known, connectedPeripheralID: known) == .readOnExistingConnection)
        #expect(policy.finishWeatherRequest() == false)
        #expect(policy.foregroundRequested)
        #expect(policy.phase == .connected(peripheralID: known, owner: .foreground))
    }

    @Test func oneShotDisconnectsOnlyConnectionItOwns() {
        var policy = BLEDiscoveryIntentPolicy()
        _ = policy.requestWeather(knownPeripheralID: known, connectedPeripheralID: nil)
        _ = policy.discovered(peripheralID: known, knownPeripheralID: known)
        policy.didConnect(peripheralID: known)

        let shouldDisconnect = policy.finishWeatherRequest()
        #expect(shouldDisconnect)
        policy.didDisconnect()
        #expect(policy.phase == .idle)
        #expect(!policy.hasIntent)
    }

    @Test func foregroundSuspendDoesNotCancelPendingWeatherIntent() {
        var policy = BLEDiscoveryIntentPolicy()
        _ = policy.requestForeground()
        _ = policy.requestWeather(knownPeripheralID: known, connectedPeripheralID: nil)
        _ = policy.discovered(peripheralID: known, knownPeripheralID: known)
        policy.didConnect(peripheralID: known)

        let shouldDisconnect = policy.cancelForeground()
        #expect(shouldDisconnect)
        policy.didDisconnect()
        #expect(policy.phase == .scanning)
        #expect(policy.scanServices == [.weatherRequest])
    }

    @Test func cancellationAndBluetoothToggleAreIdempotent() {
        var policy = BLEDiscoveryIntentPolicy()
        _ = policy.requestWeather(knownPeripheralID: known, connectedPeripheralID: nil)
        let firstFinishDisconnects = policy.finishWeatherRequest()
        let duplicateFinishDisconnects = policy.finishWeatherRequest()
        #expect(!firstFinishDisconnects)
        #expect(!duplicateFinishDisconnects)

        _ = policy.requestWeather(knownPeripheralID: known, connectedPeripheralID: nil)
        policy.radioBecameUnavailable()
        policy.radioBecameUnavailable()
        #expect(policy.phase == .idle)
        #expect(!policy.hasIntent)
    }

    @Test func restorationRequiresWeatherOnlyScanAndKnownPeripheral() {
        var policy = BLEDiscoveryIntentPolicy()
        #expect(
            policy.restoreWeatherRequest(
                scannedServices: [.control, .weatherRequest],
                restoredPeripheralIDs: [known], knownPeripheralID: known
            ) == nil
        )
        #expect(policy.phase == .idle)

        #expect(
            policy.restoreWeatherRequest(
                scannedServices: [.weatherRequest],
                restoredPeripheralIDs: [wrong], knownPeripheralID: known
            ) == nil
        )
        #expect(policy.phase == .idle)

        #expect(
            policy.restoreWeatherRequest(
                scannedServices: [.weatherRequest],
                restoredPeripheralIDs: [known, wrong], knownPeripheralID: known
            ) == known
        )
        #expect(policy.phase == .connecting(peripheralID: known, owner: .weatherRequest))
    }

    @Test func restorationWithoutPeripheralReplaysBoundedScan() {
        var policy = BLEDiscoveryIntentPolicy()
        #expect(
            policy.restoreWeatherRequest(
                scannedServices: [.weatherRequest], restoredPeripheralIDs: [], knownPeripheralID: known
            ) == nil
        )
        #expect(policy.phase == .scanning)
        #expect(policy.scanServices == [.weatherRequest])
    }
}

@Suite("BLE discovery persistence")
struct BLEDiscoveryStoreTests {
    @Test func authenticatedIDAndRestorationDeadlineRoundTripAndClear() throws {
        let suite = "obc-ble-discovery-tests-\(UUID().uuidString)"
        let defaults = try #require(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = UserDefaultsBLEDiscoveryStore(defaults: defaults)
        let id = UUID(uuidString: "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE")!
        let deadline = Date(timeIntervalSince1970: 1_800_000_000)

        #expect(store.knownPeripheralID() == nil)
        #expect(store.weatherRestorationDeadline() == nil)
        store.saveKnownPeripheralID(id)
        store.armWeatherRestoration(until: deadline)
        #expect(store.knownPeripheralID() == id)
        #expect(store.weatherRestorationDeadline() == deadline)

        store.clearKnownPeripheralID()
        store.clearWeatherRestoration()
        #expect(store.knownPeripheralID() == nil)
        #expect(store.weatherRestorationDeadline() == nil)
    }
}

// MARK: - Helpers

private func le32(_ value: UInt32) -> [UInt8] {
    (0..<4).map { UInt8((value >> (8 * UInt32($0))) & 0xFF) }
}

private func le64(_ value: UInt64) -> [UInt8] {
    (0..<8).map { UInt8((value >> (8 * UInt64($0))) & 0xFF) }
}
