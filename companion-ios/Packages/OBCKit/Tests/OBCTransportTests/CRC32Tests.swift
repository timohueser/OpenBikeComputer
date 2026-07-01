import XCTest
@testable import OBCTransport

/// The whole-object CRC-32 (end-to-end integrity, beyond the BLE link CRC).
final class CRC32Tests: XCTestCase {
    func testKnownVector() {
        // Canonical CRC-32/IEEE check value for "123456789".
        XCTAssertEqual(CRC32.checksum(Array("123456789".utf8)), 0xCBF4_3926)
        XCTAssertEqual(CRC32.checksum([]), 0)
    }

    func testStreamingEqualsOneShot() {
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
        XCTAssertEqual(hasher.finalize(), oneShot)
    }

    func testSingleBitFlipChangesChecksum() {
        var bytes = Array("a planned route payload".utf8)
        let original = CRC32.checksum(bytes)
        bytes[3] ^= 0x01
        XCTAssertNotEqual(CRC32.checksum(bytes), original)
    }
}
