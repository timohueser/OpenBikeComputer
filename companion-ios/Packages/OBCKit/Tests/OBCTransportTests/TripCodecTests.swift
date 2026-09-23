import Foundation
import Testing
@testable import OBCDomain
@testable import OBCTransport

/// The Swift half of the shared-vector pin for the trip object. Catalog metadata comes from v4
/// `LIST` entries.
struct TripCodecTests {
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
        let data = try #require(
            FileManager.default.contents(atPath: url.path),
            "fixture \(name) missing at \(url.path) — regenerate with `cargo run -p obc-vectors --example regenerate --locked`")
        return data
    }

    // MARK: trip object

    private static let vectorTrip = TripObjectCodec.Trip(
        key: 0x0123_4567_89AB_CDEF, name: "Alpen Traverse", startDate: 20_360,
        days: [
            .init(routeID: DeviceObjectID(7), joinMeters: 0, leaveMeters: 82_000),
            .init(routeID: DeviceObjectID(8), joinMeters: 0, leaveMeters: 73_600),
            // The full-width dangling ref is present exactly as stored. The device tolerates it
            // on read and never rewrites the object.
            .init(routeID: DeviceObjectID(0x1_0000_0063), joinMeters: 400, leaveMeters: .max),
        ])

    @Test
    func tripObjectVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("trip-v3.bin")
        #expect(try TripObjectCodec.decode(bytes) == Self.vectorTrip)
        #expect(TripObjectCodec.encode(Self.vectorTrip) == bytes)
        // The whole-object CRC protocol-v4 catalog metadata carries.
        #expect(TripObjectCodec.payloadCRC(Self.vectorTrip) == 0x965F_55AF)
    }

    @Test
    func tripObjectRejectsTornAndWrongVersion() throws {
        let bytes = try fixture("trip-v3.bin")
        // A torn write: the header claims three days, but the last day is cut short.
        #expect(throws: DeviceError.self) { try TripObjectCodec.decode(bytes.dropLast()) }
        #expect(throws: DeviceError.self) { try TripObjectCodec.decode(bytes + [0]) }
        var wrongVersion = bytes
        wrongVersion[wrongVersion.startIndex] = 2
        #expect(throws: DeviceError.self) { try TripObjectCodec.decode(wrongVersion) }
        let zeroKey = TripObjectCodec.encode(.init(key: 0, name: "X", startDate: 0, days: []))
        #expect(throws: DeviceError.self) { try TripObjectCodec.decode(zeroKey) }
    }

    @Test
    func tripKeyIsNeverZero() {
        #expect(Trip(id: TripID("t"), key: 0, name: "T", bikeType: .road, addedAt: Date()).key == 1)
        #expect(Trip(id: TripID("t"), key: 0xA1, name: "T", bikeType: .road, addedAt: Date()).key == 0xA1)
    }

    @Test
    func tripObjectEncodeTruncatesNameOnCharacterBoundary() throws {
        let long = String(repeating: "é", count: 40)  // 80 UTF-8 bytes
        let data = TripObjectCodec.encode(.init(key: 1, name: long, startDate: 0, days: []))
        let decoded = try TripObjectCodec.decode(data)
        // The 48-byte cap lands on a character boundary: 24 "é" at 2 bytes each.
        #expect(decoded.name == String(repeating: "é", count: 24))
    }
}
