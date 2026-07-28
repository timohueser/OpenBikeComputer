import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The app half of the shared OBCU pin: `specs/vectors/update-container-v1.bin`
/// — the same bytes `obc-dfu` decodes in `cargo test -p obc-dfu --test vectors` and
/// `obc-vectors` regenerates. A drift on either side goes red, so the file is the
/// firmware↔app contract for the update container (`OBCU_Spec.md` §1 / spec §7.6).
struct OBCUHeaderTests {
    /// `specs/vectors/`, resolved from this file's location
    /// (companion-ios/Packages/OBCKit/Tests/OBCTransportTests/…).
    private static let vectorsDir = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // OBCTransportTests
        .deletingLastPathComponent() // Tests
        .deletingLastPathComponent() // OBCKit
        .deletingLastPathComponent() // Packages
        .deletingLastPathComponent() // companion-ios
        .deletingLastPathComponent() // repo root
        .appendingPathComponent("specs/vectors")

    private func fixture(_ name: String) throws -> Data {
        let url = Self.vectorsDir.appendingPathComponent(name)
        let data = try #require(
            FileManager.default.contents(atPath: url.path),
            "fixture \(name) missing at \(url.path) — run `cargo test -p obc-vectors regenerate -- --ignored`"
        )
        return data
    }

    @Test func decodesTheSharedContainerToPinnedFields() throws {
        let container = try fixture("update-container-v1.bin")
        #expect(container.count == 192)

        let header = try #require(OBCUHeader.decode(container.prefix(OBCUHeader.length)))
        #expect(header.fwVersion == "1.2.0+abc1234")
        #expect(header.imageLength == 128)
        #expect(header.imageCRC32 == 0x5B99_0292)

        // The header CRC (bytes 0..60) is the one the fixture stores.
        #expect(CRC32.checksum(container.prefix(OBCUHeader.headerCRCLength)) == 0x56B0_35F8)
    }

    @Test func validatesBothCRCsAndReadsTheVersionAndSize() throws {
        let container = try fixture("update-container-v1.bin")
        let staged = try StagedFirmware.validate(container)
        #expect(staged.version == "1.2.0+abc1234")
        #expect(staged.imageByteCount == 128)
        #expect(staged.byteCount == 192)
        #expect(staged.container == container)
    }

    @Test func rejectsBadMagic() throws {
        var container = try fixture("update-container-v1.bin")
        container[0] = UInt8(ascii: "X")
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
        #expect(throws: FirmwareImageError.notOBCU) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsWrongHeaderVersion() throws {
        var container = try fixture("update-container-v1.bin")
        container[4] = 2 // header_version 2
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
        #expect(throws: FirmwareImageError.notOBCU) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsCorruptHeaderCRC() throws {
        var container = try fixture("update-container-v1.bin")
        container[8] ^= 0xFF // flip image_len without fixing the header CRC
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
    }

    @Test func rejectsCorruptImageBody() throws {
        var container = try fixture("update-container-v1.bin")
        let last = container.count - 1
        container[last] ^= 0xFF // header still valid; the image CRC now fails
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) != nil)
        #expect(throws: FirmwareImageError.imageCRCMismatch) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsTruncatedFile() throws {
        let container = try fixture("update-container-v1.bin")
        // Drop the last 10 image bytes: the header says 128, the body is now short.
        let short = container.prefix(container.count - 10)
        #expect(throws: FirmwareImageError.truncated) { try StagedFirmware.validate(Data(short)) }
    }

    @Test func rejectsTooSmall() {
        #expect(throws: FirmwareImageError.tooSmall) { try StagedFirmware.validate(Data(count: 32)) }
    }

    /// Trailing bytes past `64 + image_len` are FAT-cluster slack, not an error
    /// (`OBCU_Spec.md` §1.1; the firmware armer accepts `file_len >= 64 + Len`).
    /// The container is trimmed to exactly `64 + image_len`, so only those bytes
    /// stream — the slack never reaches the CRC or the wire (DR5, #733).
    @Test func acceptsTrailingSlackAndTrimsToExactLength() throws {
        let container = try fixture("update-container-v1.bin")
        var padded = container
        padded.append(Data(repeating: 0xAB, count: 512)) // FAT-cluster slack
        #expect(padded.count == 192 + 512)

        let staged = try StagedFirmware.validate(padded)
        // Validates despite the slack, and the staged container is trimmed back to
        // exactly header + image (192) — the transfer never streams the 512 slack bytes.
        #expect(staged.imageByteCount == 128)
        #expect(staged.byteCount == 192)
        #expect(staged.container == container)
    }

    /// The `installFw` reply-code mapping (spec §4.3 → §4.4): each `commandResult`
    /// status maps to exactly one request outcome.
    @Test(arguments: [
        (CommandResult.Status.ok, FirmwareInstallResult.accepted),
        (.notFound, .noStaged),
        (.busy, .busy),
        (.error, .rejected),
        (.unknownCommand, .unsupported),
    ])
    func mapsInstallReplyCodes(status: CommandResult.Status, expected: FirmwareInstallResult) {
        #expect(FirmwareInstallResult(commandStatus: status) == expected)
    }
}
