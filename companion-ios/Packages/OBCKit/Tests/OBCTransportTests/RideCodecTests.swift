import XCTest
import OBCDomain
@testable import OBCTransport

/// The provisional device ride codec (Codecs/RideCodec.swift) — round-trip,
/// quantization contract, and malformed-input behavior. Layout is S0-owned;
/// when it's repinned these tests are the single spot that must move with it.
final class RideCodecTests: XCTestCase {
    /// A ride whose values sit exactly on the wire grid (whole seconds/metres,
    /// 1e-7° coordinates, cm/s speed) so the round-trip compares exactly.
    private func quantizedRide(id: String = "ride-1", pointCount: Int = 5) -> Ride {
        let start = Date(timeIntervalSince1970: 1_760_000_000)
        let points: [RidePoint] = (0..<pointCount).map { (i: Int) -> RidePoint in
            // Built exactly the way decode rebuilds them (i32 grid ÷ 1e7),
            // so the round-trip compares bit-for-bit.
            let lat: Double = Double(430_000_000 + i * 11_000) / 1e7
            let lon: Double = Double(-885_000_000 + i * 7_000) / 1e7
            return RidePoint(
                timestamp: start.addingTimeInterval(Double(i) * 60),
                coordinate: Coordinate(latitude: lat, longitude: lon),
                elevationMeters: i == 2 ? nil : Double(300 + i)
            )
        }
        let summary = RideSummary(
            id: RideID(id), name: "Kettle Moraine Loop", date: start,
            distanceMeters: 58_200, movingTime: 10_260,
            averageSpeedMps: 5.67, climbMeters: 812,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate))
        )
        return Ride(summary: summary, points: points)
    }

    func testRoundTripRestoresSummaryAndTracklog() throws {
        let ride = quantizedRide()
        let decoded = try ProvisionalRideCodec.decode(ProvisionalRideCodec.encode(ride), id: ride.id)
        XCTAssertEqual(decoded, ride)
    }

    func testRoundTripEmptyTracklogAndUnicodeName() throws {
        var ride = quantizedRide(pointCount: 0)
        ride.summary.name = "Feierabendrunde 🚲"
        ride.summary.trackPreview = .empty
        let decoded = try ProvisionalRideCodec.decode(ProvisionalRideCodec.encode(ride), id: ride.id)
        XCTAssertEqual(decoded, ride)
        XCTAssertTrue(decoded.points.isEmpty)
    }

    func testMissingElevationSurvivesAsNilNotZero() throws {
        let ride = quantizedRide()
        let decoded = try ProvisionalRideCodec.decode(ProvisionalRideCodec.encode(ride), id: ride.id)
        XCTAssertNil(decoded.points[2].elevationMeters)
        XCTAssertEqual(decoded.points[0].elevationMeters, 300)
    }

    func testQuantizationStaysWithinTheWireGrid() throws {
        // Off-grid values land within one grid step — never garbage, never a throw.
        let start = Date(timeIntervalSince1970: 1_760_000_000.4)
        let point = RidePoint(timestamp: start.addingTimeInterval(59.7),
                              coordinate: Coordinate(latitude: 43.12345678, longitude: -88.98765432),
                              elevationMeters: 300.49)
        let ride = Ride(
            summary: RideSummary(id: RideID("q"), name: "Q", date: start,
                                 distanceMeters: 1000.4, movingTime: 60.2,
                                 averageSpeedMps: 5.678, climbMeters: 10.6,
                                 trackPreview: TrackPreview.normalizing([point.coordinate])),
            points: [point]
        )
        let decoded = try ProvisionalRideCodec.decode(ProvisionalRideCodec.encode(ride), id: ride.id)
        XCTAssertEqual(decoded.summary.distanceMeters, 1000, accuracy: 0.5)
        XCTAssertEqual(decoded.summary.averageSpeedMps, 5.678, accuracy: 0.005)
        XCTAssertEqual(decoded.points[0].coordinate.latitude, 43.12345678, accuracy: 1e-7)
        XCTAssertEqual(decoded.points[0].coordinate.longitude, -88.98765432, accuracy: 1e-7)
        XCTAssertEqual(decoded.points[0].elevationMeters ?? 0, 300.49, accuracy: 0.5)
        XCTAssertEqual(decoded.points[0].timestamp.timeIntervalSince(decoded.summary.date), 60, accuracy: 0.5)
    }

    func testDecodeRebuildsTheTrackPreviewFromPoints() throws {
        var ride = quantizedRide()
        ride.summary.trackPreview = nil  // the wire object carries no preview
        let decoded = try ProvisionalRideCodec.decode(ProvisionalRideCodec.encode(ride), id: ride.id)
        XCTAssertEqual(decoded.summary.trackPreview,
                       TrackPreview.normalizing(decoded.points.map(\.coordinate)))
    }

    func testMalformedPayloadsThrowNotCrash() {
        let good = ProvisionalRideCodec.encode(quantizedRide())
        XCTAssertThrowsError(try ProvisionalRideCodec.decode(Data(), id: RideID("x")))
        XCTAssertThrowsError(try ProvisionalRideCodec.decode(Data([0xFF]), id: RideID("x")),
                             "unknown version must be rejected")
        XCTAssertThrowsError(try ProvisionalRideCodec.decode(good.prefix(good.count / 2), id: RideID("x")),
                             "a truncated tracklog must not decode")
        var extra = good
        extra.append(0)
        XCTAssertThrowsError(try ProvisionalRideCodec.decode(extra, id: RideID("x")),
                             "trailing bytes mean a layout mismatch — reject them")
    }

    func testDecodeTakesTheIdFromTheEnvelopeNotThePayload() throws {
        let ride = quantizedRide(id: "original")
        let decoded = try ProvisionalRideCodec.decode(ProvisionalRideCodec.encode(ride), id: RideID("envelope"))
        XCTAssertEqual(decoded.id, RideID("envelope"))
    }
}
