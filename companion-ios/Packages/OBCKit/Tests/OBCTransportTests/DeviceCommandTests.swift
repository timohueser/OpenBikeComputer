import Foundation
import OBCDomain
import Testing
@testable import OBCTransport

@Suite("BLE imperative commands")
struct DeviceCommandTests {
    private static let vectorsDirectory = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()  // OBCTransportTests
        .deletingLastPathComponent()  // Tests
        .deletingLastPathComponent()  // OBCKit
        .deletingLastPathComponent()  // Packages
        .deletingLastPathComponent()  // companion-ios
        .deletingLastPathComponent()  // repository root
        .appendingPathComponent("specs/vectors")

    @Test func setClockMatchesSharedProtocolVector() throws {
        let fixture = try Data(contentsOf:
            Self.vectorsDirectory.appendingPathComponent("command-set-clock.bin"))
        let sample = WallClockSample(utcSeconds: 1_783_598_400, offsetMinutes: 120)
        #expect(SetClockCommand.encode(sample) == fixture)
    }

    @Test func setClockUsesLittleEndianFieldsAndSignedOffset() {
        let encoded = SetClockCommand.encode(
            WallClockSample(utcSeconds: 0x0102_0304, offsetMinutes: -300))
        #expect(encoded == Data([5, 0x04, 0x03, 0x02, 0x01, 0xD4, 0xFE]))
    }

    @Test func forgetBondIsTheCommandByteOnly() {
        #expect(ForgetBondCommand.encode() == Data([4]))
    }

    @Test func wallClockSampleClampsInvalidHostValues() {
        let early = WallClockSample(
            date: Date(timeIntervalSince1970: 0),
            timeZone: TimeZone(identifier: "UTC")!)
        #expect(early.utcSeconds == 1_577_836_800)
        #expect(early.offsetMinutes == 0)

        let extremeOffset = TimeZone(secondsFromGMT: 15 * 60 * 60)!
        let late = WallClockSample(
            date: Date(timeIntervalSince1970: Double(UInt32.max) + 10),
            timeZone: extremeOffset)
        #expect(late.utcSeconds == UInt32.max)
        #expect(late.offsetMinutes == 840)
    }
}
