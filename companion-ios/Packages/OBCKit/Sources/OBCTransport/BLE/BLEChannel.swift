import Foundation
import OBCDomain

/// The byte layer (Tier 2). Moves an object's **raw payload bytes** over a
/// `ByteChannel` (the L2CAP CoC on the real path). There is **no per-chunk wire
/// framing** — the transfer's metadata + whole-object CRC ride on the control plane
/// (`TransferStart`/`TransferResult` over GATT), so the MCU can sink bytes straight
/// to flash and CRC them in one pass, with no reassembly buffer.
///
/// Chunking here is purely **write / progress / resume granularity** (aligned to the
/// CoC PDU), not framing. The transfer is **resumable** by offset and **cancelable**
/// (channel teardown). `MockTransport` bypasses this entirely.
public struct BLEChannel: Sendable {
    private let channel: any ByteChannel
    private let chunkSize: Int

    /// One CoC SDU on a 2M-PHY + DLE link (251-byte PDU − L2CAP header) — the write
    /// and resume granularity. `OBCProtocol.md` → *Data plane*.
    public static let defaultChunkSize = 244

    public init(channel: any ByteChannel, chunkSize: Int = BLEChannel.defaultChunkSize) {
        self.channel = channel
        self.chunkSize = max(1, chunkSize)
    }

    /// Stream `object[offset...]` as raw bytes (app → device). The caller has already
    /// announced the transfer (`TransferStart`, incl. the whole-object CRC) on the
    /// control plane. Returns the handle the UI observes; `resume()` restarts from the
    /// last committed offset.
    public func upload(_ object: Data, from offset: Int = 0) -> TransferHandle {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let transfer = Uploader(channel: channel, chunkSize: chunkSize, object: object,
                                startOffset: offset, progress: continuation)
        Task { await transfer.start() }
        return TransferHandle(
            progress: stream,
            onCancel: { Task { await transfer.cancel() } },
            onResume: { Task { await transfer.resume() } }
        )
    }

    /// Read `length` raw bytes (device → app), CRC-ing as they arrive and rejecting on
    /// mismatch (`DeviceError.crcMismatch`) — the object is never committed on a bad
    /// CRC. Returns the handle plus a task resolving to the verified object.
    public func download(length: Int, expectedCRC: UInt32) -> (handle: TransferHandle, result: Task<Data, Error>) {
        let (stream, continuation) = AsyncStream<TransferProgress>.makeStream()
        let ch = channel
        let cs = chunkSize
        let task = Task<Data, Error> {
            var buffer = Data(capacity: length)
            var hasher = CRC32.Hasher()
            do {
                while buffer.count < length {
                    let chunk = try await ch.read(maxLength: min(cs, length - buffer.count))
                    if chunk.isEmpty { throw DeviceError.transferDropped }  // EOF before `length`
                    hasher.update(chunk)
                    buffer.append(chunk)
                    continuation.yield(TransferProgress(bytesDone: buffer.count, total: length, offset: buffer.count))
                }
            } catch {
                continuation.finish()
                throw (error as? DeviceError) ?? .transferDropped
            }
            continuation.finish()
            guard hasher.finalize() == expectedCRC else { throw DeviceError.crcMismatch }
            return buffer
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
/// race. `committed` = bytes fully handed to the channel — the resume anchor after a
/// drop (a chunk that failed to write is never committed, so resume re-sends it).
private actor Uploader {
    let channel: any ByteChannel
    let chunkSize: Int
    let object: Data
    let progress: AsyncStream<TransferProgress>.Continuation

    var committed: Int
    var running = false
    var canceled = false
    var torndown = false

    init(channel: any ByteChannel, chunkSize: Int, object: Data, startOffset: Int,
         progress: AsyncStream<TransferProgress>.Continuation) {
        self.channel = channel
        self.chunkSize = chunkSize
        self.object = object
        self.committed = min(max(0, startOffset), object.count)
        self.progress = progress
    }

    func start() async { await pump() }

    private func pump() async {
        guard !running, !canceled, !torndown else { return }
        running = true
        let total = object.count

        while committed < total {
            if canceled { break }
            let end = min(committed + chunkSize, total)
            let chunk = object.subdata(in: (object.startIndex + committed)..<(object.startIndex + end))
            do {
                try await channel.write(chunk)
            } catch {
                // Drop (or a cancel that closed the channel): stop with `committed` at
                // the last good boundary. The stream stays open so `resume()` can
                // continue into it.
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
        // Always tear down: closing the channel also unblocks a write parked on
        // backpressure, so the pump can't be stranded mid-transfer.
        await teardown()
    }

    func resume() async {
        guard !canceled, !torndown, committed < object.count else { return }
        await pump()
    }

    private func teardown() async {
        guard !torndown else { return }
        torndown = true
        await channel.close()
        progress.finish()
    }
}
