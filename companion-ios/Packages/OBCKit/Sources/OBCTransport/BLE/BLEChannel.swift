import Foundation
import OBCDomain

/// The byte layer (Tier 2). Moves typed objects over a `ByteChannel` (the L2CAP
/// CoC on the real path) using the frame codec: chunked, **resumable** by offset,
/// **cancelable**, CRC-validated per frame. Backs the `TransferHandle` that
/// `uploadRoute` (B5) and `downloadRides` (B7) return.
///
/// `MockTransport` **bypasses this entirely** — all wire logic lives here so the
/// mock stays a thin fixture server (epic architecture, `OBCProtocol.md`).
///
/// A value type holding only immutable references — per-transfer mutable state
/// lives in the `Uploader` actor — so `upload`/`download` are callable synchronously.
public struct BLEChannel: Sendable {
    private let channel: any ByteChannel
    private let chunkSize: Int

    public init(channel: any ByteChannel, chunkSize: Int = FrameFormat.defaultChunkSize) {
        self.channel = channel
        self.chunkSize = max(1, chunkSize)
    }

    /// Frame `object` and stream it out (app → device). The returned handle reports
    /// progress and drives cancel/resume.
    public func upload(_ object: Data, type: ObjectType, objectID: UInt16) -> TransferHandle {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let transfer = Uploader(
            channel: channel, chunkSize: chunkSize,
            object: object, type: type, objectID: objectID,
            progress: continuation
        )
        Task { await transfer.start() }
        return TransferHandle(
            progress: stream,
            onCancel: { Task { await transfer.cancel() } },
            onResume: { Task { await transfer.resume() } }
        )
    }

    /// Read one framed object (device → app), validating each frame's CRC and
    /// reassembling by offset. Returns the handle the UI observes plus a task that
    /// resolves to the committed object (or throws `DeviceError`).
    public func download(objectID: UInt16) -> (handle: TransferHandle, result: Task<Data, Error>) {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let reader = FrameReader(channel: channel)
        let ch = channel
        let task = Task<Data, Error> {
            var assembler = TransferAssembler()
            do {
                while let frame = try await reader.next() {
                    guard frame.header.objectID == objectID else { continue }
                    let done = try assembler.ingest(header: frame.header, payload: frame.payload)
                    continuation.yield(TransferProgress(
                        bytesDone: assembler.committedLength,
                        total: assembler.total ?? 0,
                        offset: assembler.committedLength
                    ))
                    if done { break }
                }
            } catch let error as FramingError {
                continuation.finish()
                throw error == .crcMismatch ? DeviceError.crcMismatch : DeviceError.transferDropped
            } catch {
                continuation.finish()
                throw DeviceError.transferDropped
            }
            continuation.finish()
            guard let object = assembler.object else { throw DeviceError.transferDropped }
            return object
        }
        return (
            TransferHandle(
                progress: stream,
                onCancel: { task.cancel(); Task { await ch.close() } },
                onResume: {}  // download resume re-opens the CoC at the transport level (A5-gated)
            ),
            task
        )
    }
}

/// One in-flight upload. An actor so `cancel()`/`resume()` and the send loop don't
/// race. Tracks `committed` = end offset of the last **fully written** frame — the
/// resume anchor after a drop (a partially written frame is never committed, so
/// resume re-sends it whole).
private actor Uploader {
    let channel: any ByteChannel
    let chunkSize: Int
    let object: Data
    let type: ObjectType
    let objectID: UInt16
    let progress: AsyncStream<TransferProgress>.Continuation

    var committed = 0
    var running = false
    var canceled = false
    var torndown = false

    init(channel: any ByteChannel, chunkSize: Int, object: Data, type: ObjectType, objectID: UInt16,
         progress: AsyncStream<TransferProgress>.Continuation) {
        self.channel = channel
        self.chunkSize = chunkSize
        self.object = object
        self.type = type
        self.objectID = objectID
        self.progress = progress
    }

    func start() async { await pump() }

    private func pump() async {
        guard !running, !canceled, !torndown else { return }
        running = true
        let total = object.count

        // A zero-length object still needs one frame so the receiver sees totalLen.
        if total == 0, committed == 0 {
            let frame = FrameCodec.encode(type: type, objectID: objectID, totalLen: 0, offset: 0, payload: Data())
            try? await channel.write(frame)
            progress.yield(TransferProgress(bytesDone: 0, total: 0, offset: 0))
            running = false
            progress.finish()
            return
        }

        while committed < total {
            if canceled { break }
            let end = min(committed + chunkSize, total)
            let payload = Data(object[(object.startIndex + committed)..<(object.startIndex + end)])
            let frame = FrameCodec.encode(
                type: type, objectID: objectID,
                totalLen: UInt32(total), offset: UInt32(committed), payload: payload
            )
            do {
                try await channel.write(frame)
            } catch {
                // A `cancel()` closes the channel, which surfaces here as a write
                // error — teardown is already in flight, so just stop. Otherwise it's
                // a genuine drop: leave `committed` at the last good boundary and stop
                // with the stream open so `resume()` can continue into it.
                running = false
                return
            }
            committed = end
            progress.yield(TransferProgress(bytesDone: committed, total: total, offset: committed))
        }

        running = false
        if canceled {
            await teardown()
        } else {
            progress.finish()  // complete
        }
    }

    func cancel() async {
        canceled = true
        // Always tear down: closing the channel also unblocks a write that's parked
        // on backpressure, so the pump can't be stranded mid-transfer.
        await teardown()
    }

    func resume() async {
        guard !canceled, !torndown, committed < object.count else { return }
        await pump()
    }

    /// Cancel teardown: close the channel and finish the stream (idempotent). The
    /// out-of-band abort over `TransferControl` is a `BLETransport`/GATT concern.
    private func teardown() async {
        guard !torndown else { return }
        torndown = true
        await channel.close()
        progress.finish()
    }
}

/// Pulls whole frames off a `ByteChannel`, reassembling each from arbitrary read
/// sizes. `next()` returns `nil` at a clean end-of-stream (frame boundary) and
/// throws `ChannelDropped` on a mid-frame drop or `FramingError` on corruption.
struct FrameReader: Sendable {
    let channel: any ByteChannel

    func next() async throws -> (header: FrameHeader, payload: Data)? {
        var head = Data()
        while head.count < FrameFormat.headerSize {
            let chunk = try await channel.read(maxLength: FrameFormat.headerSize - head.count)
            if chunk.isEmpty {
                if head.isEmpty { return nil }  // clean EOF at a frame boundary
                throw ChannelDropped()          // dropped mid-header
            }
            head.append(chunk)
        }
        let header = try FrameCodec.parseHeader(head)
        let payload = try await channel.readExactly(Int(header.chunkLen))
        try FrameCodec.verify(header, payload: payload)  // reject corrupt frames
        return (header, payload)
    }
}
