import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The `setClock` command encoder (spec §4.4 cmd 5, epic #638): wire layout pinned
/// against the shared `protocol-vectors/command-set-clock.bin` (the firmware pins
/// the same file), plus the offset sign + `WallClockSample` clamping.
@Suite struct SetClockCommandTests {
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

    /// The shared vector: `utc 1783598400 (2026-07-09T12:00:00Z) · offset +120`
    /// encodes to the exact bytes the firmware's decode test pins.
    @Test func vectorBytes() throws {
        let sample = WallClockSample(utcSeconds: 1_783_598_400, offsetMinutes: 120)
        #expect(SetClockCommand.encode(sample) == (try fixture("command-set-clock.bin")))
    }

    /// Layout: `cmd u8 = 5 · utc u32 LE · offset_min i16 LE` — 7 bytes.
    @Test func layout() {
        let encoded = SetClockCommand.encode(
            WallClockSample(utcSeconds: 0x0102_0304, offsetMinutes: 120))
        #expect(encoded == Data([5, 0x04, 0x03, 0x02, 0x01, 0x78, 0x00]))
        #expect(encoded.count == 7)
    }

    /// A **negative** offset (west of GMT) is two's-complement i16 LE — `-300`
    /// (−05:00) → `0xFED4` → bytes `D4 FE`.
    @Test func negativeOffsetIsTwosComplement() {
        let encoded = SetClockCommand.encode(
            WallClockSample(utcSeconds: 0, offsetMinutes: -300))
        #expect(Array(encoded.suffix(2)) == [0xD4, 0xFE])
    }

    /// `WallClockSample(date:timeZone:)` clamps into the wire's valid range so a
    /// bogus host clock still encodes to something the device accepts (spec §4.4
    /// rejects `utc < 1577836800` and `|offset| > 840`), never a rejected prologue.
    @Test func sampleClampsAPreEpochDate() {
        let sample = WallClockSample(
            date: Date(timeIntervalSince1970: 0),
            timeZone: TimeZone(identifier: "UTC")!)
        #expect(sample.utcSeconds == 1_577_836_800)
        #expect(sample.offsetMinutes == 0)
    }

    /// A real "now" sample lands a plausible, in-range value (2020-01-01 is the
    /// wire floor; the offset stays within ±840).
    @Test func sampleNowIsInRange() {
        let sample = WallClockSample()
        #expect(sample.utcSeconds >= 1_577_836_800)
        #expect(sample.offsetMinutes >= -840 && sample.offsetMinutes <= 840)
    }
}
