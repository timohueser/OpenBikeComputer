import Foundation
import Testing
@testable import OBCProtocolV4

@Suite("TransferClient")
struct TransferClientTests {
    @Test("A mid-upload disconnect uses STATUS and restarts the uncommitted replacement")
    func midTransferDisconnectReconciles() async throws {
        let payload = Data("protocol v4 replacement".utf8)
        let link = DisconnectingLink(payload: payload)
        let client = TransferClient(link: link, firstRequestID: 0x2A00)

        let result = try await client.put(
            payload, objectID: ObjectID(rawValue: 9), expectedRevision: Revision(rawValue: 3),
            kind: .route, displayName: "Alpine Loop")

        #expect(result.objectID == ObjectID(rawValue: 9))
        #expect(result.revision == Revision(rawValue: 4))
        #expect(result.payloadCRC32 == CRC32.checksum(payload))
        let trace = await link.trace
        #expect(trace.opcodes == [.list, .put, .list, .status, .put])
        #expect(trace.streamRecords == 5)
        #expect(trace.streamOffsets == [0, 8, 0, 8, 16])
        #expect(trace.restores == 1)
    }
}

private actor DisconnectingLink: TransferLink {
    nonisolated let maximumStreamPayload = 8

    struct Trace: Sendable {
        var opcodes: [Opcode] = []
        var streamRecords = 0
        var streamOffsets: [UInt64] = []
        var restores = 0
    }

    private let payload: Data
    private let storeID = Data([0x8f, 0x2c, 0x41, 0xd9, 0x6b, 0x07, 0x4e, 0xa3,
                                0xb1, 0x55, 0x9c, 0x20, 0x7d, 0xe8, 0x34, 0x66])
    private var pending: ControlFrame?
    private var responseWaiter: CheckedContinuation<Data, Error>?
    private var readyResponse: Data?
    private var firstPutBroke = false
    private var putAttempts = 0
    private var putRecords = 0
    private var state = Trace()

    init(payload: Data) { self.payload = payload }
    var trace: Trace { state }

    func sendControlRecord(_ record: Data) async throws {
        let frame = try ControlFrame(decoding: record, direction: .request)
        pending = frame
        state.opcodes.append(frame.opcode)
        if frame.opcode == .put {
            putAttempts += 1
            putRecords = 0
        }
    }

    func receiveControlRecord() async throws -> Data {
        guard let frame = pending else { throw TransferClientError.unexpectedResponse }
        pending = nil
        switch frame.opcode {
        case .list:
            return response(opcode: .list, requestID: frame.requestID, payload: listPayload())
        case .status:
            var body = Data([StatusState.superseded.rawValue, 0, 0, 0])
            body.appendLE(UInt64(3))
            body.appendLE(UInt64(12))
            body.appendLE(UInt32(0x1234_5678))
            return response(opcode: .status, requestID: frame.requestID, payload: body)
        case .put:
            if putAttempts == 1, firstPutBroke { throw TransferLinkLost() }
            if let readyResponse {
                self.readyResponse = nil
                return readyResponse
            }
            return try await withCheckedThrowingContinuation { responseWaiter = $0 }
        default:
            throw TransferClientError.unexpectedResponse
        }
    }

    func sendStreamRecord(_ record: Data) async throws {
        let stream = try StreamRecord(decoding: record)
        state.streamRecords += 1
        state.streamOffsets.append(stream.offset)
        putRecords += 1
        if putAttempts == 1, putRecords == 2 {
            firstPutBroke = true
            responseWaiter?.resume(throwing: TransferLinkLost())
            responseWaiter = nil
            throw TransferLinkLost()
        }
        if putAttempts == 2, stream.offset + UInt64(stream.payload.count) == UInt64(payload.count) {
            var body = Data()
            body.appendLE(UInt64(9))
            body.appendLE(UInt64(4))
            body.appendLE(UInt64(payload.count))
            body.appendLE(CRC32.checksum(payload))
            body.appendLE(UInt32(0))
            let result = response(opcode: .put, requestID: stream.requestID, payload: body)
            if let responseWaiter {
                self.responseWaiter = nil
                responseWaiter.resume(returning: result)
            } else {
                readyResponse = result
            }
        }
    }

    func receiveStreamRecord() async throws -> Data { throw TransferLinkLost() }
    func cancelStreamReceive() async {}
    func restore() async throws { state.restores += 1 }

    private func listPayload() -> Data {
        var body = storeID
        body.appendLE(UInt64(7))
        return body
    }

    private func response(opcode: Opcode, requestID: RequestID, payload: Data) -> Data {
        ControlFrame(
            opcode: opcode, flags: ControlFrame.responseFlag,
            requestID: requestID, payload: payload).encode()
    }
}

private extension Data {
    mutating func appendLE<T: FixedWidthInteger>(_ value: T) {
        for shift in stride(from: 0, to: T.bitWidth, by: 8) {
            append(UInt8(truncatingIfNeeded: value >> T(shift)))
        }
    }
}
