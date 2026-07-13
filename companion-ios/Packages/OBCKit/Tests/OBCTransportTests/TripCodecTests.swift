import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The Swift half of the TR5 shared-vector pin for the trip objects: the
/// checked-in fixtures `protocol-vectors/trip-v1.bin` + `trip-list.bin` must
/// decode through the app's codecs to the values `manifest.json` states, and
/// re-encode **byte-exactly**. The firmware side pins the same files
/// (`cargo test -p obc-vectors`), so neither side can drift from the spec (§7.7
/// / §7.4) without a test going red. (Swift Testing, per the new-suite rule.)
struct TripCodecTests {
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
        let data = try #require(
            FileManager.default.contents(atPath: url.path),
            "fixture \(name) missing at \(url.path) — regenerate with `cargo test -p obc-vectors regenerate -- --ignored`")
        return data
    }

    // MARK: trip object (§7.7)

    @Test
    func tripObjectVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("trip-v1.bin")
        let trip = try TripObjectCodec.decode(bytes)

        #expect(trip.version == 1)
        #expect(trip.name == "Alpen Traverse")
        // Stage ids are byte-faithful — the dangling ref (99) is present, exactly
        // as stored (the device tolerates it on read, never rewrites the object).
        #expect(trip.stageObjectIDs == [DeviceObjectID(7), DeviceObjectID(8), DeviceObjectID(99)])

        // Re-encode from the decoded name + ids reproduces the fixture byte-for-byte…
        #expect(TripObjectCodec.encode(name: trip.name, deviceStageIDs: trip.stageObjectIDs) == bytes)
        // …and so does the `TripRecord` form (ride order = the record's stageIDs).
        let record = TripRecord(id: TripID("t1"), name: "Alpen Traverse", stageIDs: [])
        #expect(TripObjectCodec.encode(record, deviceStageIDs: trip.stageObjectIDs) == bytes)

        // The whole-object CRC the tripList fingerprint / OnDeviceState reads.
        #expect(CRC32.checksum(bytes) == 0xA3C5_D591)
        #expect(TripObjectCodec.payloadCRC(for: record, deviceStageIDs: trip.stageObjectIDs) == 0xA3C5_D591)
    }

    @Test
    func tripObjectRejectsTruncatedAndWrongVersion() throws {
        // A header that claims 3 stages but carries none → out-of-bounds → throws.
        var short = try fixture("trip-v1.bin").prefix(TripObjectCodec.headerLength)
        #expect(throws: DeviceError.self) { try TripObjectCodec.decode(Data(short)) }
        // A wrong version byte is rejected, never mis-decoded.
        short = try fixture("trip-v1.bin")
        var wrongVersion = Data(short)
        wrongVersion[wrongVersion.startIndex] = 2
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

    // MARK: tripList (§7.4)

    @Test
    func tripListVectorDecodesAndReEncodesByteExactly() throws {
        let bytes = try fixture("trip-list.bin")
        let entries = try TripList.decode(bytes)
        #expect(entries.count == 1)

        let tripBytes = try fixture("trip-v1.bin")
        #expect(entries[0] == TripListEntry(
            objectID: 1, byteLen: UInt32(tripBytes.count),
            totalDistanceMeters: 4414, totalAscentMeters: 152,
            stageCount: 3, name: "Alpen Traverse", crc32: 0xA3C5_D591
        ))
        // stage_count counts every stored stage (the dangling 99 included), while
        // the totals summed only the resolvable stages (7 + 8).
        #expect(entries[0].stageCount == 3)
        // The entry CRC is the trip object's own whole-object CRC.
        #expect(entries[0].crc32 == CRC32.checksum(tripBytes))

        #expect(TripList.encode(entries) == bytes)  // byte-exact re-encode
    }

    @Test
    func tripListDecodesToReconcileCatalog() throws {
        let bytes = try fixture("trip-list.bin")
        let catalog = try TripList.catalog(bytes)
        #expect(catalog == [TripCatalogEntry(
            id: DeviceObjectID(1), name: "Alpen Traverse",
            distanceMeters: 4414, elevationGainMeters: 152,
            stageCount: 3, crc32: 0xA3C5_D591
        )])
    }
}
