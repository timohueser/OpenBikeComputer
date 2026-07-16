import Foundation
import OBCDomain

/// The byte layer (Tier 2). Moves an object's **raw payload bytes** over a
/// `ByteChannel` (the L2CAP CoC on the real path). There is **no per-chunk wire
/// framing** — the transfer's metadata + whole-object CRC ride on the control plane
/// (`TransferControl`/`StatusMessage` over GATT), so the MCU can sink bytes straight
/// to flash and CRC them in one pass, with no reassembly buffer.
///
/// Chunking here is purely **write / progress granularity** (aligned to the CoC
/// PDU), not framing. Interrupted transfers are not resumed at this layer — they
/// restart whole (spec §1 principle 4) — so both directions are plain one-shot
/// async calls, cancelable via task cancellation. `MockTransport` bypasses this
/// entirely.
public struct BLEChannel: Sendable {
    private let channel: any ByteChannel
    private let chunkSize: Int

    /// One CoC SDU on a 2M-PHY + DLE link (251-byte PDU − L2CAP header) — the write
    /// and progress granularity. `OBCProtocol.md` → *Data plane*.
    public static let defaultChunkSize = 244

    public init(channel: any ByteChannel, chunkSize: Int = BLEChannel.defaultChunkSize) {
        self.channel = channel
        self.chunkSize = max(1, chunkSize)
    }

    /// Stream the whole object as raw bytes (app → device). The caller has already
    /// announced the transfer (`TransferControl`, incl. the whole-object CRC) on the
    /// control plane and awaits the device's closing `transferResult` afterwards —
    /// returning from here only means every byte was handed to the channel.
    /// Throws `ChannelDropped` on a dead link and `CancellationError` on cancel.
    public func send(
        _ object: Data, progress: @Sendable (TransferProgress) -> Void = { _ in }
    ) async throws {
        var done = 0
        while done < object.count {
            try Task.checkCancellation()
            let end = min(done + chunkSize, object.count)
            try await channel.write(object.subdata(in: (object.startIndex + done)..<(object.startIndex + end)))
            done = end
            progress(TransferProgress(bytesDone: done, total: object.count))
        }
    }

    /// Read `length` raw bytes (device → app), CRC-ing as they arrive and rejecting
    /// on mismatch (`DeviceError.crcMismatch`) — the object is never committed on a
    /// bad CRC. Throws `DeviceError.transferDropped` on EOF before `length`.
    public func receive(
        length: Int, expectedCRC: UInt32,
        progress: @Sendable (TransferProgress) -> Void = { _ in }
    ) async throws -> Data {
        var buffer = Data(capacity: length)
        var hasher = CRC32.Hasher()
        while buffer.count < length {
            try Task.checkCancellation()
            let chunk: Data
            do {
                chunk = try await channel.read(maxLength: min(chunkSize, length - buffer.count))
            } catch {
                throw (error as? DeviceError) ?? DeviceError.transferDropped
            }
            if chunk.isEmpty { throw DeviceError.transferDropped }  // EOF before `length`
            hasher.update(chunk)
            buffer.append(chunk)
            progress(TransferProgress(bytesDone: buffer.count, total: length))
        }
        guard hasher.finalize() == expectedCRC else { throw DeviceError.crcMismatch }
        return buffer
    }

    /// Tear the underlying channel down (idempotent) — unblocks a peer parked on
    /// backpressure and, on the real path, makes the device discard its partial.
    public func close() async {
        await channel.close()
    }
}
