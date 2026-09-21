import Testing
@testable import OBCTransport

/// The whole-object CRC-32: end-to-end integrity beyond the BLE link CRC.
struct CRC32Tests {
    /// Canonical CRC-32/IEEE check value for "123456789", plus the empty input.
    private static let knownVectors: [(bytes: [UInt8], checksum: UInt32)] = [
        (Array("123456789".utf8), 0xCBF4_3926),
        ([], 0),
    ]

    @Test(arguments: knownVectors)
    func knownVector(bytes: [UInt8], checksum: UInt32) {
        #expect(CRC32.checksum(bytes) == checksum)
    }

    @Test
    func streamingEqualsOneShot() {
        let bytes = (0..<5000).map { UInt8(($0 * 31 + 7) & 0xFF) }
        let oneShot = CRC32.checksum(bytes)

        // Feed in irregular chunks (what the MCU does streaming to flash).
        var hasher = CRC32.Hasher()
        var i = 0
        for size in [1, 7, 100, 993, 2048, 0, 1851] where i < bytes.count {
            let end = min(i + size, bytes.count)
            hasher.update(bytes[i..<end])
            i = end
        }
        hasher.update(bytes[i...])
        #expect(hasher.finalize() == oneShot)
    }

    @Test
    func singleBitFlipChangesChecksum() {
        var bytes = Array("a planned route payload".utf8)
        let original = CRC32.checksum(bytes)
        bytes[3] ^= 0x01
        #expect(CRC32.checksum(bytes) != original)
    }
}
