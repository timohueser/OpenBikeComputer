import Foundation
import Testing
import OBCDomain
import OBCTransport

/// The `OBCR_Spec.md` §1.2 estimate against the shared vector file, and the bike-type byte in
/// the OBCR header.
struct BikeTypeTests {
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // OBCTransportTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // OBCKit
        .deletingLastPathComponent()  // Packages
        .deletingLastPathComponent()  // companion-ios
        .deletingLastPathComponent()  // repo root
        .appendingPathComponent("specs/vectors")

    @Test
    func estimateMatchesTheSharedVectorFile() throws {
        let csv = try String(contentsOf: Self.vectorsDir.appendingPathComponent("eta.csv"), encoding: .utf8)
        let rows = csv.split(separator: "\n").drop { !$0.hasPrefix("bike,") }.dropFirst()
        for row in rows {
            let fields = row.split(separator: ",").map { UInt64($0.trimmingCharacters(in: .whitespaces))! }
            let bike = try #require(BikeType(rawValue: UInt8(fields[0])))
            let seconds = bike.estimatedSeconds(distanceMeters: UInt32(fields[1]), ascentMeters: UInt32(fields[2]))
            #expect(UInt64(seconds) == fields[3], "\(row)")
        }
        #expect(rows.count >= 4 * 10, "the vector file covers every type")
    }

    @Test
    func codecRoundTripsTheTypeAndRejectsUnknownValues() throws {
        let points = (0..<3).map {
            RoutePoint(coordinate: Coordinate(latitude: 47 + 0.01 * Double($0), longitude: 11), elevationMeters: 500)
        }
        var bytes = RouteObjectCodec.encode(points: points, waypoints: [], name: "Ridge Loop", bikeType: .mtb)
        #expect(bytes[7] == 2)
        #expect(try RouteObjectCodec.decode(bytes).bikeType == .mtb)
        bytes[7] = 4
        #expect(throws: DeviceError.self) { try RouteObjectCodec.decode(bytes) }
    }
}
