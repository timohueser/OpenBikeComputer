import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The `ackRides` command encoder (spec §4.4 cmd 2): wire layout pinned against
/// the shared `protocol-vectors/` fixtures (the firmware pins the same files),
/// plus the chunking rules that make a long possession list safe to split.
@Suite struct AckRidesCommandTests {
    /// `protocol-vectors/` at the repo root, resolved from this file's location
    /// (the `ProtocolVectorTests` convention).
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
            Issue.record("fixture \(name) missing at \(url.path) — regenerate with `cargo test -p obc-vectors regenerate -- --ignored`")
            throw DeviceError.readFailed
        }
        return data
    }

    /// The shared vector: three acked rides encode to the exact bytes the
    /// firmware's `AckRides::decode` test pins.
    @Test func vectorBytes() throws {
        let chunks = AckRidesCommand.chunks([3, 5, 9].map(DeviceObjectID.init))
        #expect(chunks == [try fixture("command-ack-rides.bin")])
    }

    /// Layout: `cmd u8 = 2 · count u8 · count × object_id u16 LE`.
    @Test func layout() {
        let chunk = AckRidesCommand.chunks([DeviceObjectID(0x1234)])[0]
        #expect(chunk == Data([2, 1, 0x34, 0x12]))
    }

    /// An empty possession list is no write at all — never a zero-count command.
    @Test func emptyListEncodesNoWrites() {
        #expect(AckRidesCommand.chunks([]).isEmpty)
    }

    /// Chunking: ≤ 31 ids per write (the device's 64-byte command value), every
    /// id exactly once, counts self-describing per chunk.
    @Test(arguments: [1, 31, 32, 70]) func chunking(count: Int) {
        let ids = (0..<count).map { DeviceObjectID(UInt16($0)) }
        let chunks = AckRidesCommand.chunks(ids)
        #expect(chunks.count == (count + 30) / 31)

        var decoded: [UInt16] = []
        for chunk in chunks {
            #expect(chunk[0] == AckRidesCommand.commandByte)
            let n = Int(chunk[1])
            #expect(n <= AckRidesCommand.maxIDsPerWrite)
            #expect(chunk.count == 2 + n * 2)
            for i in 0..<n {
                decoded.append(UInt16(chunk[2 + i * 2]) | (UInt16(chunk[3 + i * 2]) << 8))
            }
        }
        #expect(decoded == ids.map(\.raw))
    }
}
