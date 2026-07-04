import Testing
@testable import OBCTransport

/// The whole-object CRC-32 (end-to-end integrity, beyond the BLE link CRC).
///
/// This is the in-repo **Swift Testing template** (#363): new suites use
/// `import Testing` + `@Test`/`#expect`; existing XCTest suites migrate only
/// when substantially rewritten anyway. Both frameworks run side by side in
/// the same test target under `swift test` (and therefore in CI) — nothing to
/// configure. Known interactions:
/// - `@MainActor` view-model suites: Swift Testing supports actor-isolated
///   tests, but the shape differs from XCTest expectations — document that
///   pattern when the first model suite actually migrates, not here.
/// - `@Test(arguments:)` below is the parameterization pattern; the
///   codec/protocol-vector tests are its natural future beneficiaries.
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
