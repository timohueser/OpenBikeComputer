import Foundation

/// Bounded little-endian reader. Every read is length-checked, so a malformed record can only
/// produce a `WireFault` — never an out-of-range trap.
struct ByteReader {
    let bytes: [UInt8]
    private(set) var index: Int
    /// What the caller is decoding; folded into the fault context.
    let subject: String

    init(_ bytes: [UInt8], subject: String) {
        self.bytes = bytes
        self.index = 0
        self.subject = subject
    }

    var remaining: Int { bytes.count - index }

    mutating func take(_ count: Int) throws -> ArraySlice<UInt8> {
        guard count >= 0, remaining >= count else {
            throw WireFault.truncated("\(subject): wanted \(count), \(remaining) left")
        }
        let slice = bytes[index..<(index + count)]
        index += count
        return slice
    }

    mutating func u8() throws -> UInt8 {
        // `take(1)` already guarantees one element on success; the guard keeps the module free of
        // force-unwraps outright rather than free of them by argument.
        guard let byte = try take(1).first else {
            throw WireFault.truncated("\(subject): u8")
        }
        return byte
    }

    mutating func u16() throws -> UInt16 {
        let b = Array(try take(2))
        return UInt16(b[0]) | UInt16(b[1]) << 8
    }

    mutating func u32() throws -> UInt32 {
        let b = Array(try take(4))
        var v: UInt32 = 0
        for i in (0..<4).reversed() { v = v << 8 | UInt32(b[i]) }
        return v
    }

    /// §1: a codec MUST decode and encode the full unsigned 64-bit range. Swift's `UInt64` carries
    /// it exactly, so the JavaScript `BigInt` caveat has no Swift counterpart.
    mutating func u64() throws -> UInt64 {
        let b = Array(try take(8))
        var v: UInt64 = 0
        for i in (0..<8).reversed() { v = v << 8 | UInt64(b[i]) }
        return v
    }

    mutating func i32() throws -> Int32 { Int32(bitPattern: try u32()) }
    mutating func i64() throws -> Int64 { Int64(bitPattern: try u64()) }

    mutating func opaque16() throws -> [UInt8] { Array(try take(16)) }

    /// §1: "Reserved fields and inactive fixed-width alternatives are encoded as zero and rejected
    /// when nonzero."
    mutating func reserved(_ count: Int, _ what: String) throws {
        let slice = try take(count)
        guard slice.allSatisfy({ $0 == 0 }) else {
            throw WireFault.reservedBits("\(subject): \(what)")
        }
    }

    mutating func rest() -> [UInt8] {
        let out = Array(bytes[index...])
        index = bytes.count
        return out
    }

    /// The ResultEnvelope rule of §10 and every fixed-size body: a short payload is `truncated`, a
    /// long one is `trailingBytes`.
    func requireExhausted(_ what: String) throws {
        guard remaining == 0 else {
            throw WireFault.trailingBytes("\(subject): \(remaining) byte(s) after \(what)")
        }
    }
}

/// Little-endian writer. Every operation is total — no width conversion here can trap, because a
/// value that will not fit its field is refused by the `narrow*` helpers before it reaches the
/// writer, and `zeros` treats a negative count as none rather than as a precondition failure.
struct ByteWriter {
    private(set) var bytes: [UInt8] = []

    mutating func u8(_ v: UInt8) { bytes.append(v) }
    mutating func u16(_ v: UInt16) { bytes.append(contentsOf: [UInt8(v & 0xFF), UInt8(v >> 8)]) }
    mutating func u32(_ v: UInt32) {
        for i in 0..<4 { bytes.append(UInt8(truncatingIfNeeded: v >> (8 * i))) }
    }
    mutating func u64(_ v: UInt64) {
        for i in 0..<8 { bytes.append(UInt8(truncatingIfNeeded: v >> (8 * i))) }
    }
    mutating func i32(_ v: Int32) { u32(UInt32(bitPattern: v)) }
    mutating func i64(_ v: Int64) { u64(UInt64(bitPattern: v)) }
    mutating func raw(_ v: [UInt8]) { bytes.append(contentsOf: v) }
    mutating func zeros(_ n: Int) {
        guard n > 0 else { return }
        bytes.append(contentsOf: [UInt8](repeating: 0, count: n))
    }
}

// §1 forbids emitting a structure that cannot be framed, and this codec ships inside an app: a
// value too wide for its field is a caller error to report, never a process-fatal narrowing trap.

func narrowU16(_ value: Int, _ what: String) throws -> UInt16 {
    guard value >= 0, value <= Int(UInt16.max) else {
        throw WireFault.payloadLength("\(what): \(value) does not fit a u16")
    }
    return UInt16(value)
}

func narrowU8(_ value: Int, _ what: String) throws -> UInt8 {
    guard value >= 0, value <= Int(UInt8.max) else {
        throw WireFault.payloadLength("\(what): \(value) does not fit a u8")
    }
    return UInt8(value)
}

/// A count that must not exceed a contract limit, reported as a frame-bounds error.
func requireAtMost(_ value: Int, _ limit: Int, _ what: String) throws {
    guard value <= limit else {
        throw WireFault.frameBounds("\(what): \(value) exceeds the limit of \(limit)")
    }
}

/// CRC-32/IEEE: reflected polynomial `0xEDB88320`, initial and final XOR `0xFFFFFFFF` (§1).
/// It detects accidental corruption; it is not identity, authentication, or an idempotency proof.
public enum CRC32IEEE {
    private static let table: [UInt32] = {
        (0..<256).map { i -> UInt32 in
            var c = UInt32(i)
            for _ in 0..<8 { c = (c & 1) != 0 ? 0xEDB8_8320 ^ (c >> 1) : c >> 1 }
            return c
        }
    }()

    public static func checksum<C: Collection>(_ bytes: C) -> UInt32 where C.Element == UInt8 {
        var crc: UInt32 = 0xFFFF_FFFF
        for b in bytes { crc = table[Int((crc ^ UInt32(b)) & 0xFF)] ^ (crc >> 8) }
        return crc ^ 0xFFFF_FFFF
    }
}

/// §2.2's text rule, shared by metadata text fields and the device-config name: shortest-form valid
/// UTF-8 with no NUL, C0/C1 control, surrogate, or noncharacter scalar. Accepted bytes are
/// canonical as-is — this never normalizes, trims, or case-folds.
enum WireText {
    static func validate(_ bytes: [UInt8], subject: String) throws -> String {
        guard let text = String(bytes: bytes, encoding: .utf8) else {
            throw WireFault.noncanonicalMetadata("\(subject): not valid UTF-8")
        }
        // `String(bytes:encoding:.utf8)` already rejects overlong forms, lone surrogates and
        // truncated sequences; what remains is the scalar blacklist.
        for scalar in text.unicodeScalars {
            let v = scalar.value
            if v == 0 || v < 0x20 || (v >= 0x7F && v <= 0x9F) {
                throw WireFault.noncanonicalMetadata("\(subject): control scalar U+\(String(v, radix: 16))")
            }
            if v >= 0xFDD0 && v <= 0xFDEF {
                throw WireFault.noncanonicalMetadata("\(subject): noncharacter U+\(String(v, radix: 16))")
            }
            if (v & 0xFFFF) == 0xFFFE || (v & 0xFFFF) == 0xFFFF {
                throw WireFault.noncanonicalMetadata("\(subject): noncharacter U+\(String(v, radix: 16))")
            }
        }
        return text
    }
}
