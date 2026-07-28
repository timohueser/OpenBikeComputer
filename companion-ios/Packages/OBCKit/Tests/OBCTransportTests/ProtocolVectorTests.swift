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
        XCTAssertEqual(decodedFull.obcm, 10)

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
        XCTAssertEqual(decodedFuture.obcm, 10)
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
