import XCTest
import OBCDomain
@testable import OBCTransport
#if canImport(CoreBluetooth)
import CoreBluetooth
#endif

/// The Swift half of the S0 shared-vector pin: the checked-in fixtures under
/// `specs/vectors/` (repo root) must decode through the app's codecs to the
/// values `manifest.json` states, and re-encode **byte-exactly**. The firmware
/// side pins the same files (`cargo test -p obc-vectors`), so neither side can
/// drift from `obc-ble-interface-spec.md` without a test going red.
final class ProtocolVectorTests: XCTestCase {
    /// `specs/vectors/`, resolved from this file's location
    /// (companion-ios/Packages/OBCKit/Tests/OBCTransportTests/…).
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // OBCTransportTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // OBCKit
        .deletingLastPathComponent()  // Packages
        .deletingLastPathComponent()  // companion-ios
        .deletingLastPathComponent()  // repo root
        .appendingPathComponent("specs/vectors")

    private func fixture(_ name: String) throws -> Data {
        let url = Self.vectorsDir.appendingPathComponent(name)
        guard let data = FileManager.default.contents(atPath: url.path) else {
            XCTFail("fixture \(name) missing at \(url.path) — regenerate with `cargo test -p obc-vectors regenerate -- --ignored`")
            throw DeviceError.readFailed
        }
        return data
    }

    private func manifest() throws -> [String: Any] {
        let data = try fixture("manifest.json")
        return try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    func testManifestPinsTheProtocolVersion() throws {
        let manifest = try manifest()
        XCTAssertEqual(manifest["protocol_version"] as? Int, Int(OBCProtocol.version))
    }

    #if canImport(CoreBluetooth)
    func testGATTUUIDsMatchTheManifest() throws {
        let uuids = try XCTUnwrap(try manifest()["uuids"] as? [String: String])
        XCTAssertEqual(GATT.obcControlService.uuidString, uuids["obc_control_service"])
        XCTAssertEqual(GATT.command.uuidString, uuids["command"])
        XCTAssertEqual(GATT.status.uuidString, uuids["status"])
        XCTAssertEqual(GATT.config.uuidString, uuids["config"])
        XCTAssertEqual(GATT.transferControl.uuidString, uuids["transfer_control"])
        XCTAssertEqual(GATT.psm.uuidString, uuids["psm"])
        XCTAssertEqual(GATT.protocolVersion.uuidString, uuids["protocol_version"])
        // v2 dropped `objectStore` (0003) and `diagnostics` (0006) — the manifest
        // no longer lists them, and the six-characteristic surface is what remains.
        XCTAssertNil(uuids["object_store"])
        XCTAssertNil(uuids["diagnostics"])
        // The Weather Request service (WX3 / #1188) is a base of its own, not a block inside OBC
        // Control: iOS matches the advertisement on this UUID alone, so the two must be
        // independently advertisable.
        XCTAssertEqual(GATT.weatherRequestService.uuidString, uuids["weather_request_service"])
        XCTAssertEqual(GATT.weatherRequestContext.uuidString, uuids["weather_request_context"])
    }
    #endif

    func testCRC32CheckValueAndRouteObjectCRC() throws {
        // Spec §6's pinned check value…
        XCTAssertEqual(CRC32.checksum(Data("123456789".utf8)), 0xCBF4_3926)
        // …and the whole-object CRC the upload descriptor announces.
        let route = try fixture("route-waypoints.obcr")
        let start = try TransferControl(decoding: try fixture("transfer-upload-start.bin"))
        XCTAssertEqual(CRC32.checksum(route), start.crc32)
        XCTAssertEqual(Int(start.totalLen), route.count)
    }

    func testUploadTranscriptDescriptors() throws {
        // The 12-byte v2 descriptor (no trailing `offset`).
        let startBytes = try fixture("transfer-upload-start.bin")
        XCTAssertEqual(startBytes.count, TransferControl.encodedLength)
        let start = try TransferControl(decoding: startBytes)
        XCTAssertEqual(start.op, .upload)
        XCTAssertEqual(start.type, .route)
        XCTAssertEqual(start.objectID, TransferControl.newObjectID)
        XCTAssertEqual(start.encode(), startBytes)

        // Download request: rideList, no length/CRC (the device's announce fills them).
        let requestBytes = try fixture("transfer-download-request.bin")
        let request = try TransferControl(decoding: requestBytes)
        XCTAssertEqual(request, TransferControl(op: .download, type: .rideList, objectID: 0))
        XCTAssertEqual(request.encode(), requestBytes)

        // Abort of the active upload.
        let abortBytes = try fixture("transfer-abort.bin")
        let abort = try TransferControl(decoding: abortBytes)
        XCTAssertEqual(abort, TransferControl(op: .abort, type: .route, objectID: TransferControl.newObjectID))
        XCTAssertEqual(abort.encode(), abortBytes)
    }

    func testStatusMessages() throws {
        let route = try fixture("route-waypoints.obcr")

        // The closing result: committed, the device-assigned id, all bytes durable.
        let resultBytes = try fixture("status-transfer-result.bin")
        let result = try StatusMessage(decoding: resultBytes)
        XCTAssertEqual(result, .transferResult(TransferResult(
            objectID: DeviceObjectID(7), status: .committed, committedOffset: UInt32(route.count)
        )))
        XCTAssertEqual(result.encode(), resultBytes)

        // The storage-full reject: a new-route upload (id 0xFFFF → nil) refused at
        // descriptor-open time with nothing committed. The `0xFFFF` sentinel maps
        // to a nil objectID.
        let storageFullBytes = try fixture("status-transfer-storage-full.bin")
        let storageFull = try StatusMessage(decoding: storageFullBytes)
        XCTAssertEqual(storageFull, .transferResult(TransferResult(
            objectID: nil, status: .storageFull, committedOffset: 0
        )))
        XCTAssertEqual(storageFull.encode(), storageFullBytes)

        let changedBytes = try fixture("status-store-changed.bin")
        let changed = try StatusMessage(decoding: changedBytes)
        XCTAssertEqual(changed, .storeChanged(StoreChanged(type: .route, revision: 42)))
        XCTAssertEqual(changed.encode(), changedBytes)

        // v2: the download announce rides `status` as `msg = 4` (the msg byte + the
        // 12-byte descriptor) — one notify surface, not the retired `transferControl`
        // CCCD. It decodes to the same descriptor the device would have notified.
        let announceBytes = try fixture("status-download-announce.bin")
        XCTAssertEqual(announceBytes.count, 1 + TransferControl.encodedLength)
        let announce = try StatusMessage(decoding: announceBytes)
        XCTAssertEqual(announce, .downloadAnnounce(TransferControl(
            op: .download, type: .route, objectID: 7,
            totalLen: UInt32(route.count), crc32: CRC32.checksum(route)
        )))
        XCTAssertEqual(announce.encode(), announceBytes)
    }

    /// The identity read's decode is **length-driven**, and this walks all three lengths the wire
    /// defines (spec §1). The rule that matters is the same at every step: a trailing field that
    /// did not arrive is `nil`, never a fabricated `0` — `0` is a legal store epoch, and OBCM `0`
    /// would read as "supports OBCM v0" and refuse every real map.
    func testVersionReadDecodesEveryLength() throws {
        /// The same decode `BLETransport.deviceInfo()` performs, over an arbitrary read.
        func decode(_ bytes: Data) -> (version: UInt16, epoch: UInt32?, obcm: UInt8?) {
            let b = bytes.startIndex
            let version = bytes.count >= 2 ? UInt16(bytes[b]) | (UInt16(bytes[b + 1]) << 8) : OBCProtocol.version
            let epoch: UInt32? = bytes.count >= 6
                ? UInt32(bytes[b + 2]) | (UInt32(bytes[b + 3]) << 8)
                    | (UInt32(bytes[b + 4]) << 16) | (UInt32(bytes[b + 5]) << 24)
                : nil
            return (version, epoch, bytes.count >= 7 ? bytes[b + 6] : nil)
        }

        // 7 bytes: `version u16 · store_epoch u32 · obcm_version u8`. The last is the OBCM
        // map-format version the device's reader reads — not the protocol version beside it.
        let full = try fixture("version-read.bin")
        XCTAssertEqual(full.count, 7)
        let decodedFull = decode(full)
        XCTAssertEqual(decodedFull.version, OBCProtocol.version)
        XCTAssertEqual(decodedFull.epoch, 0xA1B2_C3D4)
        XCTAssertEqual(decodedFull.obcm, 13)

        // 6 bytes: a firmware predating the field. The epoch is present, so the ack gate is open;
        // the map version is simply unknown.
        let noObcm = try fixture("version-read-noobcm.bin")
        XCTAssertEqual(noObcm.count, 6)
        let decodedNoObcm = decode(noObcm)
        XCTAssertEqual(decodedNoObcm.epoch, 0xA1B2_C3D4)
        XCTAssertNil(decodedNoObcm.obcm, "an absent trailing field is unknown, never 0")

        // 2 bytes: no mounted card, so no era to name — and no room for the byte after it.
        let noStore = try fixture("version-read-nostore.bin")
        XCTAssertEqual(noStore.count, 2)
        let decodedNoStore = decode(noStore)
        XCTAssertEqual(decodedNoStore.version, OBCProtocol.version)
        XCTAssertNil(decodedNoStore.epoch, "ack fail-closed — never epoch 0, which is a legal era")
        XCTAssertNil(decodedNoStore.obcm)

        // And the append-only rule the obcm byte itself rode in on: bytes past the known fields are
        // ignored rather than rejected, which is why the field needed no protocol-version bump.
        let future = full + Data([0xEE, 0xEE])
        let decodedFuture = decode(future)
        XCTAssertEqual(decodedFuture.epoch, 0xA1B2_C3D4)
        XCTAssertEqual(decodedFuture.obcm, 13)
    }

    /// The widened identity read (WX3 / #1188): the capability word is an **append**, so the same
    /// length-driven decode reads all four lengths, and a device that predates the word reads as
    /// "no weather" rather than as a fabricated `0`.
    func testVersionReadCapabilityWord() throws {
        func decodeFeatures(_ bytes: Data) -> UInt32? {
            let b = bytes.startIndex
            guard bytes.count >= 11 else { return nil }
            return UInt32(bytes[b + 7]) | (UInt32(bytes[b + 8]) << 8)
                | (UInt32(bytes[b + 9]) << 16) | (UInt32(bytes[b + 10]) << 24)
        }

        let features = try fixture("version-read-features.bin")
        XCTAssertEqual(features.count, 11)
        let b = features.startIndex
        XCTAssertEqual(UInt16(features[b]) | (UInt16(features[b + 1]) << 8), OBCProtocol.version,
                       "an append never moves the protocol version underneath it")
        // The epoch deliberately differs from the 0xA1B2C3D4 the three older identity reads share,
        // so opening the wrong file fails here rather than passing everything but the feature word.
        XCTAssertEqual(
            UInt32(features[b + 2]) | (UInt32(features[b + 3]) << 8)
                | (UInt32(features[b + 4]) << 16) | (UInt32(features[b + 5]) << 24),
            0xC0DE_F00D
        )
        XCTAssertEqual(decodeFeatures(features), OBCProtocol.featureWeather)
        XCTAssertTrue(
            DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: decodeFeatures(features)).supportsWeather
        )

        // The pre-WX3 read is now exactly "a device without the weather contract".
        let noFeatures = try fixture("version-read.bin")
        XCTAssertEqual(noFeatures.count, 7)
        XCTAssertNil(decodeFeatures(noFeatures), "absent, never 0")
        XCTAssertFalse(
            DeviceInfo(name: "OBC", firmwareVersion: "1.0", featureBits: decodeFeatures(noFeatures)).supportsWeather
        )

        // 8, 9 and 10 bytes are a broken read of a u32, not a smaller capability set — decoding the
        // bytes that arrived could claim a feature the device never announced.
        for length in 8..<11 {
            XCTAssertNil(decodeFeatures(features.prefix(length)), "\(length) bytes is a torn word")
        }
    }

    /// The Config pair — same object, one appended byte. The only way to get this wrong is the
    /// offset, and an off-by-one still reads the *shorter* file correctly, which is why both are
    /// pinned together.
    func testConfigWeatherRefreshVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("config-weather-refresh.bin")
        XCTAssertEqual(bytes.count, 14)
        let config = try ConfigObjectCodec.decode(bytes)
        XCTAssertEqual(config, DeviceConfig(name: "OBC Alpine", units: .imperial, weatherRefresh: .every60))
        XCTAssertEqual(config.knownWeatherRefresh?.minutes, 60)
        XCTAssertEqual(try config.weatherRefreshToApply(), .every60, "a device may adopt it")
        XCTAssertEqual(ConfigObjectCodec.encode(config), bytes)

        // …and its sibling, the blob an app predating WX3 writes: the refresh field is UNSPECIFIED
        // (device default), never `Off`, or a rename would silently switch weather off.
        let v1 = try ConfigObjectCodec.decode(try fixture("config-v1.bin"))
        XCTAssertNil(v1.weatherRefreshRaw)
        XCTAssertNotEqual(v1.knownWeatherRefresh, .off)
        XCTAssertEqual(v1.effectiveWeatherRefresh, .every30)
        // …and on the *write* side absent is not the default either: it is "change nothing".
        XCTAssertNil(try v1.weatherRefreshToApply())
    }

    /// §11.8's asymmetry, pinned on a real blob rather than a hand-built one: a Config whose
    /// refresh byte names an interval v1 never defined. A **read** must tolerate it — otherwise
    /// appending a fifth interval one day locks a shipped app out of Config badly enough that it
    /// can no longer even rename the device — while a **write** of it must be refused, because a
    /// device cannot honour an interval it does not know and substituting one would report a
    /// setting back to the rider that was discarded.
    func testConfigUnknownWeatherRefreshVectorIsToleratedOnReadAndRefusedOnWrite() throws {
        let bytes = try fixture("config-weather-refresh-unknown.bin")
        let config = try ConfigObjectCodec.decode(bytes)

        let raw = try XCTUnwrap(config.weatherRefreshRaw)
        XCTAssertNil(WeatherRefresh(wireByte: raw), "the fixture must name an interval v1 lacks")
        XCTAssertNil(config.knownWeatherRefresh, "unknown, and specifically not Off")
        XCTAssertNil(config.effectiveWeatherRefresh, "never dressed up as the 30-minute default")
        XCTAssertEqual(ConfigObjectCodec.encode(config), bytes, "and a rename re-writes it verbatim")

        XCTAssertThrowsError(try config.weatherRefreshToApply()) { error in
            XCTAssertEqual(error as? WeatherRequestError, .unknownRefresh(raw))
        }
    }

    /// The weather request context (spec §11) — the one value the companion reads before it
    /// disconnects. All three fixtures are 52 bytes; what differs is what they *claim*.
    func testWeatherRequestContextVectorsDecodeAndReEncodeByteExactly() throws {
        // A rider mid-ride with everything the device can know.
        let fullBytes = try fixture("weather-request-context-full.bin")
        XCTAssertEqual(fullBytes.count, WeatherRequestContext.encodedLength)
        let full = try WeatherRequestContext(decoding: fullBytes)
        XCTAssertEqual(full.version, WeatherRequestContext.currentVersion)
        XCTAssertEqual(full.validity, [.position, .bearing, .speed, .bundle, .route])
        XCTAssertEqual(full.reason, [.scheduled])
        XCTAssertEqual(full.refresh, .every30)
        XCTAssertEqual(full.requestID, 0x1188_0001)
        let fix = try XCTUnwrap(full.fix)
        XCTAssertEqual(fix.latitudeMicrodegrees, 47_999_008)
        XCTAssertEqual(fix.longitudeMicrodegrees, 7_842_104)
        XCTAssertEqual(fix.utc, Date(timeIntervalSince1970: 1_800_001_800))
        XCTAssertEqual(full.bearingDegrees, 342)
        XCTAssertEqual(try XCTUnwrap(full.speedMetersPerSecond), 7.1, accuracy: 1e-9)
        XCTAssertEqual(full.routeID, 7)
        let bundle = try XCTUnwrap(full.bundle)
        XCTAssertEqual(bundle.generation, 6)
        XCTAssertEqual(bundle.generatedAt, Date(timeIntervalSince1970: 1_800_000_000))
        // The bundle group names a bundle that EXISTS: this is the whole-object CRC of the OBCW an
        // upload of it would have announced, not that file's internal header CRC.
        XCTAssertEqual(bundle.crc32, 0xBC1E_46C8)
        XCTAssertEqual(bundle.crc32, CRC32.checksum(try fixture("weather-dwd-96x96-9f.obcw")))
        XCTAssertEqual(full.encode(), fullBytes)

        // The resting value: structurally valid, claiming nothing. Deliberately *not* all-zeroes —
        // an all-zero attribute would decode as layout version 0 with weather switched Off.
        let emptyBytes = try fixture("weather-request-context-empty.bin")
        let empty = try WeatherRequestContext(decoding: emptyBytes)
        XCTAssertEqual(empty, WeatherRequestContext.empty)
        XCTAssertTrue(empty.validity.isEmpty)
        XCTAssertTrue(empty.reason.isEmpty)
        XCTAssertEqual(empty.refresh, .every30, "the default is stated, not left as byte 0 = Off")
        XCTAssertEqual(empty.encode(), emptyBytes)

        // Cold start indoors: urgent, no fix, no bundle. Absence is a cleared flag, so the zero
        // coordinates must not put the rider at 0°N 0°E holding generation 0 — and `refresh == off`
        // configures the *schedule*, not the right to ask, so this remains a diagnostic/retryable
        // request even though the companion cannot fetch before the device supplies a fix.
        let noFixBytes = try fixture("weather-request-context-no-fix.bin")
        let noFix = try WeatherRequestContext(decoding: noFixBytes)
        XCTAssertEqual(noFix.reason, [.urgent, .noBundle])
        XCTAssertEqual(noFix.refresh, .off)
        XCTAssertEqual(noFix.requestID, 0x1188_0002)
        XCTAssertNil(noFix.fix)
        XCTAssertNil(noFix.bundle)
        XCTAssertNil(noFix.routeID)
        XCTAssertEqual(noFix.encode(), noFixBytes)
    }

    /// The read half of §11.8 against a checked-in fixture: a context whose refresh byte names an
    /// interval this build does not define. It must decode — a firmware appending a fifth interval
    /// is an ordinary enum append, and a read that threw here would leave weather permanently dead
    /// on every already-shipped app — report *unknown* rather than Off or the default, and survive
    /// a re-encode byte-for-byte so nothing is lost on the way back out.
    func testWeatherRequestContextUnknownRefreshVectorDecodesAsUnknown() throws {
        let bytes = try fixture("weather-request-context-unknown-refresh.bin")
        let context = try WeatherRequestContext(decoding: bytes)

        XCTAssertNil(WeatherRefresh(wireByte: context.refreshRaw), "the fixture must name an interval v1 lacks")
        XCTAssertNil(context.refresh, "unknown…")
        XCTAssertNotEqual(context.refresh, .off, "…and specifically not Off")
        XCTAssertNotEqual(context.refresh, .deviceDefault, "…nor the default")
        XCTAssertEqual(context.encode(), bytes, "the byte rides back out verbatim")

        // The rest of the read is unaffected — one unknown byte must not cost the whole request.
        XCTAssertEqual(context.version, WeatherRequestContext.currentVersion)
        XCTAssertFalse(context.reason.isEmpty, "a request this app must still answer")
    }

    /// Sign handling, which no other context fixture exercises: a rider in the southern **and**
    /// western hemispheres, with a pre-1970 timestamp. Latitude/longitude are `i32` microdegrees
    /// and the two UTC stamps are `i64` seconds, and every one of those four paths goes through a
    /// bit-pattern reinterpretation — a decoder that read them as unsigned would put this rider
    /// 4295 degrees north and date the fix to the year 2554 rather than refusing, which is exactly
    /// the kind of failure that shows up as a forecast for the wrong hemisphere and nothing else.
    func testWeatherRequestContextSouthernVectorDecodesSignedFieldsCorrectly() throws {
        let bytes = try fixture("weather-request-context-southern.bin")
        let context = try WeatherRequestContext(decoding: bytes)

        let fix = try XCTUnwrap(context.fix, "the southern fixture must claim a position")
        XCTAssertLessThan(fix.latitudeMicrodegrees, 0, "southern hemisphere")
        XCTAssertLessThan(fix.longitudeMicrodegrees, 0, "western hemisphere")
        XCTAssertEqual(fix.latitude, Double(fix.latitudeMicrodegrees) / 1_000_000)
        XCTAssertEqual(fix.longitude, Double(fix.longitudeMicrodegrees) / 1_000_000)

        // At least one of the two i64 stamps is negative (pre-1970) — that is what this fixture is
        // for. Whichever it is, it must survive as a date before the epoch, not a far-future one.
        let bundleStamp = context.bundleWireGeneratedAtSeconds
        XCTAssertTrue(
            context.fixUTCSeconds < 0 || bundleStamp < 0,
            "the fixture must carry a negative i64 to be worth pinning"
        )
        if context.fixUTCSeconds < 0 {
            XCTAssertLessThan(fix.utc, Date(timeIntervalSince1970: 0))
        }
        if context.validity.contains(.bundle), bundleStamp < 0 {
            XCTAssertLessThan(try XCTUnwrap(context.bundle).generatedAt, Date(timeIntervalSince1970: 0))
        }

        XCTAssertEqual(context.encode(), bytes, "and every signed field re-encodes byte-exactly")
    }

    func testRideVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("ride-v1.bin")
        let ride = try RideObjectCodec.decode(bytes, id: RideID("7"))

        XCTAssertEqual(ride.summary.name, "Höhenweg")
        XCTAssertEqual(ride.summary.date, Date(timeIntervalSince1970: 1_751_450_000))
        XCTAssertEqual(ride.summary.distanceMeters, 42_500)
        XCTAssertEqual(ride.summary.movingTime, 9_000)
        XCTAssertEqual(ride.summary.averageSpeedMps, 4.72, accuracy: 0.001)
        XCTAssertEqual(ride.summary.climbMeters, 810)
        XCTAssertEqual(ride.points.count, 3)
        XCTAssertEqual(ride.points[0].coordinate.latitude, 48.0, accuracy: 1e-7)
        XCTAssertEqual(ride.points[0].coordinate.longitude, 7.8, accuracy: 1e-7)
        XCTAssertEqual(ride.points[0].elevationMeters, 214)
        XCTAssertEqual(ride.points[1].timestamp.timeIntervalSince(ride.summary.date), 60)
        XCTAssertNil(ride.points[2].elevationMeters, "INT16_MIN is the no-elevation sentinel")

        // The decoded values are exactly on the wire grid, so re-encoding must
        // reproduce the fixture byte-for-byte.
        XCTAssertEqual(RideObjectCodec.encode(ride), bytes)
    }

    func testConfigVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("config-v1.bin")
        let config = try ConfigObjectCodec.decode(bytes)
        XCTAssertEqual(config, DeviceConfig(name: "OBC Tourer", units: .metric))
        XCTAssertEqual(ConfigObjectCodec.encode(config), bytes)
    }

    func testRouteListVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("route-list.bin")
        // The regenerated fixture (epic #638 S4): a 6-byte header advertising
        // **entry_len 84** and 3 entries — the 76-byte v2 core + the auto-expiry
        // tail (`expires_at u32 · retention u8 · reserved[3]`), decoded here by the
        // header's `entry_len`, not a hard-coded length (the S6-fixed decoder).
        XCTAssertEqual(bytes[bytes.startIndex + 1], 84, "entry_len is 84 in the v2+expiry fixture")
        let entries = try RouteList.decode(bytes)
        XCTAssertEqual(entries.count, 3)

        // Entry fields come from the stored routes' OBCR headers (ids continue the transcript's
        // assigned id 7); byte_len sizes each stored file. v2 appends the whole-object `crc32`
        // (the content fingerprint); the expiry tail sits **after** it, outside its coverage.
        let waypointsRoute = try fixture("route-waypoints.obcr")
        let plainRoute = try fixture("route-plain.obcr")
        // Entry 0 — a live countdown: `expires_at` non-zero, retention = 3 (two weeks).
        XCTAssertEqual(entries[0], RouteListEntry(
            objectID: 7, byteLen: UInt32(waypointsRoute.count), distanceMeters: 2207,
            ascentMeters: 76, pointCount: 9, waypointCount: 2, name: "Vector Loop",
            crc32: CRC32.checksum(waypointsRoute), expiresAt: 1_784_808_000, retention: 3
        ))
        // Entry 1 — clock not started (`last_used == 0` → `expires_at == 0` → nil), retention = 1.
        XCTAssertEqual(entries[1], RouteListEntry(
            objectID: 8, byteLen: UInt32(plainRoute.count), distanceMeters: 2207,
            ascentMeters: 76, pointCount: 9, waypointCount: 0, name: "Vector Loop",
            crc32: CRC32.checksum(plainRoute), expiresAt: nil, retention: 1
        ))
        // Entry 2 — retention "never" (0), no expiry.
        XCTAssertEqual(entries[2], RouteListEntry(
            objectID: 9, byteLen: UInt32(plainRoute.count), distanceMeters: 2207,
            ascentMeters: 76, pointCount: 9, waypointCount: 0, name: "Vector Loop",
            crc32: 0x1557_AE0B, expiresAt: nil, retention: 0
        ))
        // The CRC is the OBCR object's own — the app can re-derive it to verify identity (V6).
        XCTAssertEqual(entries[0].crc32, 0x1BFB_6E3C)

        XCTAssertEqual(RouteList.encode(entries), bytes)  // byte-exact re-encode
    }
}
