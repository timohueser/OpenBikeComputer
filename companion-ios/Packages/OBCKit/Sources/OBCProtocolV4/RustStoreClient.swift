import Foundation
import OBCHost

/// The actor calls this synchronously. Every returned buffer is copied before an await.
final class RustStoreClient {
    private let handle: OpaquePointer

    init(payloadCeiling: Int) throws {
        guard payloadCeiling > 0, payloadCeiling <= Int(UInt16.max),
              let handle = obc_client_open(Int(UInt16.max) + FlatStoreV4.controlHeaderLength,
                                           payloadCeiling + FlatStoreV4.streamHeaderLength)
        else { throw TransferClientError.invalidLinkCeiling(payloadCeiling) }
        self.handle = handle
    }
    deinit { obc_client_close(handle) }

    struct Result: Sendable {
        let opcode: UInt32
        let objectID: ObjectID
        let revision: Revision
        let length: UInt64
        let crc: UInt32
        let sequence: UInt64
        let timestamp: UInt32
        let state: UInt32
        let flag: Bool
        let store: StoreID
        let entries: [CatalogEntry]
    }
    enum Action {
        case send(UInt64, UInt32, Data)
        case source(UInt64, UInt64, Int)
        case sink(UInt64, UInt64, Data)
        case resetSink
        case progress(UInt64, UInt64)
        case resetChannels
        case restore
        case complete(Swift.Result<Result, Error>)
        case query(UInt64, Swift.Result<Result, Error>)
    }

    func start(_ input: ObcClientRequest, name: String = "", query: Bool = false,
               timeout: UInt64? = nil, now: UInt64) throws -> UInt64 {
        var input = input
        let nameBytes = Data(name.utf8)
        return try nameBytes.withUnsafeBytes { bytes in
            input.name = bytes.baseAddress?.assumingMemoryBound(to: UInt8.self)
            input.name_len = bytes.count
            let result = obc_client_start(handle, &input, query, timeout ?? 0, timeout != nil, now)
            try Self.check(result)
            return result.context
        }
    }

    func event(_ kind: UInt32, token: UInt64 = 0, offset: UInt64 = 0,
               bytes: Data = Data(), length: Int? = nil, now: UInt64) throws {
        try bytes.withUnsafeBytes { data in
            try Self.check(obc_client_event(handle, kind, token, offset,
                data.baseAddress?.assumingMemoryBound(to: UInt8.self), length ?? data.count, now))
        }
    }

    func cancelQuery(_ id: UInt64) { _ = obc_client_cancel_query(handle, id) }

    var deadline: UInt64? {
        var value: UInt64 = 0
        return obc_client_deadline(handle, &value) ? value : nil
    }
    var reads: UInt32 { obc_client_reads(handle) }
    var transfer: RequestID? { RequestID(rawValue: obc_client_transfer(handle)) }

    static func checksum(_ bytes: Data) -> UInt32 {
        bytes.withUnsafeBytes { obc_client_crc32($0.baseAddress?.assumingMemoryBound(to: UInt8.self), $0.count) }
    }

    func next() -> Action? {
        let item = obc_client_next(handle)
        func data() -> Data {
            guard let bytes = item.bytes, item.length > 0 else { return Data() }
            return Data(bytes: bytes, count: item.length)
        }
        switch item.kind {
        case 1: return .send(item.token, item.channel, data())
        case 2: return .source(item.token, item.offset, item.length)
        case 3: return .sink(item.token, item.offset, data())
        case 4: return .resetSink
        case 5: return .progress(item.offset, item.total)
        case 6: return .resetChannels
        case 7: return .restore
        case 8: return .complete(Swift.Result { try Self.result(item.result) })
        case 9: return .query(item.token, Swift.Result { try Self.result(item.result) })
        default: return nil
        }
    }

    private static func check(_ result: ObcClientResult) throws {
        switch result.error {
        case 0: return
        case 4: throw TransferClientError.checksumMismatch
        case 5:
            guard let code = RemoteErrorCode(rawValue: result.remote_code) else {
                throw TransferClientError.unexpectedResponse
            }
            throw WireError.remote(RemoteErrorBody(code: code, detail: result.detail, context: result.context))
        case 6: throw TransferClientError.responseTimedOut
        case 7: throw CancellationError()
        case 9: throw TransferLinkLost()
        case 10: throw TransferClientError.storeChanged(previous: try store(result.previous), current: try store(result.store))
        case 11: throw TransferClientError.catalogChanged
        case 12, 13: throw TransferClientError.outcomeNotCommitted
        case 14: throw TransferClientError.requestIDExhausted
        default: throw TransferClientError.unexpectedResponse
        }
    }

    private static func store<T>(_ bytes: T) throws -> StoreID {
        var bytes = bytes
        return try withUnsafeBytes(of: &bytes) { try StoreID(bytes: Data($0)) }
    }

    private static func result(_ input: ObcClientResult) throws -> Result {
        try check(input)
        var entries: [CatalogEntry] = []
        if let pointer = input.entries {
            for entry in UnsafeBufferPointer(start: pointer, count: input.entry_count) {
                guard let kind = ObjectKind(rawValue: entry.kind), entry.name_len <= FlatStoreV4.maximumDisplayNameLength
                else { throw TransferClientError.unexpectedResponse }
                var name = entry.name
                let text = withUnsafeBytes(of: &name) { String(data: Data($0.prefix(entry.name_len)), encoding: .utf8) }
                guard let text else { throw TransferClientError.unexpectedResponse }
                entries.append(CatalogEntry(objectID: ObjectID(rawValue: entry.object_id),
                    revision: Revision(rawValue: entry.revision), payloadLength: entry.length,
                    payloadCRC32: entry.crc, kind: kind, flags: CatalogFlags(rawValue: entry.flags), displayName: text))
            }
        }
        return Result(opcode: input.opcode, objectID: ObjectID(rawValue: input.object_id),
            revision: Revision(rawValue: input.revision), length: input.length, crc: input.crc,
            sequence: input.sequence, timestamp: input.timestamp, state: input.state, flag: input.flag != 0,
            store: try store(input.store), entries: entries)
    }
}
