import Foundation

/// The physical facts protocol v4 needs from BLE or USB. Implementations preserve record
/// boundaries, order a control write before stream records for that request, and restore a broken
/// link. They do not interpret frames or retain operation state.
public protocol TransferLink: Sendable {
    var maximumStreamPayload: Int { get }
    func sendControlRecord(_ record: Data) async throws
    func receiveControlRecord() async throws -> Data
    func sendStreamRecord(_ record: Data) async throws
    func receiveStreamRecord() async throws -> Data
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
    case catalogChanged
    case storeChanged(previous: StoreID, current: StoreID)
    case outcomeNotCommitted
}

/// Protocol-v4's one client shape: announce on control, stream on the live link, then consume the
/// one result. A link loss discards transfer bytes, restores the physical link, establishes the
/// store identity with LIST, and reconciles mutations with STATUS (or LIST for a create).
public actor TransferClient {
    private let link: any TransferLink
    private var nextRequestValue: UInt32
    private var currentStoreID: StoreID?

    // Actor reentrancy must not turn two callers into two live transfers. This small FIFO is the
    // client's operation gate; BLETransport has no transfer slot or operation queue anymore.
    private var busy = false
    private var operationWaiters: [CheckedContinuation<Void, Never>] = []

    public init(link: any TransferLink, firstRequestID: UInt32 = 1) {
        self.link = link
        self.nextRequestValue = firstRequestID == 0 ? 1 : firstRequestID
    }

    public func list(kind: ObjectKind? = nil) async throws -> [CatalogEntry] {
        await acquire()
        defer { release() }
        do { return try await listAll(kind: kind).entries }
        catch is TransferLinkLost {
            try await restoreAndRefreshStore()
            return try await listAll(kind: kind).entries
        }
    }

    public func catalog(kind: ObjectKind? = nil) async throws -> (storeID: StoreID, entries: [CatalogEntry]) {
        await acquire()
        defer { release() }
        do { return try await listAll(kind: kind) }
        catch is TransferLinkLost {
            try await restoreAndRefreshStore()
            return try await listAll(kind: kind)
        }
    }

    /// Establish the store identity from the first LIST page without walking the whole catalog.
    /// A BLE page carries only two entries at the preferred MTU, so using `catalog()` merely to
    /// learn the StoreId turns a full benchmark card into hundreds of needless control writes.
    public func storeID() async throws -> StoreID {
        await acquire()
        defer { release() }
        do { return try await identifyStore() }
        catch is TransferLinkLost {
            try await restoreAndRefreshStore()
            guard let currentStoreID else { throw TransferClientError.unexpectedResponse }
            return currentStoreID
        }
    }

    public func status(objectID: ObjectID, revision: Revision) async throws -> StatusResult {
        await acquire()
        defer { release() }
        try await ensureIntroduced()
        do { return try await statusOnLiveLink(objectID: objectID, revision: revision) }
        catch is TransferLinkLost {
            try await restoreAndRefreshStore()
            return try await statusOnLiveLink(objectID: objectID, revision: revision)
        }
    }

    public func get(
        objectID: ObjectID, revision: Revision? = nil,
        progress: @escaping @Sendable (_ bytesDone: Int, _ total: Int) -> Void = { _, _ in }
    ) async throws -> (result: GetResult, payload: Data) {
        await acquire()
        defer { release() }
        try await ensureIntroduced()
        do {
            return try await getOnLiveLink(objectID: objectID, revision: revision, progress: progress)
        } catch is TransferLinkLost {
            try await restoreAndRefreshStore()
            // GET has no durable effect. A fresh request id and a restart at offset zero is its
            // complete recovery; no resume offset exists in v4.
            return try await getOnLiveLink(objectID: objectID, revision: revision, progress: progress)
        }
    }

    public func put(
        _ payload: Data, objectID: ObjectID? = nil, expectedRevision: Revision? = nil,
        kind: ObjectKind, retainPrevious: Bool = false, displayName: String,
        progress: @escaping @Sendable (_ bytesDone: Int, _ total: Int) -> Void = { _, _ in }
    ) async throws -> PutResult {
        await acquire()
        defer { release() }
        try await ensureIntroduced()
        let crc = CRC32.checksum(payload)
        let request = PutRequest(
            objectID: objectID, expectedRevision: expectedRevision,
            payloadLength: UInt64(payload.count), payloadCRC32: crc, kind: kind,
            retainPrevious: retainPrevious, displayName: displayName)
        do {
            return try await putOnLiveLink(request, payload: payload, progress: progress)
        } catch is TransferLinkLost {
            try await restoreAndRefreshStore()
            if let objectID, let expectedRevision {
                guard expectedRevision.rawValue < UInt64.max else { throw TransferClientError.outcomeNotCommitted }
                let produced = Revision(rawValue: expectedRevision.rawValue + 1)
                let status = try await statusOnLiveLink(objectID: objectID, revision: produced)
                if status.state == .committed {
                    guard status.headPayloadLength == UInt64(payload.count),
                        status.headPayloadCRC32 == crc
                    else { throw TransferClientError.outcomeNotCommitted }
                    return PutResult(
                        objectID: objectID, revision: produced,
                        payloadLength: status.headPayloadLength, payloadCRC32: status.headPayloadCRC32)
                }
                // STATUS says the proposed revision is not the head, so the interrupted PUT did
                // not commit. Re-announcing the same CAS is safe: if the catalog changed again the
                // peer returns revisionConflict rather than overwriting it.
                return try await putOnLiveLink(request, payload: payload, progress: progress)
            }

            // A create has no ObjectId until its response. LIST's immutable fingerprint is the
            // normative reconciliation key for the lost assignment.
            let catalog = try await listAll(kind: kind)
            let matches = catalog.entries.filter {
                !$0.flags.contains(.retained)
                    && $0.payloadLength == UInt64(payload.count)
                    && $0.payloadCRC32 == crc
                    && $0.displayName == displayName
            }
            guard let entry = matches.max(by: { $0.objectID < $1.objectID }) else {
                return try await putOnLiveLink(request, payload: payload, progress: progress)
            }
            // A create response can be lost after commit and a refreshed LIST can race the
            // publication once. If that produced duplicates, keep the newest assignment and
            // remove the others with their exact revisions.
            for duplicate in matches where duplicate.objectID != entry.objectID {
                _ = try await removeOnLiveLink(
                    objectID: duplicate.objectID, expectedRevision: duplicate.revision)
            }
            return PutResult(
                objectID: entry.objectID, revision: entry.revision,
                payloadLength: entry.payloadLength, payloadCRC32: entry.payloadCRC32)
        }
    }

    public func remove(objectID: ObjectID, expectedRevision: Revision) async throws -> RemoveResult {
        await acquire()
        defer { release() }
        try await ensureIntroduced()
        do {
            return try await removeOnLiveLink(objectID: objectID, expectedRevision: expectedRevision)
        } catch is TransferLinkLost {
            try await restoreAndRefreshStore()
            let result = try await statusOnLiveLink(objectID: objectID, revision: expectedRevision)
            if result.state == .absent { return RemoveResult(commitSequence: nil) }
            // The exact revision is still the head: the remove did not land, and its CAS remains
            // valid, so finish it once on the restored link. A different head is a real conflict.
            if result.state == .committed {
                return try await removeOnLiveLink(objectID: objectID, expectedRevision: expectedRevision)
            }
            throw TransferClientError.outcomeNotCommitted
        }
    }

    public func cancel(transfer: RequestID) async throws -> CancelResult {
        await acquire()
        defer { release() }
        try await ensureIntroduced()
        let response = try await request(.cancel(transfer: transfer), opcode: .cancel)
        guard case .cancel(let result) = response else { throw TransferClientError.unexpectedResponse }
        return result
    }

    public func arm(packageObjectID: ObjectID, expectedRevision: Revision) async throws -> ArmResult {
        await acquire()
        defer { release() }
        try await ensureIntroduced()
        let response = try await request(
            .arm(packageObjectID: packageObjectID, expectedRevision: expectedRevision), opcode: .arm)
        guard case .arm(let result) = response else { throw TransferClientError.unexpectedResponse }
        return result
    }

    private func getOnLiveLink(
        objectID: ObjectID, revision: Revision?,
        progress: @escaping @Sendable (Int, Int) -> Void
    ) async throws -> (result: GetResult, payload: Data) {
        let requestID = try makeRequestID()
        try await link.sendControlRecord(
            ControlRequest.get(objectID: objectID, revision: revision).frame(requestID: requestID).encode())

        enum Part: Sendable { case result(GetResult, complete: Bool), streamComplete }
        let accumulator = DownloadAccumulator(requestID: requestID)
        var result: GetResult?
        do {
            try await withThrowingTaskGroup(of: Part.self) { group in
                group.addTask { [link] in
                    let record = try await link.receiveControlRecord()
                    let response = try ControlResponse(
                        decoding: record, expectedOpcode: .get, expectedRequestID: requestID)
                    guard case .get(let result) = response else { throw TransferClientError.unexpectedResponse }
                    return .result(result, complete: await accumulator.set(result: result))
                }
                group.addTask { [link] in
                    do {
                        while true {
                            let record = try StreamRecord(decoding: try await link.receiveStreamRecord())
                            // Stream traffic can outlive the answer that ended an earlier transfer.
                            // Request ids name the live direction; every other valid record is late
                            // traffic and is discarded in silence (§3.8).
                            guard record.requestID == requestID else { continue }
                            if try await accumulator.append(record) { return .streamComplete }
                        }
                    } catch is CancellationError {
                        return .streamComplete
                    }
                }
                for try await part in group {
                    switch part {
                    case .result(let value, let complete):
                        result = value
                        if complete {
                            await link.cancelStreamReceive()
                            group.cancelAll()
                        }
                    case .streamComplete:
                        if result != nil { group.cancelAll() }
                    }
                }
            }
        } catch {
            await link.cancelStreamReceive()
            throw error
        }
        let payload = await accumulator.payload
        guard let result else { throw TransferClientError.lengthMismatch }
        guard UInt64(payload.count) == result.payloadLength else { throw TransferClientError.lengthMismatch }
        guard CRC32.checksum(payload) == result.payloadCRC32 else { throw TransferClientError.checksumMismatch }
        progress(payload.count, payload.count)
        return (result, payload)
    }

    private func putOnLiveLink(
        _ request: PutRequest, payload: Data,
        progress: @escaping @Sendable (Int, Int) -> Void
    ) async throws -> PutResult {
        let ceiling = link.maximumStreamPayload
        guard ceiling > 0, ceiling <= Int(UInt16.max) else {
            throw TransferClientError.invalidLinkCeiling(ceiling)
        }
        let requestID = try makeRequestID()
        // Awaiting the response is armed by the peer's indication subscription before this call;
        // this write is the announce, and only after it returns do stream records begin.
        try await link.sendControlRecord(ControlRequest.put(request).frame(requestID: requestID).encode())

        enum Part: Sendable { case result(PutResult), streamed }
        var result: PutResult?
        try await withThrowingTaskGroup(of: Part.self) { group in
            group.addTask { [link] in
                let record = try await link.receiveControlRecord()
                let response = try ControlResponse(
                    decoding: record, expectedOpcode: .put, expectedRequestID: requestID)
                guard case .put(let result) = response else { throw TransferClientError.unexpectedResponse }
                return .result(result)
            }
            group.addTask { [link] in
                var offset = 0
                while offset < payload.count {
                    try Task.checkCancellation()
                    let end = min(offset + ceiling, payload.count)
                    let frame = try StreamRecord(
                        requestID: requestID, offset: UInt64(offset), payload: payload[offset..<end])
                    try await link.sendStreamRecord(frame.encode())
                    offset = end
                    progress(offset, payload.count)
                }
                return .streamed
            }
            var streamed = false
            for try await part in group {
                switch part {
                case .result(let value): result = value
                case .streamed: streamed = true
                }
                if result != nil, streamed { group.cancelAll() }
            }
        }
        guard let result,
            result.payloadLength == UInt64(payload.count),
            result.payloadCRC32 == request.payloadCRC32
        else { throw TransferClientError.unexpectedResponse }
        if let objectID = request.objectID, result.objectID != objectID {
            throw TransferClientError.unexpectedResponse
        }
        return result
    }

    private func removeOnLiveLink(objectID: ObjectID, expectedRevision: Revision) async throws -> RemoveResult {
        let response = try await request(
            .remove(objectID: objectID, expectedRevision: expectedRevision), opcode: .remove)
        guard case .remove(let result) = response else { throw TransferClientError.unexpectedResponse }
        return result
    }

    private func statusOnLiveLink(objectID: ObjectID, revision: Revision) async throws -> StatusResult {
        let response = try await request(.status(objectID: objectID, revision: revision), opcode: .status)
        guard case .status(let result) = response else { throw TransferClientError.unexpectedResponse }
        return result
    }

    private func request(_ request: ControlRequest, opcode: Opcode) async throws -> ControlResponse {
        let requestID = try makeRequestID()
        try await link.sendControlRecord(try request.frame(requestID: requestID).encode())
        return try ControlResponse(
            decoding: try await link.receiveControlRecord(),
            expectedOpcode: opcode, expectedRequestID: requestID)
    }

    private func ensureIntroduced() async throws {
        guard currentStoreID == nil else { return }
        do { _ = try await identifyStore() }
        catch is TransferLinkLost { try await restoreAndRefreshStore() }
    }

    private func restoreAndRefreshStore() async throws {
        let previous = currentStoreID
        currentStoreID = nil
        try await link.restore()
        let refreshed = try await identifyStore()
        if let previous, previous != refreshed {
            throw TransferClientError.storeChanged(previous: previous, current: refreshed)
        }
    }

    /// One cursorless LIST is sufficient to introduce a store: every page carries the same
    /// StoreId and commit sequence. Pagination belongs only to callers that need the entries.
    private func identifyStore() async throws -> StoreID {
        let response = try await request(.list(kind: nil, cursor: nil), opcode: .list)
        guard case .list(let page) = response else { throw TransferClientError.unexpectedResponse }
        if let currentStoreID, currentStoreID != page.storeID {
            self.currentStoreID = page.storeID
            throw TransferClientError.storeChanged(previous: currentStoreID, current: page.storeID)
        }
        currentStoreID = page.storeID
        return page.storeID
    }

    private func listAll(kind: ObjectKind?) async throws -> (storeID: StoreID, entries: [CatalogEntry]) {
        for _ in 0..<3 {
            do { return try await listOneSnapshot(kind: kind) }
            catch WireError.remote(let error) where error.code == .catalogChanged { continue }
        }
        throw TransferClientError.catalogChanged
    }

    private func listOneSnapshot(kind: ObjectKind?) async throws -> (storeID: StoreID, entries: [CatalogEntry]) {
        var cursor: CatalogCursor?
        var storeID: StoreID?
        var commitSequence: UInt64?
        var entries: [CatalogEntry] = []
        repeat {
            let response = try await request(.list(kind: kind, cursor: cursor), opcode: .list)
            guard case .list(let page) = response else { throw TransferClientError.unexpectedResponse }
            if let kind, page.entries.contains(where: { $0.kind != kind }) {
                throw TransferClientError.unexpectedResponse
            }
            if let storeID, storeID != page.storeID { throw TransferClientError.catalogChanged }
            if let commitSequence, commitSequence != page.commitSequence { throw TransferClientError.catalogChanged }
            storeID = page.storeID
            commitSequence = page.commitSequence
            entries.append(contentsOf: page.entries)
            cursor = page.hasMore ? page.entries.last.map {
                CatalogCursor(
                    objectID: $0.objectID, revision: $0.revision,
                    commitSequence: page.commitSequence)
            } : nil
            if page.hasMore, cursor == nil { throw TransferClientError.unexpectedResponse }
        } while cursor != nil
        guard let storeID else { throw TransferClientError.unexpectedResponse }
        if let currentStoreID, currentStoreID != storeID {
            self.currentStoreID = storeID
            throw TransferClientError.storeChanged(previous: currentStoreID, current: storeID)
        }
        currentStoreID = storeID
        return (storeID, entries)
    }

    private func makeRequestID() throws -> RequestID {
        guard let id = RequestID(rawValue: nextRequestValue) else { throw TransferClientError.requestIDExhausted }
        if nextRequestValue == UInt32.max { nextRequestValue = 1 } else { nextRequestValue += 1 }
        return id
    }

    private func acquire() async {
        if !busy { busy = true; return }
        await withCheckedContinuation { operationWaiters.append($0) }
    }

    private func release() {
        if operationWaiters.isEmpty { busy = false } else { operationWaiters.removeFirst().resume() }
    }
}

public enum CRC32 {
    public static func checksum(_ data: Data) -> UInt32 {
        var crc: UInt32 = 0xFFFF_FFFF
        for byte in data {
            crc ^= UInt32(byte)
            for _ in 0..<8 {
                crc = (crc & 1 == 1) ? (crc >> 1) ^ 0xEDB8_8320 : crc >> 1
            }
        }
        return crc ^ 0xFFFF_FFFF
    }
}

private actor DownloadAccumulator {
    private let requestID: RequestID
    private var bytes = Data()
    private var expectedLength: UInt64?

    init(requestID: RequestID) { self.requestID = requestID }

    func set(result: GetResult) -> Bool {
        expectedLength = result.payloadLength
        return UInt64(bytes.count) == result.payloadLength
    }

    func append(_ record: StreamRecord) throws -> Bool {
        guard record.requestID == requestID, record.offset == UInt64(bytes.count) else {
            throw TransferClientError.unexpectedStream
        }
        bytes.append(record.payload)
        if let expectedLength, UInt64(bytes.count) > expectedLength {
            throw TransferClientError.lengthMismatch
        }
        return expectedLength == UInt64(bytes.count)
    }

    var payload: Data { bytes }
}
