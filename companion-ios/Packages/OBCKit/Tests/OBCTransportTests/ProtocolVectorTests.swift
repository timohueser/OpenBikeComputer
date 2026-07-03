import XCTest
import OBCDomain
@testable import OBCTransport
#if canImport(CoreBluetooth)
import CoreBluetooth
#endif

/// The Swift half of the shared-vector pin: the checked-in fixtures under
/// `protocol-vectors/` (repo root) must decode through the app's codecs to the
/// values `manifest.json` states, and re-encode **byte-exactly**. The firmware
/// side pins the same files (`cargo test -p obc-vectors`), so neither side can
/// drift from `obc-ble-interface-spec.md` without a test going red.
final class ProtocolVectorTests: XCTestCase {
    /// `protocol-vectors/` at the repo root, resolved from this file's location
    /// (companion-ios/Packages/OBCKit/Tests/OBCTransportTests/…).
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // OBCTransportTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // OBCKit
        .deletingLastPathComponent()  // Packages
        .deletingLastPathComponent()  // companion-ios
        .deletingLastPathComponent()  // repo root
        .appendingPathComponent("protocol-vectors")

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
        XCTAssertEqual(GATT.objectStore.uuidString, uuids["object_store"])
        XCTAssertEqual(GATT.config.uuidString, uuids["config"])
        XCTAssertEqual(GATT.transferControl.uuidString, uuids["transfer_control"])
        XCTAssertEqual(GATT.diagnostics.uuidString, uuids["diagnostics"])
        XCTAssertEqual(GATT.psm.uuidString, uuids["psm"])
        XCTAssertEqual(GATT.protocolVersion.uuidString, uuids["protocol_version"])
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
        let startBytes = try fixture("transfer-upload-start.bin")
        let start = try TransferControl(decoding: startBytes)
        XCTAssertEqual(start.op, .upload)
        XCTAssertEqual(start.type, .route)
        XCTAssertEqual(start.objectID, TransferControl.newObjectID)
        XCTAssertEqual(start.offset, 0)
        XCTAssertEqual(start.encode(), startBytes)

        // The historic resume descriptor differs from the fresh start only in its
        // offset — kept as a shape-stability pin (it must DECODE byte-exactly; the
        // device answers it `error`, transfers restart rather than resume).
        let resumeBytes = try fixture("transfer-upload-resume.bin")
        let resume = try TransferControl(decoding: resumeBytes)
        var expected = start
        expected.offset = resume.offset
        XCTAssertGreaterThan(resume.offset, 0)
        XCTAssertLessThan(Int(resume.offset), Int(start.totalLen))
        XCTAssertEqual(resume, expected)
        XCTAssertEqual(resume.encode(), resumeBytes)

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

    func testStatusMessagesAndStoreDigest() throws {
        let route = try fixture("route-waypoints.obcr")

        // The closing result: committed, the device-assigned id, all bytes durable.
        let resultBytes = try fixture("status-transfer-result.bin")
        let result = try StatusMessage(decoding: resultBytes)
        XCTAssertEqual(result, .transferResult(TransferResult(
            objectID: 7, status: .committed, committedOffset: UInt32(route.count)
        )))
        XCTAssertEqual(result.encode(), resultBytes)

        let changedBytes = try fixture("status-store-changed.bin")
        let changed = try StatusMessage(decoding: changedBytes)
        XCTAssertEqual(changed, .storeChanged(StoreChanged(type: .route, revision: 42)))
        XCTAssertEqual(changed.encode(), changedBytes)

        let digestBytes = try fixture("object-store.bin")
        let digest = try ObjectStoreDigest(decoding: digestBytes)
        XCTAssertEqual(digest, ObjectStoreDigest(revision: 42, routeCount: 3, rideCount: 5))
        XCTAssertEqual(digest.encode(), digestBytes)
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
        let entries = try RouteList.decode(bytes)
        XCTAssertEqual(entries.count, 2)

        // Entry fields come from the stored routes' OBCR headers (ids continue the transcript's
        // assigned id 7); byte_len sizes each stored file.
        let waypointsRoute = try fixture("route-waypoints.obcr")
        let plainRoute = try fixture("route-plain.obcr")
        XCTAssertEqual(entries[0], RouteListEntry(
            objectID: 7, byteLen: UInt32(waypointsRoute.count), distanceMeters: 2207,
            ascentMeters: 76, pointCount: 9, waypointCount: 2, name: "Vector Loop"
        ))
        XCTAssertEqual(entries[1], RouteListEntry(
            objectID: 8, byteLen: UInt32(plainRoute.count), distanceMeters: 2207,
            ascentMeters: 76, pointCount: 9, waypointCount: 0, name: "Vector Loop"
        ))

        XCTAssertEqual(RouteList.encode(entries), bytes)  // byte-exact re-encode
    }
}
