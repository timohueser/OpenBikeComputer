import Foundation
import Testing
@testable import OBCProtocolV4

@Suite("Archive receipt exchange")
struct ArchiveReceiptTests {
    private func send(_ client: TransferClient, store: StoreID) async throws -> ArchiveRideResult {
        try await client.archiveRide(storeID: store, objectID: ObjectID(rawValue: 0x1234_5678_9ABC_DEF0),
                                     revision: Revision(rawValue: 0xFEDC_BA98_7654_3210),
                                     payloadLength: 0x1_0000_0001, payloadCRC32: 0)
    }

    @Test(arguments: [UInt32(0), UInt32(1_800_000_000)])
    func exactTupleAndCorrelatedReply(timestamp: UInt32) async throws {
        let link = ReceiptLink(steps: [.reply(timestamp)], lateReply: true)
        let result = try await send(TransferClient(link: link), store: link.store)
        #expect(result.timestamp == timestamp)
        #expect(result.commitSequence == 17)
        let frames = await link.frames
        #expect(frames.map(\.opcode) == [.list, .archiveRide])
        var expected = link.store.bytes
        expected.receiptLE(UInt64(0x1234_5678_9ABC_DEF0))
        expected.receiptLE(UInt64(0xFEDC_BA98_7654_3210))
        expected.receiptLE(UInt64(0x1_0000_0001))
        expected.receiptLE(UInt32(0))
        #expect(frames.last?.payload == expected)
    }

    @Test func lostReplyRestoresAndRepeatsReceiptWithFreshRequest() async throws {
        let link = ReceiptLink(steps: [.drop, .reply(100)])
        let result = try await send(TransferClient(link: link), store: link.store)
        #expect(result.timestamp == 100)
        let frames = await link.frames
        #expect(frames.map(\.opcode) == [.list, .archiveRide, .list, .list, .archiveRide])
        let receipts = frames.filter { $0.opcode == .archiveRide }
        #expect(receipts[0].payload == receipts[1].payload)
        #expect(receipts[0].requestID != receipts[1].requestID)
        #expect(await link.restores == 1)
    }

    @Test func replacementCardStopsRetry() async throws {
        let link = ReceiptLink(steps: [.drop], replaceOnRestore: true)
        await #expect(throws: TransferClientError.storeChanged(previous: link.store, current: link.replacement)) {
            try await send(TransferClient(link: link), store: link.store)
        }
        #expect(await link.frames.filter { $0.opcode == .archiveRide }.count == 1)
    }

    @Test func missingReplyTimesOutAndReleasesLaneForRetry() async throws {
        let link = ReceiptLink(steps: [.park, .reply(0)])
        let client = TransferClient(link: link, archiveResponseTimeout: .milliseconds(20))
        await #expect(throws: TransferClientError.responseTimedOut) { try await send(client, store: link.store) }
        #expect(await link.controlCancels == 1)
        #expect(try await send(client, store: link.store).timestamp == 0)
        #expect(await link.restores == 0)
    }

    @Test func cancelledReceiveAndQueuedReceiptDoNotWriteAgain() async throws {
        let link = ReceiptLink(steps: [.park, .reply(0)])
        let client = TransferClient(link: link)
        let first = Task { try await send(client, store: link.store) }
        await link.waitUntilParked()
        let queued = Task { try await send(client, store: link.store) }
        await Task.yield()
        queued.cancel()
        first.cancel()
        await #expect(throws: CancellationError.self) { try await first.value }
        await #expect(throws: CancellationError.self) { try await queued.value }
        #expect(await link.frames.filter { $0.opcode == .archiveRide }.count == 1)
        #expect(try await send(client, store: link.store).timestamp == 0)
    }

    @Test(arguments: [RemoteErrorCode.busy, .unsupported, .invalidRequest, .readOnly, .mediaIO])
    func remoteFailureIsNotConfirmationOrAutomaticRetry(code: RemoteErrorCode) async throws {
        let link = ReceiptLink(steps: [.reject(code)])
        do {
            _ = try await send(TransferClient(link: link), store: link.store)
            Issue.record("remote rejection became success")
        } catch WireError.remote(let error) {
            #expect(error.code == code)
        }
        #expect(await link.frames.filter { $0.opcode == .archiveRide }.count == 1)
        #expect(await link.restores == 0)
    }
}

private actor ReceiptLink: TransferLink {
    enum Step: Sendable { case reply(UInt32), drop, park, reject(RemoteErrorCode) }
    nonisolated let maximumStreamPayload = 512
    nonisolated let store = try! StoreID(bytes: Data(repeating: 0xA5, count: 16))
    nonisolated let replacement = try! StoreID(bytes: Data(repeating: 0xB6, count: 16))
    var frames: [ControlFrame] = []
    var restores = 0
    var controlCancels = 0
    private var steps: [Step]
    private var pending: ControlFrame?
    private var lateReply: Bool
    private let replaceOnRestore: Bool
    private var waiter: CheckedContinuation<Data, Error>?
    private var parkObserver: CheckedContinuation<Void, Never>?

    init(steps: [Step], lateReply: Bool = false, replaceOnRestore: Bool = false) {
        self.steps = steps
        self.lateReply = lateReply
        self.replaceOnRestore = replaceOnRestore
    }
    func sendControlRecord(_ record: Data) async throws {
        let frame = try ControlFrame(decoding: record, direction: .request)
        frames.append(frame)
        pending = frame
    }
    func receiveControlRecord() async throws -> Data {
        try Task.checkCancellation()
        guard let frame = pending else { throw TransferClientError.unexpectedResponse }
        if frame.opcode == .archiveRide && lateReply {
            lateReply = false
            return response(frame, timestamp: 999, requestID: RequestID(rawValue: 999)!)
        }
        pending = nil
        if frame.opcode == .list {
            var body = replaceOnRestore && restores > 0 ? replacement.bytes : store.bytes
            body.receiptLE(UInt64(17))
            return ControlFrame(opcode: .list, flags: ControlFrame.responseFlag,
                                requestID: frame.requestID, payload: body).encode()
        }
        guard frame.opcode == .archiveRide, !steps.isEmpty else { throw TransferClientError.unexpectedResponse }
        switch steps.removeFirst() {
        case .reply(let timestamp): return response(frame, timestamp: timestamp)
        case .drop: throw TransferLinkLost()
        case .park:
            return try await withCheckedThrowingContinuation {
                waiter = $0
                parkObserver?.resume()
                parkObserver = nil
            }
        case .reject(let code):
            var body = Data()
            body.receiptLE(code.rawValue)
            body.receiptLE(UInt16(0))
            body.receiptLE(UInt32(0))
            body.receiptLE(UInt64(0))
            return ControlFrame(opcode: .archiveRide, flags: ControlFrame.responseFlag | ControlFrame.errorFlag,
                                requestID: frame.requestID, payload: body).encode()
        }
    }
    private func response(_ frame: ControlFrame, timestamp: UInt32, requestID: RequestID? = nil) -> Data {
        var body = Data()
        body.receiptLE(UInt64(17))
        body.receiptLE(timestamp)
        body.receiptLE(UInt32(0))
        return ControlFrame(opcode: .archiveRide, flags: ControlFrame.responseFlag,
                            requestID: requestID ?? frame.requestID, payload: body).encode()
    }
    func waitUntilParked() async {
        if waiter != nil { return }
        await withCheckedContinuation { parkObserver = $0 }
    }
    func cancelControlReceive() async {
        controlCancels += 1
        waiter?.resume(throwing: CancellationError())
        waiter = nil
    }
    func restore() async throws { restores += 1 }
    func cancelStreamReceive() async {}
    func sendStreamRecord(_ record: Data) async throws { throw TransferClientError.unexpectedStream }
    func receiveStreamRecord() async throws -> Data { throw TransferClientError.unexpectedStream }
}

private extension Data {
    mutating func receiptLE<T: FixedWidthInteger>(_ value: T) {
        var little = value.littleEndian
        Swift.withUnsafeBytes(of: &little) { append(contentsOf: $0) }
    }
}
