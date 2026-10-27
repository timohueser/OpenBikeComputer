import Foundation
@testable import OBCProtocolV4

// The fake links answer with the bytes a device sends. The Rust client owns the phone's codec, and
// firmware/obc-link pins both sides to specs/vectors/flat-store-v4/.

/// A response control record: the response flag, plus the error or more flag.
func responseRecord(
    _ opcode: Opcode, requestID: RequestID, payload: Data, error: Bool = false, more: Bool = false
) -> Data {
    var flags: UInt16 = 1
    if error { flags |= 1 << 1 }
    if more { flags |= 1 << 2 }
    var record = Data("OBC4".utf8)
    record.append(FlatStoreV4.wireMajor)
    record.append(opcode.rawValue)
    record.appendLE(flags)
    record.appendLE(UInt16(payload.count))
    record.appendLE(UInt16(0))
    record.appendLE(requestID.rawValue)
    return record + payload
}

func streamRecord(requestID: RequestID, offset: UInt64, payload: Data) -> Data {
    var record = Data()
    record.appendLE(requestID.rawValue)
    record.appendLE(offset)
    record.appendLE(UInt16(payload.count))
    record.appendLE(UInt16(0))
    return record + payload
}

extension Data {
    mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        Swift.withUnsafeBytes(of: &little) { append(contentsOf: $0) }
    }
}
