import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The app half of the shared OBCU pin: `specs/vectors/update-container-v2.bin` (the
/// signed container a release publishes) and `update-container-v1.bin` (unsigned — the
/// shape the device refuses, and the other half of the §1.2 offset-compatibility pin).
/// The same bytes `obc-dfu` decodes in `cargo test -p obc-dfu --test vectors` and
/// `obc-vectors` regenerates. A drift on either side goes red, so the files are the
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
        let container = try fixture("update-container-v2.bin")
        #expect(container.count == 256) // 64 header + 128 image + 64 signature

        let header = try #require(OBCUHeader.decode(container.prefix(OBCUHeader.length)))
        #expect(header.fwVersion == "1.2.0+abc1234")
        #expect(header.imageLength == 128)
        #expect(header.imageCRC32 == 0x5B99_0292)
        #expect(header.sigScheme == OBCUHeader.sigSchemeEd25519)
        #expect(header.sigLength == 64)

        // The header CRC (bytes 0..60) is the one the fixture stores.
        #expect(CRC32.checksum(container.prefix(OBCUHeader.headerCRCLength)) == 0x53AF_7E37)
    }

    /// §1.2, from the app's side: a v2 header still reads as header-version `1`, and every
    /// field this decoder looks at is byte-identical to the v1 fixture's. That is the
    /// property that lets a bootloader flashed before v2 existed install a v2 image.
    @Test func readsEveryV1FieldIdenticallyFromBothContainers() throws {
        let v1 = try fixture("update-container-v1.bin")
        let v2 = try fixture("update-container-v2.bin")
        let h1 = try #require(OBCUHeader.decode(v1.prefix(OBCUHeader.length)))
        let h2 = try #require(OBCUHeader.decode(v2.prefix(OBCUHeader.length)))

        #expect(h1.fwVersion == h2.fwVersion)
        #expect(h1.imageLength == h2.imageLength)
        #expect(h1.imageCRC32 == h2.imageCRC32)
        // Header bytes 0..48 are literally the same bytes; only the marker and CRC move.
        #expect(Array(v1.prefix(48)) == Array(v2.prefix(48)))
        #expect(h1.sigScheme == OBCUHeader.sigSchemeNone)
        #expect(h2.sigScheme == OBCUHeader.sigSchemeEd25519)
        // And the image sits at the same offset in both.
        let imageV1 = v1[(v1.startIndex + OBCUHeader.length)...]
        let imageV2 = v2[(v2.startIndex + OBCUHeader.length) ..< (v2.startIndex + OBCUHeader.length + 128)]
        #expect(Array(imageV1) == Array(imageV2))
    }

    @Test func validatesBothCRCsAndReadsTheVersionAndSize() throws {
        let container = try fixture("update-container-v2.bin")
        let staged = try StagedFirmware.validate(container)
        #expect(staged.version == "1.2.0+abc1234")
        #expect(staged.imageByteCount == 128)
        #expect(staged.byteCount == 256, "the staged container includes the signature trailer")
        #expect(staged.container == container)
    }

    @Test func rejectsBadMagic() throws {
        var container = try fixture("update-container-v2.bin")
        container[0] = UInt8(ascii: "X")
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
        #expect(throws: FirmwareImageError.notOBCU) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsWrongHeaderVersion() throws {
        var container = try fixture("update-container-v2.bin")
        container[4] = 2 // header_version 2
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
        #expect(throws: FirmwareImageError.notOBCU) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsCorruptHeaderCRC() throws {
        var container = try fixture("update-container-v2.bin")
        container[8] ^= 0xFF // flip image_len without fixing the header CRC
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
    }

    @Test func rejectsCorruptImageBody() throws {
        var container = try fixture("update-container-v2.bin")
        container[OBCUHeader.length + 3] ^= 0xFF // header still valid; the image CRC now fails
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) != nil)
        #expect(throws: FirmwareImageError.imageCRCMismatch) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsTruncatedFile() throws {
        let container = try fixture("update-container-v2.bin")
        // Drop the last 10 image bytes: the header says 128, the body is now short.
        let short = container.prefix(container.count - 10)
        #expect(throws: FirmwareImageError.truncated) { try StagedFirmware.validate(Data(short)) }
        // The whole trailer missing is the same verdict — a v1-length file whose header
        // promises a signature (the regression that a naive `64 + image_len` trim causes).
        let noTrailer = container.prefix(OBCUHeader.length + 128)
        #expect(throws: FirmwareImageError.truncated) { try StagedFirmware.validate(Data(noTrailer)) }
    }

    @Test func rejectsTooSmall() {
        #expect(throws: FirmwareImageError.tooSmall) { try StagedFirmware.validate(Data(count: 32)) }
    }

    /// §1.4: the device installs signed containers only, so the picker refuses an unsigned
    /// one rather than spending a transfer on a file that will be refused on arrival.
    @Test func rejectsAnUnsignedContainer() throws {
        let v1 = try fixture("update-container-v1.bin")
        #expect(OBCUHeader.decode(v1.prefix(OBCUHeader.length)) != nil, "it still *decodes*")
        #expect(throws: FirmwareImageError.unsigned) { try StagedFirmware.validate(v1) }
    }

    /// Trailing bytes past `64 + image_len + sig_len` are FAT-cluster slack, not an error
    /// (`OBCU_Spec.md` §1.1; the firmware armer accepts `file_len >= container_len`).
    /// The container is trimmed to exactly the container length — the signature trailer is
    /// **kept**, only the slack past it is dropped (DR5, #733; #997).
    @Test func acceptsTrailingSlackAndTrimsToExactLength() throws {
        let container = try fixture("update-container-v2.bin")
        var padded = container
        padded.append(Data(repeating: 0xAB, count: 512)) // FAT-cluster slack
        #expect(padded.count == 256 + 512)

        let staged = try StagedFirmware.validate(padded)
        // Validates despite the slack, and the staged container is trimmed back to exactly
        // header + image + signature (256) — the transfer never streams the 512 slack bytes.
        #expect(staged.imageByteCount == 128)
        #expect(staged.byteCount == 256)
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
