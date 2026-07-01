import Foundation

/// CRC-32 over the frame header + payload — the integrity check the framing layer
/// validates **before commit** (see `OBCProtocol.md` → *CoC framing*).
///
/// This is the standard **CRC-32/IEEE** (a.k.a. zlib / gzip / PNG): reflected in
/// and out, polynomial `0xEDB88320`, init `0xFFFFFFFF`, final XOR `0xFFFFFFFF`.
///
/// > **Provisional pending firmware `S0`.** `OBCProtocol.md` names the CRC
/// > polynomial/seed as firmware-owned. CRC-32/IEEE is the near-universal default;
/// > if `S0` freezes a different variant, change it **here only** — the framing and
/// > every test go through `CRC32.checksum`.
public enum CRC32 {
    private static let table: [UInt32] = {
        (0..<256).map { i -> UInt32 in
            var c = UInt32(i)
            for _ in 0..<8 {
                c = (c & 1) != 0 ? (0xEDB8_8320 ^ (c >> 1)) : (c >> 1)
            }
            return c
        }
    }()

    /// CRC-32/IEEE of `bytes`.
    public static func checksum<C: Collection>(_ bytes: C) -> UInt32 where C.Element == UInt8 {
        var crc: UInt32 = 0xFFFF_FFFF
        for byte in bytes {
            crc = table[Int((crc ^ UInt32(byte)) & 0xFF)] ^ (crc >> 8)
        }
        return crc ^ 0xFFFF_FFFF
    }
}
