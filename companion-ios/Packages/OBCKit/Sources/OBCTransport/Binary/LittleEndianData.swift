import Foundation

// Internal fixed-width wire primitives for codecs that validate their complete shape before
// reaching a field. Direct indexing preserves their existing trapping behavior for programming
// errors; decoders keep their format-specific truncation checks at the call site.
extension Data {
    mutating func appendUInt16LE(_ value: UInt16) {
        append(UInt8(value & 0xFF))
        append(UInt8((value >> 8) & 0xFF))
    }

    mutating func appendUInt32LE(_ value: UInt32) {
        append(UInt8(value & 0xFF))
        append(UInt8((value >> 8) & 0xFF))
        append(UInt8((value >> 16) & 0xFF))
        append(UInt8((value >> 24) & 0xFF))
    }

    mutating func appendUInt64LE(_ value: UInt64) {
        for shift in stride(from: 0, to: 64, by: 8) {
            append(UInt8((value >> UInt64(shift)) & 0xFF))
        }
    }

    mutating func writeUInt16LE(_ value: UInt16, at offset: Int) {
        let index = startIndex + offset
        self[index] = UInt8(value & 0xFF)
        self[index + 1] = UInt8((value >> 8) & 0xFF)
    }

    mutating func writeUInt32LE(_ value: UInt32, at offset: Int) {
        let index = startIndex + offset
        self[index] = UInt8(value & 0xFF)
        self[index + 1] = UInt8((value >> 8) & 0xFF)
        self[index + 2] = UInt8((value >> 16) & 0xFF)
        self[index + 3] = UInt8((value >> 24) & 0xFF)
    }

    func readUInt16LE(at index: Index) -> UInt16 {
        UInt16(self[index]) | (UInt16(self[index + 1]) << 8)
    }

    func readUInt32LE(at index: Index) -> UInt32 {
        UInt32(self[index]) | (UInt32(self[index + 1]) << 8)
            | (UInt32(self[index + 2]) << 16) | (UInt32(self[index + 3]) << 24)
    }

    func readUInt64LE(at index: Index) -> UInt64 {
        var value: UInt64 = 0
        for byte in 0..<8 {
            value |= UInt64(self[index + byte]) << UInt64(byte * 8)
        }
        return value
    }
}
