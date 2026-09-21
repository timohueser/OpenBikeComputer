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

    @Test("Store identity stops after the first LIST page")
    func storeIdentityDoesNotWalkTheCatalog() async throws {
        let link = PagedIdentityLink()
        let client = TransferClient(link: link, firstRequestID: 0x4100)

        let storeID = try await client.storeID()
        let requests = await link.listRequests

        #expect(storeID == link.storeID)
        #expect(requests == 1)
    }

    @Test("A fresh create starts PUT without a catalog preflight")
    func freshCreateDoesNotWalkTheCatalog() async throws {
        let payload = Data("new route".utf8)
        let link = FreshCreateLink(payload: payload)
        let client = TransferClient(link: link, firstRequestID: 0x4200)

        let result = try await client.put(payload, kind: .route, displayName: "New Route")
        let opcodes = await link.opcodes

        #expect(result.objectID == ObjectID(rawValue: 42))
        #expect(opcodes == [.list, .put])
    }

    @Test(
        "A stream lane that dies before the first record unwinds the announced PUT",
        .timeLimit(.minutes(1)))
    func streamDropWithNoAnswerReconciles() async throws {
        let payload = Data("protocol v4 replacement".utf8)
        let link = AbandonedAnswerLink(payload: payload)
        let client = TransferClient(link: link, firstRequestID: 0x5300)

        let result = try await client.put(
            payload, objectID: ObjectID(rawValue: 9), expectedRevision: Revision(rawValue: 3),
            kind: .route, displayName: "Alpine Loop")

        #expect(result.objectID == ObjectID(rawValue: 9))
        #expect(result.revision == Revision(rawValue: 4))
        let trace = await link.trace
        #expect(trace.opcodes == [.list, .put, .list, .status, .put])
        #expect(trace.streamOffsets == [0, 8, 16])
        #expect(trace.controlCancels == 1)
        #expect(trace.restores == 1)
    }

    @Test(
        "The peer's answer to the abandoned request does not fail the next one",
        .timeLimit(.minutes(1)))
    func lateAnswerToAnAbandonedRequestIsSkipped() async throws {
        let payload = Data("protocol v4 replacement".utf8)
        let link = AbandonedAnswerLink(payload: payload, answersTheAbandonedRequest: true)
        let client = TransferClient(link: link, firstRequestID: 0x6100)

        let result = try await client.put(
            payload, objectID: ObjectID(rawValue: 9), expectedRevision: Revision(rawValue: 3),
            kind: .route, displayName: "Alpine Loop")

        #expect(result.objectID == ObjectID(rawValue: 9))
        #expect(result.revision == Revision(rawValue: 4))
        let trace = await link.trace
        #expect(trace.opcodes == [.list, .put, .list, .status, .put])
        #expect(trace.lateAnswers == 1)
    }
}

/// The wedge shape of a lost CoC: the announce reaches the peer, not one stream byte follows, and
/// the peer answers nothing. Only the client asking for it releases the parked PUT answer.
private actor AbandonedAnswerLink: TransferLink {
    nonisolated let maximumStreamPayload = 8

    struct Trace: Sendable {
        var opcodes: [Opcode] = []
        var streamOffsets: [UInt64] = []
        var controlCancels = 0
        var lateAnswers = 0
        var restores = 0
    }

    private let answersTheAbandonedRequest: Bool
    private var livePut: RequestID?
    private var cancelledRequest: RequestID?
    private var abandoned: RequestID?
    private let payload: Data
    private let storeID = Data([0x14, 0x7b, 0xc0, 0x39, 0x8a, 0x62, 0x4d, 0x11,
                                0x9e, 0x05, 0x33, 0xf7, 0x2a, 0xb8, 0x61, 0x4c])
    private var pending: ControlFrame?
    private var answerWaiter: CheckedContinuation<Data, Error>?
    private var readyAnswer: Data?
    private var putAttempts = 0
    private var state = Trace()

    init(payload: Data, answersTheAbandonedRequest: Bool = false) {
        self.payload = payload
        self.answersTheAbandonedRequest = answersTheAbandonedRequest
    }
    var trace: Trace { state }

    func sendControlRecord(_ record: Data) async throws {
        let frame = try ControlFrame(decoding: record, direction: .request)
        pending = frame
        state.opcodes.append(frame.opcode)
        if frame.opcode == .put {
            putAttempts += 1
            livePut = frame.requestID
        }
    }

    func receiveControlRecord() async throws -> Data {
        // A cancel can reach the link before the receive it names ever runs. The receive then
        // resolves as cancelled instead of parking for an answer nobody waits for.
        if let cancelledRequest, let pending, cancelledRequest == pending.requestID {
            self.cancelledRequest = nil
            self.pending = nil
            throw CancellationError()
        }
        // The peer answers every request exactly once, whenever it terminates — including the one
        // the client walked away from. That answer reaches the lane ahead of the live request's.
        if let abandoned {
            self.abandoned = nil
            state.lateAnswers += 1
            var body = Data()
            body.appendLE(RemoteErrorCode.cancelled.rawValue)
            body.appendLE(UInt16(0))
            body.appendLE(UInt64(0))
            body.appendLE(UInt32(0))
            return ControlFrame(
                opcode: .put, flags: ControlFrame.responseFlag | ControlFrame.errorFlag,
                requestID: abandoned, payload: body).encode()
        }
        guard let frame = pending else { throw TransferClientError.unexpectedResponse }
        pending = nil
        switch frame.opcode {
        case .list:
            var body = storeID
            body.appendLE(UInt64(7))
            return response(opcode: .list, requestID: frame.requestID, payload: body)
        case .status:
            var body = Data([StatusState.superseded.rawValue, 0, 0, 0])
            body.appendLE(UInt64(3))
            body.appendLE(UInt64(12))
            body.appendLE(UInt32(0x1234_5678))
            return response(opcode: .status, requestID: frame.requestID, payload: body)
        case .put:
            if let readyAnswer {
                self.readyAnswer = nil
                return readyAnswer
            }
            return try await withCheckedThrowingContinuation { answerWaiter = $0 }
        default:
            throw TransferClientError.unexpectedResponse
        }
    }

    func sendStreamRecord(_ record: Data) async throws {
        let stream = try StreamRecord(decoding: record)
        guard putAttempts > 1 else { throw TransferLinkLost() }
        state.streamOffsets.append(stream.offset)
        guard stream.offset + UInt64(stream.payload.count) == UInt64(payload.count) else { return }
        var body = Data()
        body.appendLE(UInt64(9))
        body.appendLE(UInt64(4))
        body.appendLE(UInt64(payload.count))
        body.appendLE(CRC32.checksum(payload))
        body.appendLE(UInt32(0))
        let answer = response(opcode: .put, requestID: stream.requestID, payload: body)
        if let answerWaiter {
            self.answerWaiter = nil
            answerWaiter.resume(returning: answer)
        } else {
            readyAnswer = answer
        }
    }

    func receiveStreamRecord() async throws -> Data { throw TransferClientError.unexpectedStream }

    func cancelControlReceive() async {
        state.controlCancels += 1
        if answersTheAbandonedRequest { abandoned = livePut }
        // A parked receive takes the cancellation; one that has not run yet is named instead, so
        // either arrival order resolves it exactly once.
        if let answerWaiter {
            self.answerWaiter = nil
            answerWaiter.resume(throwing: CancellationError())
        } else {
            cancelledRequest = livePut
        }
    }

    func cancelStreamReceive() async {}
    func restore() async throws { state.restores += 1 }

    private func response(opcode: Opcode, requestID: RequestID, payload: Data) -> Data {
        ControlFrame(
            opcode: opcode, flags: ControlFrame.responseFlag,
            requestID: requestID, payload: payload).encode()
    }
}

private actor PagedIdentityLink: TransferLink {
    nonisolated let maximumStreamPayload = 8
    nonisolated let storeID = try! StoreID(bytes: Data(repeating: 0xA5, count: 16))

    private var pending: ControlFrame?
    private(set) var listRequests = 0

    func sendControlRecord(_ record: Data) async throws {
        let frame = try ControlFrame(decoding: record, direction: .request)
        guard frame.opcode == .list else { throw TransferClientError.unexpectedResponse }
        pending = frame
        listRequests += 1
    }

    func receiveControlRecord() async throws -> Data {
        guard let frame = pending else { throw TransferClientError.unexpectedResponse }
        pending = nil
        var payload = Data(repeating: 0xA5, count: 16)
        payload.appendLE(UInt64(7))
        payload.appendLE(UInt64(9))
        payload.appendLE(UInt64(1))
        payload.appendLE(UInt64(512))
        payload.appendLE(UInt32(0x1234_5678))
        payload.appendLE(ObjectKind.route.rawValue)
        payload.appendLE(UInt16(0))
        payload.append(UInt8(4))
        payload.append(contentsOf: [0, 0, 0])
        payload.append(Data("Test".utf8))
        payload.append(Data(repeating: 0, count: 44))
        payload.appendLE(UInt32(0))
        return ControlFrame(
            opcode: .list,
            flags: ControlFrame.responseFlag | ControlFrame.moreFlag,
            requestID: frame.requestID,
            payload: payload
        ).encode()
    }

    func sendStreamRecord(_ record: Data) async throws { throw TransferClientError.unexpectedStream }
    func receiveStreamRecord() async throws -> Data { throw TransferClientError.unexpectedStream }
    func cancelControlReceive() async {}
    func cancelStreamReceive() async {}
    func restore() async throws {}
}

private actor FreshCreateLink: TransferLink {
    nonisolated let maximumStreamPayload = 8

    private let payload: Data
    private var pending: ControlFrame?
    private var responseWaiter: CheckedContinuation<Data, Error>?
    private var readyResponse: Data?
    private(set) var opcodes: [Opcode] = []

    init(payload: Data) { self.payload = payload }

    func sendControlRecord(_ record: Data) async throws {
        let frame = try ControlFrame(decoding: record, direction: .request)
        pending = frame
        opcodes.append(frame.opcode)
    }

    func receiveControlRecord() async throws -> Data {
        guard let frame = pending else { throw TransferClientError.unexpectedResponse }
        pending = nil
        switch frame.opcode {
        case .list:
            var body = Data(repeating: 0xB6, count: 16)
            body.appendLE(UInt64(11))
            body.appendLE(UInt64(9))
            body.appendLE(UInt64(1))
            body.appendLE(UInt64(512))
            body.appendLE(UInt32(0x1234_5678))
            body.appendLE(ObjectKind.route.rawValue)
            body.appendLE(UInt16(0))
            body.append(UInt8(4))
            body.append(contentsOf: [0, 0, 0])
            body.append(Data("Test".utf8))
            body.append(Data(repeating: 0, count: 44))
            body.appendLE(UInt32(0))
            return ControlFrame(
                opcode: .list,
                flags: ControlFrame.responseFlag | ControlFrame.moreFlag,
                requestID: frame.requestID,
                payload: body
            ).encode()
        case .put:
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
        guard stream.offset + UInt64(stream.payload.count) == UInt64(payload.count) else { return }
        var body = Data()
        body.appendLE(UInt64(42))
        body.appendLE(UInt64(1))
        body.appendLE(UInt64(payload.count))
        body.appendLE(CRC32.checksum(payload))
        body.appendLE(UInt32(0))
        let response = ControlFrame(
            opcode: .put, flags: ControlFrame.responseFlag,
            requestID: stream.requestID, payload: body
        ).encode()
        if let responseWaiter {
            self.responseWaiter = nil
            responseWaiter.resume(returning: response)
        } else {
            readyResponse = response
        }
    }

    func receiveStreamRecord() async throws -> Data { throw TransferClientError.unexpectedStream }
    func cancelControlReceive() async {}
    func cancelStreamReceive() async {}
    func restore() async throws {}
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
            // A GATT drop fails every parked control waiter from the link side, the way
            // `BLETransport.failAllPending` does. `AbandonedAnswerLink` carries the other shape:
            // the CoC dying under a live GATT link.
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

    func cancelControlReceive() async {
        responseWaiter?.resume(throwing: CancellationError())
        responseWaiter = nil
    }

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

@Suite("Pinned archive GET")
struct ArchiveGetTests {
    @Test(arguments: ArchiveGetLink.Fault.allCases)
    fileprivate func checksStoreRevisionAndVerifiedContent(_ fault: ArchiveGetLink.Fault) async throws {
        let link = ArchiveGetLink(fault: fault)
        let client = TransferClient(link: link)
        do {
            let downloaded = try await client.get(objectID: ObjectID(rawValue: 42),
                                                  revision: Revision(rawValue: 3), expectedStoreID: link.storeID)
            #expect(fault == .none)
            #expect(downloaded.payload == link.payload)
            #expect(downloaded.result.revision.rawValue == 3)
            #expect(downloaded.result.payloadCRC32 == CRC32.checksum(link.payload))
        } catch let error as TransferClientError {
            switch fault {
            case .none: Issue.record("Unexpected GET failure: \(error)")
            case .revision: #expect(error == .unexpectedResponse)
            case .checksum: #expect(error == .checksumMismatch)
            case .store: #expect(error == .storeChanged(previous: link.storeID, current: link.otherStoreID))
            }
        }
    }
}

private actor ArchiveGetLink: TransferLink {
    enum Fault: CaseIterable, Sendable { case none, revision, checksum, store }
    nonisolated let maximumStreamPayload = 512
    nonisolated let storeID = try! StoreID(bytes: Data(repeating: 0xA5, count: 16))
    nonisolated let otherStoreID = try! StoreID(bytes: Data(repeating: 0xB6, count: 16))
    nonisolated let payload = Data("verified ride bytes".utf8)
    let fault: Fault
    var pending: ControlFrame?
    var getID: RequestID?
    var sentStream = false
    var listCount = 0
    var streamWaiter: CheckedContinuation<Data, Error>?
    var streamCancelled = false

    init(fault: Fault) { self.fault = fault }

    func sendControlRecord(_ record: Data) async throws {
        let frame = try ControlFrame(decoding: record, direction: .request)
        pending = frame
        if frame.opcode == .get { getID = frame.requestID }
    }

    func receiveControlRecord() async throws -> Data {
        guard let frame = pending else { throw TransferClientError.unexpectedResponse }
        pending = nil
        var body = Data()
        switch frame.opcode {
        case .list:
            listCount += 1
            body.append(fault == .store && listCount == 3 ? otherStoreID.bytes : storeID.bytes)
            body.appendLE(UInt64(1))
        case .get:
            body.appendLE(UInt64(fault == .revision ? 4 : 3))
            body.appendLE(UInt64(payload.count))
            body.appendLE(CRC32.checksum(payload) ^ (fault == .checksum ? 1 : 0))
            body.appendLE(UInt32(0))
        default: throw TransferClientError.unexpectedResponse
        }
        return ControlFrame(opcode: frame.opcode, flags: ControlFrame.responseFlag,
                            requestID: frame.requestID, payload: body).encode()
    }

    func receiveStreamRecord() async throws -> Data {
        if streamCancelled || Task.isCancelled { throw CancellationError() }
        if !sentStream, let getID {
            sentStream = true
            return try StreamRecord(requestID: getID, offset: 0, payload: payload).encode()
        }
        return try await withCheckedThrowingContinuation { streamWaiter = $0 }
    }

    func cancelStreamReceive() async {
        streamCancelled = true
        streamWaiter?.resume(throwing: CancellationError())
        streamWaiter = nil
    }
    func sendStreamRecord(_ record: Data) async throws { throw TransferClientError.unexpectedStream }
    func cancelControlReceive() async {}
    func restore() async throws { throw TransferLinkLost() }
}
