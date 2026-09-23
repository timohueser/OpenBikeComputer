import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The single cross-language ride-object contract: recorded 20-byte samples plus the v5 footer.
struct RideCodecVectorTests {
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("specs/vectors")

    private func vector() throws -> Data {
        let url = Self.vectorsDir.appendingPathComponent("ride-v5.bin")
        return try #require(FileManager.default.contents(atPath: url.path),
                            "fixture ride-v5.bin missing at \(url.path)")
    }

    @Test func decodesAndReencodesTheVectorExactly() throws {
        let bytes = try vector()
        #expect(bytes.count == 3 * RideObjectCodec.sampleLength + RideObjectCodec.footerLength)
        let ride = try RideObjectCodec.decode(bytes, id: RideID("42"))
        let summary = ride.summary
        #expect(summary.name == "Sensor Ride")
        #expect(summary.date == Date(timeIntervalSince1970: 1_751_460_000))
        #expect(summary.distanceMeters == 12_345)
        #expect(summary.movingTime == 3_600)
        #expect(abs(summary.averageSpeedMps - 3.43) < 0.001)
        #expect(summary.climbMeters == 120)
        #expect(summary.descentMeters == 95)
        #expect(summary.avgHeartRate == 142)
        #expect(summary.maxHeartRate == 176)
        #expect(summary.avgCadence == 85)
        #expect(summary.avgPower == 210)
        #expect(summary.maxPower == 480)
        #expect(summary.energyKJ == 756)
        #expect(summary.bikeType == .gravel)
        #expect(summary.trip == RideTrip(key: 0x0123_4567_89AB_CDEF, dayIndex: 1, dayCount: 3,
                                         name: "Alpen Traverse"))

        #expect(ride.points.count == 3)
        #expect(ride.points.map(\.segmentStart) == [true, false, true])
        #expect(ride.points[0].coordinate == Coordinate(latitude: 48, longitude: 7.8))
        #expect(ride.points[0].heartRate == 140)
        #expect(ride.points[0].cadence == 84)
        #expect(ride.points[0].power == 205)
        #expect(ride.points[1].heartRate == nil)
        #expect(ride.points[1].cadence == nil)
        #expect(ride.points[1].power == nil)
        #expect(ride.points[2].timestamp.timeIntervalSince(summary.date) == 120)
        #expect(RideObjectCodec.encode(ride) == bytes)
    }

    @Test func rejectsBadFooterAndReservedSampleFlags() throws {
        let bytes = try vector()
        let footer = bytes.count - RideObjectCodec.footerLength
        // Reserved byte, a day past the count, a bike type past the four, trip-name padding.
        for (offset, value) in [(33, 1), (98, 3), (100, 4), (149, 1)] as [(Int, UInt8)] {
            var badFooter = bytes
            badFooter[footer + offset] = value
            #expect(throws: (any Error).self) {
                try RideObjectCodec.decode(badFooter, id: RideID("x"))
            }
        }

        var badFlags = bytes
        badFlags[10] = 2
        #expect(throws: (any Error).self) {
            try RideObjectCodec.decode(badFlags, id: RideID("x"))
        }
    }
}
