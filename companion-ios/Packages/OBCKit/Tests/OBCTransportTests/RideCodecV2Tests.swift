import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The **ride object v2** codec (epic #707, SE4 #711): the app must accept the
/// BLE-sensor ride the SE3 firmware writes — a per-ride sensor summary in the
/// header, per-point `hr`/`cad`/`pwr`. These pins hold the Swift decode to the
/// cross-language contract `protocol-vectors/ride-v2.bin` (the firmware side
/// pins the same bytes), plus a v2-specific round-trip / rejection net. The v1
/// vector + round-trip stay in `ProtocolVectorTests` / `RideCodecTests` and must
/// remain green — v1 rides still list, download, and delete.
struct RideCodecV2Tests {
    /// `protocol-vectors/` at the repo root, resolved from this file's location
    /// (companion-ios/Packages/OBCKit/Tests/OBCTransportTests/…) — the same
    /// traversal `ProtocolVectorTests` uses.
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // OBCTransportTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // OBCKit
        .deletingLastPathComponent()  // Packages
        .deletingLastPathComponent()  // companion-ios
        .deletingLastPathComponent()  // repo root
        .appendingPathComponent("protocol-vectors")

    private func vector(_ name: String) throws -> Data {
        let url = Self.vectorsDir.appendingPathComponent(name)
        return try #require(FileManager.default.contents(atPath: url.path),
                            "fixture \(name) missing at \(url.path)")
    }

    // MARK: The cross-language contract

    @Test func decodesRideV2VectorToTheManifestValues() throws {
        let bytes = try vector("ride-v2.bin")
        #expect(bytes.count == 96)  // 31 header + 11 name + 3 × 18 points

        let ride = try RideObjectCodec.decode(bytes, id: RideID("42"))
        let s = ride.summary
        #expect(s.name == "Sensor Ride")
        #expect(s.date == Date(timeIntervalSince1970: 1_751_460_000))
        #expect(s.distanceMeters == 12_345)
        #expect(s.movingTime == 3_600)
        #expect(abs(s.averageSpeedMps - 3.43) < 0.001)
        #expect(s.climbMeters == 120)

        // The v2 header's per-ride sensor summary.
        #expect(s.avgHeartRate == 142)
        #expect(s.maxHeartRate == 176)
        #expect(s.avgCadence == 85)
        #expect(s.avgPower == 210)
        #expect(s.maxPower == 480)

        #expect(ride.points.count == 3)

        // Point 0 — everything present.
        let p0 = ride.points[0]
        #expect(abs(p0.coordinate.latitude - 48.0) < 1e-7)
        #expect(abs(p0.coordinate.longitude - 7.8) < 1e-7)
        #expect(p0.elevationMeters == 214)
        #expect(p0.heartRate == 140)
        #expect(p0.cadence == 84)
        #expect(p0.power == 205)

        // Point 1 — a dropped fix: every sensor absent (sentinels → nil), and a
        // real elevation still present (the ele + sensor sentinels are independent).
        let p1 = ride.points[1]
        #expect(p1.timestamp.timeIntervalSince(s.date) == 60)
        #expect(p1.elevationMeters == 219)
        #expect(p1.heartRate == nil)
        #expect(p1.cadence == nil)
        #expect(p1.power == nil)

        // Point 2 — partial: hr + pwr present, cadence absent, no elevation. The
        // elevation sentinel is independent of the sensor sentinels.
        let p2 = ride.points[2]
        #expect(p2.timestamp.timeIntervalSince(s.date) == 120)
        #expect(p2.elevationMeters == nil)
        #expect(p2.heartRate == 150)
        #expect(p2.cadence == nil)
        #expect(p2.power == 215)

        // Every decoded value sits exactly on the wire grid, so re-encoding must
        // reproduce the fixture byte-for-byte — the SE3↔SE4 contract.
        #expect(RideObjectCodec.encode(ride) == bytes)
    }

    // MARK: The ride-v2.json expected-decode pin

    @Test func decodeMatchesTheExpectedJSON() throws {
        let bytes = try vector("ride-v2.bin")
        let ride = try RideObjectCodec.decode(bytes, id: RideID("42"))

        let url = try #require(Bundle.module.url(
            forResource: "ride-v2", withExtension: "json", subdirectory: "Fixtures"))
        let expected = try JSONDecoder().decode(ExpectedRide.self, from: Data(contentsOf: url))

        #expect(expected.version == 2)
        let s = ride.summary
        #expect(s.name == expected.summary.name)
        #expect(s.date.timeIntervalSince1970 == expected.summary.startTimeUnix)
        #expect(s.distanceMeters == expected.summary.distanceMeters)
        #expect(s.movingTime == expected.summary.movingTimeSeconds)
        #expect(abs(s.averageSpeedMps - expected.summary.averageSpeedMps) < 0.001)
        #expect(s.climbMeters == expected.summary.climbMeters)
        #expect(s.avgHeartRate == expected.summary.avgHeartRate)
        #expect(s.maxHeartRate == expected.summary.maxHeartRate)
        #expect(s.avgCadence == expected.summary.avgCadence)
        #expect(s.avgPower == expected.summary.avgPower)
        #expect(s.maxPower == expected.summary.maxPower)

        #expect(ride.points.count == expected.points.count)
        for (point, want) in zip(ride.points, expected.points) {
            #expect(point.timestamp.timeIntervalSince(s.date) == want.tOffsetSeconds)
            #expect(abs(point.coordinate.latitude - want.latitude) < 1e-7)
            #expect(abs(point.coordinate.longitude - want.longitude) < 1e-7)
            #expect(point.elevationMeters == want.elevationMeters)
            #expect(point.heartRate == want.heartRate)
            #expect(point.cadence == want.cadence)
            #expect(point.power == want.power)
        }
    }

    // MARK: Round-trip + rejection

    @Test func roundTripsAMixedSensorRide() throws {
        let start = Date(timeIntervalSince1970: 1_760_000_000)
        // Built with an explicit loop + typed intermediates on purpose: a single
        // `.map` closure with inline arithmetic and four optional ternaries blows
        // the Swift type-checker's budget (it timed out in CI).
        var points: [RidePoint] = []
        for i in 0..<4 {
            let timestamp: Date = start.addingTimeInterval(Double(i) * 30)
            let latitude: Double = Double(430_000_000 + i * 11_000) / 1e7
            let longitude: Double = Double(-885_000_000 + i * 7_000) / 1e7
            let elevation: Double? = i == 1 ? nil : Double(300 + i)
            let heartRate: Int? = i == 2 ? nil : 130 + i
            let cadence: Int? = i == 3 ? nil : 80 + i
            let power: Int? = i == 0 ? nil : 200 + i
            points.append(RidePoint(
                timestamp: timestamp,
                coordinate: Coordinate(latitude: latitude, longitude: longitude),
                elevationMeters: elevation,
                heartRate: heartRate,
                cadence: cadence,
                power: power))
        }
        let summary = RideSummary(
            id: RideID("m"), name: "Mixed", date: start,
            distanceMeters: 12_000, movingTime: 3_600,
            averageSpeedMps: 3.33, climbMeters: 90,
            trackPreview: TrackPreview.normalizing(points.map(\.coordinate)),
            avgHeartRate: 141, maxHeartRate: 170, avgCadence: 82,
            avgPower: 205, maxPower: 460
        )
        let ride = Ride(summary: summary, points: points)

        let encoded = RideObjectCodec.encode(ride)
        #expect(encoded.first == 2, "a ride carrying any sensor value encodes as v2")
        let decoded = try RideObjectCodec.decode(encoded, id: ride.id)
        #expect(decoded == ride)
    }

    @Test func aSensorlessRideStaysV1() throws {
        let start = Date(timeIntervalSince1970: 1_760_000_000)
        let point = RidePoint(
            timestamp: start, coordinate: Coordinate(latitude: 43.0, longitude: -88.0),
            elevationMeters: 300)
        let ride = Ride(
            summary: RideSummary(id: RideID("v1"), name: "Plain", date: start,
                                 distanceMeters: 1_000, movingTime: 300,
                                 averageSpeedMps: 3.3, climbMeters: 10,
                                 trackPreview: TrackPreview.normalizing([point.coordinate])),
            points: [point])
        let encoded = RideObjectCodec.encode(ride)
        #expect(encoded.first == 1, "no sensor data anywhere → the object stays v1")
        // 23 header + 5 name + 14 point.
        #expect(encoded.count == 23 + 5 + 14)
        #expect(try RideObjectCodec.decode(encoded, id: ride.id) == ride)
    }

    @Test func rejectsAV2PayloadOfTheWrongLength() throws {
        let bytes = try vector("ride-v2.bin")
        // One byte short — the fixed per-version layout no longer adds up.
        #expect(throws: (any Error).self) {
            try RideObjectCodec.decode(bytes.dropLast(), id: RideID("x"))
        }
        // A trailing byte is just as wrong.
        var extra = bytes
        extra.append(0)
        #expect(throws: (any Error).self) {
            try RideObjectCodec.decode(extra, id: RideID("x"))
        }
        // A v1-length payload must not be read as v2 (or vice versa): flip the
        // version byte of the v2 vector and the v2 length check fails.
        var mislabelled = bytes
        mislabelled[mislabelled.startIndex] = 1
        #expect(throws: (any Error).self) {
            try RideObjectCodec.decode(mislabelled, id: RideID("x"))
        }
    }
}

/// The `Fixtures/ride-v2.json` shape — the expected decode of `ride-v2.bin`.
private struct ExpectedRide: Decodable {
    struct Summary: Decodable {
        let name: String
        let startTimeUnix: Double
        let distanceMeters: Double
        let movingTimeSeconds: Double
        let averageSpeedMps: Double
        let climbMeters: Double
        let avgHeartRate: Int?
        let maxHeartRate: Int?
        let avgCadence: Int?
        let avgPower: Int?
        let maxPower: Int?
    }

    struct Point: Decodable {
        let tOffsetSeconds: Double
        let latitude: Double
        let longitude: Double
        let elevationMeters: Double?
        let heartRate: Int?
        let cadence: Int?
        let power: Int?
    }

    let version: Int
    let summary: Summary
    let points: [Point]
}
