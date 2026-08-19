import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The Swift half of the TR5 shared-vector pin for the trip object. The v2 list-object transport
/// retired with protocol v4; catalog metadata now comes from v4 `LIST` entries.
struct TripCodecTests {
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
        let data = try #require(
            FileManager.default.contents(atPath: url.path),
            "fixture \(name) missing at \(url.path) — regenerate with `cargo test -p obc-vectors regenerate -- --ignored`")
        return data
    }

    // MARK: trip object (§7.7)

    @Test
    func tripObjectVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("trip-v2.bin")
        let trip = try TripObjectCodec.decode(bytes)

        #expect(trip.version == 2)
        #expect(trip.name == "Alpen Traverse")
        // Stage ids are byte-faithful — the full-width dangling ref is present, exactly
        // as stored (the device tolerates it on read, never rewrites the object).
        #expect(trip.stageObjectIDs == [DeviceObjectID(7), DeviceObjectID(8), DeviceObjectID(0x1_0000_0063)])

        // Re-encode from the decoded name + ids reproduces the fixture byte-for-byte.
        #expect(TripObjectCodec.encode(name: trip.name, deviceStageIDs: trip.stageObjectIDs) == bytes)

        // The whole-object CRC protocol-v4 catalog metadata carries.
        #expect(CRC32.checksum(bytes) == 0x5B1C_1606)
        #expect(TripObjectCodec.payloadCRC(name: trip.name, deviceStageIDs: trip.stageObjectIDs) == 0x5B1C_1606)
    }

    @Test
    func tripObjectRejectsTruncatedAndWrongVersion() throws {
        // A header that claims 3 stages but carries none → out-of-bounds → throws.
        var short = try fixture("trip-v2.bin").prefix(TripObjectCodec.headerLength)
        #expect(throws: DeviceError.self) { try TripObjectCodec.decode(Data(short)) }
        // A wrong version byte is rejected, never mis-decoded.
        short = try fixture("trip-v2.bin")
        var wrongVersion = Data(short)
        wrongVersion[wrongVersion.startIndex] = 1
        #expect(throws: DeviceError.self) { try TripObjectCodec.decode(wrongVersion) }
    }

    @Test
    func tripObjectEncodeTruncatesNameOnCharacterBoundary() {
        let long = String(repeating: "é", count: 40)  // 80 UTF-8 bytes
        let data = TripObjectCodec.encode(name: long, deviceStageIDs: [])
        let decoded = try! TripObjectCodec.decode(data)
        // 48-byte cap on a char boundary → 24 × "é" (2 bytes each) = 48 bytes.
        #expect(decoded.name == String(repeating: "é", count: 24))
        #expect(Array(decoded.name.utf8).count == 48)
    }
}
