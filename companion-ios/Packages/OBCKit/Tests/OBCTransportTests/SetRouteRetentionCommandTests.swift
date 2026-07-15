import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The `setRouteRetention` command encoder (spec §4.4 cmd 6, epic #638): wire
/// layout pinned against the shared `protocol-vectors/command-set-route-retention.bin`
/// (the firmware pins the same file), plus the object-id + retention-byte layout.
@Suite struct SetRouteRetentionCommandTests {
    /// `protocol-vectors/` at the repo root (the `ProtocolVectorTests` convention).
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

    /// The shared vector: `object_id 7 · retention 3 (two weeks)` encodes to the
    /// exact bytes the firmware's decode test pins.
    @Test func vectorBytes() throws {
        let encoded = SetRouteRetentionCommand.encode(objectID: DeviceObjectID(7), retention: .twoWeeks)
        #expect(encoded == (try fixture("command-set-route-retention.bin")))
    }

    /// Layout: `cmd u8 = 6 · object_id u16 LE · retention u8` — 4 bytes.
    @Test func layout() {
        let encoded = SetRouteRetentionCommand.encode(objectID: DeviceObjectID(0x1234), retention: .oneDay)
        #expect(encoded == Data([6, 0x34, 0x12, 1]))
    }

    /// Every wire level maps to its byte (`0` never … `5` two months).
    @Test(arguments: zip(
        [Retention.never, .oneDay, .oneWeek, .twoWeeks, .oneMonth, .twoMonths],
        [UInt8(0), 1, 2, 3, 4, 5]))
    func retentionByte(_ retention: Retention, _ raw: UInt8) {
        let encoded = SetRouteRetentionCommand.encode(objectID: DeviceObjectID(1), retention: retention)
        #expect(encoded[encoded.startIndex + 3] == raw)
    }
}
