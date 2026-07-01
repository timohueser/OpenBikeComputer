import Foundation

/// Whole-object CRC-32 — the **end-to-end** integrity check, verified once before
/// commit (`OBCProtocol.md` → *Bulk transfers*).
///
/// > **Not the on-air check.** The BLE Link Layer already CRCs every packet (24-bit)
/// > and retransmits, so the L2CAP CoC is a reliable, ordered stream. This CRC adds
/// > only what the link CRC can't: end-to-end coverage across the whole path
/// > (phone encode → BLE → MCU → flash) — logic bugs, storage write errors, residual
/// > undetected link errors. One CRC per *object*, never per chunk.
///
/// Standard **CRC-32/IEEE** (zlib/gzip/PNG). The `Hasher` streams chunk-by-chunk
/// with O(1) state — no full-object buffering — which is exactly how a RAM-limited
/// MCU verifies bytes as it writes them out.
///
/// > **Provisional pending firmware `S0`** (owns the CRC polynomial/seed). CRC-32/IEEE
/// > is the near-universal default; change it **here only** if `S0` differs.
public enum CRC32 {
    private static let table: [UInt32] = {
        (0..<256).map { i -> UInt32 in
            var c = UInt32(i)
            for _ in 0..<8 { c = (c & 1) != 0 ? (0xEDB8_8320 ^ (c >> 1)) : (c >> 1) }
            return c
        }
    }()

    /// CRC-32/IEEE of a whole buffer.
    public static func checksum<C: Collection>(_ bytes: C) -> UInt32 where C.Element == UInt8 {
        var hasher = Hasher()
        hasher.update(bytes)
        return hasher.finalize()
    }

    /// Incremental CRC-32/IEEE — feed chunks as they arrive, `finalize()` at the end.
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
