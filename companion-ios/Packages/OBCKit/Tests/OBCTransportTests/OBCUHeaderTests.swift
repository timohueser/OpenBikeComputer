import Foundation
import Testing
import OBCDomain
@testable import OBCTransport

/// The app half of the shared OBCU pin: the signed container a release publishes
/// (`specs/vectors/update-container.bin`). `obc-vectors` regenerates it; a drift on either side
/// goes red.
struct OBCUHeaderTests {
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
            "fixture \(name) missing at \(url.path) — run `cargo run -p obc-vectors --example regenerate --locked`"
        )
        return data
    }

    @Test func decodesTheSharedContainerToPinnedFields() throws {
        let container = try fixture("update-container.bin")
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

    @Test func validatesBothCRCsAndReadsTheVersionAndSize() throws {
        let container = try fixture("update-container.bin")
        let staged = try StagedFirmware.validate(container)
        #expect(staged.version == "1.2.0+abc1234")
        #expect(staged.imageByteCount == 128)
        #expect(staged.byteCount == 256, "the staged container includes the signature trailer")
        #expect(staged.container == container)
    }

    @Test func rejectsBadMagic() throws {
        var container = try fixture("update-container.bin")
        container[0] = UInt8(ascii: "X")
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
        #expect(throws: FirmwareImageError.notOBCU) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsWrongHeaderVersion() throws {
        var container = try fixture("update-container.bin")
        container[4] = 2 // header_version 2
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
        #expect(throws: FirmwareImageError.notOBCU) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsCorruptHeaderCRC() throws {
        var container = try fixture("update-container.bin")
        container[8] ^= 0xFF // flip image_len without fixing the header CRC
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) == nil)
    }

    @Test func rejectsCorruptImageBody() throws {
        var container = try fixture("update-container.bin")
        container[OBCUHeader.length + 3] ^= 0xFF // header still valid; the image CRC now fails
        #expect(OBCUHeader.decode(container.prefix(OBCUHeader.length)) != nil)
        #expect(throws: FirmwareImageError.imageCRCMismatch) { try StagedFirmware.validate(container) }
    }

    @Test func rejectsTruncatedFile() throws {
        let container = try fixture("update-container.bin")
        // Drop the last 10 image bytes: the header says 128, the body is now short.
        let short = container.prefix(container.count - 10)
        #expect(throws: FirmwareImageError.truncated) { try StagedFirmware.validate(Data(short)) }
        // A file whose header promises a signature but ends at `64 + image_len` is truncated too.
        let noTrailer = container.prefix(OBCUHeader.length + 128)
        #expect(throws: FirmwareImageError.truncated) { try StagedFirmware.validate(Data(noTrailer)) }
    }

    @Test func rejectsTooSmall() {
        #expect(throws: FirmwareImageError.tooSmall) { try StagedFirmware.validate(Data(count: 32)) }
    }

    /// The device installs signed containers only, so the picker refuses an unsigned one rather
    /// than spending a transfer on it.
    @Test func rejectsAnUnsignedContainer() throws {
        // The signed fixture with its scheme marker cleared, its header CRC resealed and no trailer.
        let signed = try fixture("update-container.bin")
        var unsigned = Data(signed.prefix(OBCUHeader.length + 128))
        unsigned.replaceSubrange(48 ..< 52, with: [UInt8](repeating: 0, count: 4))
        let crc = CRC32.checksum(unsigned.prefix(OBCUHeader.headerCRCLength))
        unsigned.replaceSubrange(60 ..< 64, with: withUnsafeBytes(of: crc.littleEndian) { Array($0) })
        #expect(OBCUHeader.decode(unsigned.prefix(OBCUHeader.length)) != nil, "it still *decodes*")
        #expect(throws: FirmwareImageError.unsigned) { try StagedFirmware.validate(unsigned) }
    }

    /// Trailing bytes past `64 + image_len + sig_len` are slack, not an error. The
    /// container is trimmed to the exact container length; the signature trailer stays.
    @Test func acceptsTrailingSlackAndTrimsToExactLength() throws {
        let container = try fixture("update-container.bin")
        var padded = container
        padded.append(Data(repeating: 0xAB, count: 512)) // slack past the container
        #expect(padded.count == 256 + 512)

        let staged = try StagedFirmware.validate(padded)
        // The transfer never streams the 512 slack bytes.
        #expect(staged.imageByteCount == 128)
        #expect(staged.byteCount == 256)
        #expect(staged.container == container)
    }
}
