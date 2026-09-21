import Foundation

/// Whole-object CRC-32: the end-to-end integrity check, verified once before commit
/// (`OBCProtocol.md`). The BLE Link Layer already CRCs and retransmits every packet, so this
/// adds only what the link CRC cannot: coverage of the whole path from the phone's encode to
/// MCU flash. One CRC per object, never per chunk.
///
/// Standard CRC-32/IEEE: reflected, poly `0xEDB88320`, init and xorout `0xFFFFFFFF`, check
/// value `crc32("123456789") == 0xCBF43926`. The `Hasher` streams chunk by chunk with O(1)
/// state, the way a RAM-limited MCU verifies bytes as it writes them out.
public enum CRC32 {
    private static let table: [UInt32] = {
        (0..<256).map { i -> UInt32 in
            var c = UInt32(i)
            for _ in 0..<8 { c = (c & 1) != 0 ? (0xEDB8_8320 ^ (c >> 1)) : (c >> 1) }
            return c
        }
    }()

    public static func checksum<C: Collection>(_ bytes: C) -> UInt32 where C.Element == UInt8 {
        var hasher = Hasher()
        hasher.update(bytes)
        return hasher.finalize()
    }

    /// Incremental CRC-32/IEEE: feed chunks as they arrive, then `finalize()`.
    public struct Hasher: Sendable {
        private var crc: UInt32 = 0xFFFF_FFFF
        public init() {}

        public mutating func update<C: Collection>(_ bytes: C) where C.Element == UInt8 {
            var c = crc
            for byte in bytes { c = CRC32.table[Int((c ^ UInt32(byte)) & 0xFF)] ^ (c >> 8) }
            crc = c
        }

        public func finalize() -> UInt32 { crc ^ 0xFFFF_FFFF }
    }
}
