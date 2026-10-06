import Foundation
import OBCHost

/// The physical facts protocol v4 needs from BLE or USB. Implementations preserve record
/// boundaries, order a control write before stream records for that request, release a parked
/// receive on request, and restore a broken link. They do not interpret frames or retain
/// operation state.
public protocol TransferLink: Sendable {
    var maximumStreamPayload: Int { get }
    func sendControlRecord(_ record: Data) async throws
    func receiveControlRecord() async throws -> Data
    func sendStreamRecord(_ record: Data) async throws
    func receiveStreamRecord() async throws -> Data
    func cancelControlReceive() async
    func cancelStreamReceive() async
    func restore() async throws
}

/// The only error a physical link uses to request protocol-level reconciliation.
public struct TransferLinkLost: Error, Sendable {
    public init() {}
}

public enum TransferClientError: Error, Equatable, Sendable {
    case invalidLinkCeiling(Int)
    case unexpectedResponse
    case unexpectedStream
    case payloadTooLarge
    case lengthMismatch
    case checksumMismatch
    case requestIDExhausted
    case responseTimedOut
    case catalogChanged
    case storeChanged(previous: StoreID, current: StoreID)
    case outcomeNotCommitted
}

/// Rust owns request correlation, catalogue snapshots and mutation recovery. This actor owns
/// the FIFO, payload storage and the two physical receive tasks.
public actor TransferClient {
    private let link: any TransferLink
    private let archiveResponseTimeout: Duration
    private var core: RustStoreClient?
    private var busy = false
    private var operationWaiters: [CheckedContinuation<Void, Never>] = []
    private var completion: CheckedContinuation<RustStoreClient.Result, Error>?
    private var source = Data()
    private var sink = Data()
    private var progress: @Sendable (Int, Int) -> Void = { _, _ in }
    private var queryID: UInt64?
    private var generation: UInt64 = 0
    private var ioEpoch: UInt64 = 0
    private var controlReader: ReceiveTask?
    private var streamReader: ReceiveTask?
    private var controlReadSettled = false
    private var streamReadSettled = false
    private var writers: [UInt64: Task<Void, Never>] = [:]
    private var timer: Task<Void, Never>?
    private var draining = false
    private var receiveFailure: Error?
    private var deferredAction: RustStoreClient.Action?

    public init(link: any TransferLink, archiveResponseTimeout: Duration = .seconds(10)) {
        self.link = link
        self.archiveResponseTimeout = archiveResponseTimeout
    }

    public func list(kind: ObjectKind? = nil) async throws -> [CatalogEntry] {
        try await catalog(kind: kind).entries
    }

    public func catalog(kind: ObjectKind? = nil) async throws -> (storeID: StoreID, entries: [CatalogEntry]) {
        try await acquire()
        defer { release() }
        let result = try await execute(request(1, kind: kind))
        return (result.store, result.entries)
    }

    public func storeID() async throws -> StoreID {
        try await acquire()
        defer { release() }
        return try await identifyStore()
    }

    public func status(objectID: ObjectID, revision: Revision) async throws -> StatusResult {
        try await acquire()
        defer { release() }
        let value = try await execute(request(2, id: objectID, revision: revision))
        guard let state = StatusState(rawValue: UInt8(exactly: value.state) ?? 255) else {
            throw TransferClientError.unexpectedResponse
        }
        return StatusResult(state: state, headRevision: value.revision,
                            headPayloadLength: value.length, headPayloadCRC32: value.crc)
    }

    public func get(objectID: ObjectID, revision: Revision? = nil, expectedStoreID: StoreID? = nil,
                    progress: @escaping @Sendable (Int, Int) -> Void = { _, _ in }) async throws
        -> (result: GetResult, payload: Data) {
        try await acquire()
        defer { release() }
        if let expectedStoreID { _ = try await identifyStore(expected: expectedStoreID) }
        let value = try await execute(request(3, id: objectID, revision: revision, store: expectedStoreID),
                                      progress: progress)
        let payload = sink
        if let revision, revision != value.revision { throw TransferClientError.unexpectedResponse }
        if let expectedStoreID { _ = try await identifyStore(expected: expectedStoreID) }
        return (GetResult(revision: value.revision, payloadLength: value.length,
                          payloadCRC32: value.crc), payload)
    }

    public func put(_ payload: Data, objectID: ObjectID? = nil, expectedRevision: Revision? = nil,
                    kind: ObjectKind, displayName: String,
                    progress: @escaping @Sendable (Int, Int) -> Void = { _, _ in }) async throws -> PutResult {
        try await acquire()
        defer { release() }
        var input = request(4, id: objectID, revision: expectedRevision, kind: kind)
        input.length = UInt64(payload.count)
        input.crc = RustStoreClient.checksum(payload)
        let value = try await execute(input, name: displayName, source: payload, progress: progress)
        return PutResult(objectID: value.objectID, revision: value.revision,
                         payloadLength: value.length, payloadCRC32: value.crc)
    }

    public func remove(objectID: ObjectID, expectedRevision: Revision) async throws -> RemoveResult {
        try await acquire()
        defer { release() }
        let value = try await execute(request(5, id: objectID, revision: expectedRevision))
        return RemoveResult(commitSequence: value.flag ? value.sequence : nil)
    }

    public func cancel(transfer: RequestID) async throws -> CancelResult {
        try await acquire()
        defer { release() }
        var input = request(6)
        input.object_id = UInt64(transfer.rawValue)
        return try await execute(input).flag ? .cancelled : .noSuchTransfer
    }

    public func arm(packageObjectID: ObjectID, expectedRevision: Revision) async throws -> ArmResult {
        try await acquire()
        defer { release() }
        let value = try await execute(request(7, id: packageObjectID, revision: expectedRevision))
        return ArmResult(rollbackObjectID: value.objectID, commitSequence: value.sequence)
    }

    public func format(expectedStoreID: StoreID, replacementStoreID: StoreID) async throws -> FormatResult {
        try await acquire()
        defer { release() }
        var input = request(8, store: expectedStoreID)
        copy(replacementStoreID, to: &input.replacement)
        let value = try await execute(input)
        return FormatResult(storeID: value.store)
    }

    public func archiveRide(storeID: StoreID, objectID: ObjectID, revision: Revision,
                            payloadLength: UInt64, payloadCRC32: UInt32) async throws -> ArchiveRideResult {
        try await acquire()
        defer { release() }
        _ = try await identifyStore(expected: storeID)
        var input = request(9, id: objectID, revision: revision, store: storeID)
        input.length = payloadLength
        input.crc = payloadCRC32
        let components = archiveResponseTimeout.components
        let seconds = UInt64(clamping: components.seconds)
        let fractional = UInt64(clamping: components.attoseconds) / 1_000_000_000_000_000
        let (whole, overflow) = seconds.multipliedReportingOverflow(by: 1_000)
        let (milliseconds, sumOverflow) = whole.addingReportingOverflow(fractional)
        let value = try await execute(input, timeout: overflow || sumOverflow ? UInt64.max : milliseconds)
        return ArchiveRideResult(commitSequence: value.sequence, timestamp: value.timestamp)
    }

    private func identifyStore(expected: StoreID? = nil) async throws -> StoreID {
        let input = request(1, store: expected)
        do { return try await execute(input, query: true).store }
        catch is TransferLinkLost {
            try Task.checkCancellation()
            try await link.restore()
            try client().event(10, offset: UInt64(Int(UInt16.max) + FlatStoreV4.controlHeaderLength),
                               length: link.maximumStreamPayload + FlatStoreV4.streamHeaderLength, now: now)
            return try await execute(input, query: true).store
        }
    }

    private func request(_ opcode: UInt32, id: ObjectID? = nil, revision: Revision? = nil,
                         kind: ObjectKind? = nil, store: StoreID? = nil) -> ObcClientRequest {
        var input = ObcClientRequest()
        input.opcode = opcode
        input.object_id = id?.rawValue ?? 0
        input.revision = revision?.rawValue ?? 0
        input.kind = UInt32(kind?.rawValue ?? 0)
        if let store { input.scoped = 1; copy(store, to: &input.store) }
        return input
    }

    private func copy<T>(_ store: StoreID, to output: inout T) {
        withUnsafeMutableBytes(of: &output) { _ = store.bytes.copyBytes(to: $0) }
    }

    private func client() throws -> RustStoreClient {
        if let core { return core }
        let value = try RustStoreClient(payloadCeiling: link.maximumStreamPayload)
        core = value
        return value
    }
    private var now: UInt64 { DispatchTime.now().uptimeNanoseconds / 1_000_000 }

    private func execute(_ input: ObcClientRequest, name: String = "", query: Bool = false,
                         source: Data = Data(), timeout: UInt64? = nil,
                         progress: @escaping @Sendable (Int, Int) -> Void = { _, _ in }) async throws
        -> RustStoreClient.Result {
        try Task.checkCancellation()
        let core = try client()
        generation &+= 1
        let live = generation
        self.source = source
        sink.removeAll(keepingCapacity: true)
        self.progress = progress
        let id = try core.start(input, name: name, query: query, timeout: timeout, now: now)
        queryID = query ? id : nil
        let result = try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                completion = continuation
                scheduleDrain()
            }
        } onCancel: {
            Task { await self.abort(live) }
        }
        try Task.checkCancellation()
        return result
    }

    private func abort(_ live: UInt64) {
        guard generation == live, completion != nil, let core else { return }
        if let queryID { core.cancelQuery(queryID) }
        else { try? core.event(7, now: now) }
        // Stream cancellation does not fabricate a write acknowledgement. Its task must settle.
        for task in writers.values { task.cancel() }
        scheduleDrain()
    }

    private func scheduleDrain() {
        guard !draining else { return }
        draining = true
        Task { await self.drain() }
    }

    private func drain() async {
        guard let core else { draining = false; return }
        while true {
            if let error = receiveFailure {
                receiveFailure = nil
                self.core = nil
                await finish(.failure(error))
                return
            }
            guard let action = deferredAction ?? core.next() else { break }
            deferredAction = nil
            switch action {
            case .send(let token, let channel, let bytes):
                let epoch = ioEpoch
                writers[token] = Task { [weak self, link] in
                    do {
                        if channel == 0 { try await link.sendControlRecord(bytes) }
                        else { try await link.sendStreamRecord(bytes) }
                        await self?.sent(token, epoch: epoch)
                    } catch { await self?.failed(error, token: token, epoch: epoch) }
                }
            case .source(let token, let offset, let length):
                guard let start = Int(exactly: offset), start <= source.count else {
                    try? core.event(8, token: token, now: now); continue
                }
                let end = start + min(length, source.count - start)
                try? core.event(3, token: token, offset: offset, bytes: source.subdata(in: start..<end), now: now)
            case .sink(let token, let offset, let bytes):
                guard offset == UInt64(sink.count) else { try? core.event(8, token: token, now: now); continue }
                sink.append(bytes)
                try? core.event(5, token: token, offset: offset, length: bytes.count, now: now)
            case .resetSink: sink.removeAll(keepingCapacity: true)
            case .progress(let done, let total):
                if let done = Int(exactly: done), let total = Int(exactly: total) { progress(done, total) }
            case .resetChannels: await stopIO()
            case .restore:
                do {
                    try await link.restore()
                    try core.event(10, offset: UInt64(Int(UInt16.max) + FlatStoreV4.controlHeaderLength),
                                   length: link.maximumStreamPayload + FlatStoreV4.streamHeaderLength, now: now)
                } catch { receiveFailure = error }
            case .complete(let result):
                await finish(result)
                return
            case .query(let id, let result):
                if queryID == id { await finish(result); return }
            }
        }
        if controlReadSettled {
            await controlReader?.drain()
            controlReader = nil; controlReadSettled = false
        }
        if streamReadSettled {
            await streamReader?.drain()
            streamReader = nil; streamReadSettled = false
        }
        if completion != nil {
            if core.reads & 1 != 0 { startReader(stream: false) }
            if core.reads & 2 != 0 { startReader(stream: true) }
        }
        // I/O may settle while the completed reader task is joined above.
        deferredAction = core.next()
        armTimer()
        draining = false
        if deferredAction != nil { scheduleDrain() }
    }

    private func sent(_ token: UInt64, epoch: UInt64) {
        guard ioEpoch == epoch, completion != nil, let core else { return }
        writers.removeValue(forKey: token)
        try? core.event(4, token: token, now: now)
        scheduleDrain()
    }

    private func failed(_ error: Error, token: UInt64 = 0, epoch: UInt64) {
        guard ioEpoch == epoch, completion != nil, let core else { return }
        if error is TransferLinkLost {
            ioEpoch &+= 1
            try? core.event(9, now: now)
        } else if token == 0 {
            // A failed receive cannot settle a write token. Close this failed session after drain.
            receiveFailure = error
        } else { try? core.event(8, token: token, now: now) }
        scheduleDrain()
    }

    private func startReader(stream: Bool) {
        if stream ? streamReader != nil : controlReader != nil { return }
        let epoch = ioEpoch
        let cancellation = ReceiveCancellation()
        let task = Task { [weak self, link] in
            do {
                let bytes = try await withTaskCancellationHandler {
                    if stream { return try await link.receiveStreamRecord() }
                    return try await link.receiveControlRecord()
                } onCancel: {
                    cancellation.request(on: link, stream: stream)
                }
                await self?.readFinished(.success(bytes), stream: stream, epoch: epoch)
            } catch {
                if !Task.isCancelled {
                    await self?.readFinished(.failure(error), stream: stream, epoch: epoch)
                }
            }
        }
        let read = ReceiveTask(task: task, cancellation: cancellation)
        if stream { streamReader = read } else { controlReader = read }
    }

    private func readFinished(_ result: Swift.Result<Data, Error>, stream: Bool, epoch: UInt64) {
        guard epoch == ioEpoch, completion != nil else { return }
        if stream { streamReadSettled = true } else { controlReadSettled = true }
        switch result {
        case .success(let bytes): try? core?.event(stream ? 2 : 1, bytes: bytes, now: now)
        case .failure(let error): failed(error, epoch: epoch)
        }
        scheduleDrain()
    }

    private func stopIO() async {
        ioEpoch &+= 1
        timer?.cancel(); timer = nil
        let reads = [controlReader, streamReader].compactMap { $0 }
        controlReader = nil; streamReader = nil
        controlReadSettled = false; streamReadSettled = false
        let writes = Array(writers.values)
        writers.removeAll()
        for read in reads { read.task.cancel() }
        for task in writes { task.cancel() }
        for read in reads { await read.drain() }
        for task in writes { await task.value }
    }

    private func finish(_ result: Swift.Result<RustStoreClient.Result, Error>) async {
        await stopIO()
        let continuation = completion
        completion = nil
        queryID = nil
        source.removeAll(keepingCapacity: true)
        draining = false
        continuation?.resume(with: result)
    }

    private func armTimer() {
        timer?.cancel(); timer = nil
        guard completion != nil, let deadline = core?.deadline else { return }
        let live = generation
        let delay = deadline > now ? deadline - now : 0
        timer = Task { [weak self] in
            do { try await Task.sleep(for: .milliseconds(Int64(clamping: delay))) }
            catch { return }
            await self?.tick(live)
        }
    }
    private func tick(_ live: UInt64) {
        guard generation == live, completion != nil else { return }
        try? core?.event(6, now: now)
        scheduleDrain()
    }

    private func acquire() async throws {
        try Task.checkCancellation()
        if !busy { busy = true; return }
        await withCheckedContinuation { operationWaiters.append($0) }
        do { try Task.checkCancellation() }
        catch { release(); throw error }
    }
    private func release() {
        if operationWaiters.isEmpty { busy = false } else { operationWaiters.removeFirst().resume() }
    }
}

public enum CRC32 {
    public static func checksum(_ data: Data) -> UInt32 { RustStoreClient.checksum(data) }
}

/// Cancellation can release a read before its physical cleanup returns. Both lifetimes belong
/// to the receive barrier; no cleanup task may reach the next operation's channel.
private struct ReceiveTask {
    let task: Task<Void, Never>
    let cancellation: ReceiveCancellation

    func drain() async {
        await task.value
        await cancellation.drain()
    }
}

private final class ReceiveCancellation: @unchecked Sendable {
    private let lock = NSLock()
    private var task: Task<Void, Never>?

    func request(on link: any TransferLink, stream: Bool) {
        lock.withLock {
            guard task == nil else { return }
            task = Task {
                if stream { await link.cancelStreamReceive() }
                else { await link.cancelControlReceive() }
            }
        }
    }

    func drain() async {
        let pending = lock.withLock { task }
        await pending?.value
    }
}
