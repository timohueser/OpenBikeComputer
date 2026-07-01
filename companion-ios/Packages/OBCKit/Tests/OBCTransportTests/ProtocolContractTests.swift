import XCTest
import OBCDomain

/// B-S0 guardrails: the domain-type skeletons `B1` builds against compile and
/// construct, and the pinned `protocol_version` is stated in one place. These are
/// contract checks, not behavior — the real transport/codec coverage is `B1`.
final class ProtocolContractTests: XCTestCase {
    func testProtocolVersionIsPinned() {
        // Bump deliberately, in lockstep with firmware S0 — not by accident.
        XCTAssertEqual(OBCProtocol.version, 1)
    }

    func testDeviceInfoDefaultsToTheExpectedProtocolVersion() {
        let info = DeviceInfo(name: "OBC", firmwareVersion: "1.0.0")
        XCTAssertEqual(info.protocolVersion, OBCProtocol.version)
    }

    func testProtocolMismatchCarriesBothVersions() {
        let err = DeviceError.protocolMismatch(expected: 1, found: 2)
        XCTAssertEqual(err, .protocolMismatch(expected: 1, found: 2))
        XCTAssertNotEqual(err, .protocolMismatch(expected: 1, found: 3))
    }

    func testTransferProgressFraction() {
        XCTAssertEqual(TransferProgress(bytesDone: 25, total: 100, offset: 25).fraction, 0.25)
        // Unknown total → 0, never a divide-by-zero.
        XCTAssertEqual(TransferProgress(bytesDone: 5, total: 0, offset: 0).fraction, 0)
    }

    func testDomainSkeletonsConstruct() {
        // Config carries the device name (Delta 1).
        XCTAssertEqual(DeviceConfig(name: "OBC-Trailhead").name, "OBC-Trailhead")

        // Routes accept both import formats (Delta 2).
        let route = RouteSummary(
            id: RouteID("r1"), name: "Ridge Loop",
            distanceMeters: 42_000, elevationGainMeters: 1_200, source: .gpx
        )
        XCTAssertEqual(route.source, .gpx)
        XCTAssertEqual(RouteBlob(summary: route, payload: Data([0xAB])).payload.count, 1)

        let ride = RideSummary(
            id: RideID("ride1"), name: "Morning", date: Date(), distanceMeters: 12_500
        )
        XCTAssertEqual(ride.id, RideID("ride1"))

        XCTAssertEqual(ConnectionState.outOfRange, .outOfRange)
    }
}
