import Foundation
import OBCProtocolV4
import OBCDomain

/// The physical protocol-v4 stream channel over the L2CAP CoC. Each write is one complete
/// `StreamRecord`; reads reassemble that same record from CoreBluetooth's partial `InputStream`
/// delivery. Announce/result correlation and recovery live in `TransferClient`, not here.
public struct BLEChannel: Sendable {
    private let channel: any ByteChannel
    private let chunkSize: Int

    /// One CoC SDU on a 2M-PHY + DLE link (251-byte PDU − L2CAP header).
    public static let defaultChunkSize = 244

    public init(channel: any ByteChannel, chunkSize: Int = BLEChannel.defaultChunkSize) {
        self.channel = channel
        self.chunkSize = max(1, chunkSize)
    }

    /// Maximum protocol payload that leaves the 16-byte v4 stream header inside one CoC SDU.
    public var maximumRecordPayload: Int { max(0, chunkSize - FlatStoreV4.streamHeaderLength) }

    /// One complete protocol-v4 stream record in one CoC SDU.
    public func sendRecord(_ record: Data) async throws {
        guard record.count <= chunkSize else { throw DeviceError.transferRejected }
        _ = try StreamRecord(decoding: record)
        try await channel.write(record)
    }

    /// Reassembles one protocol-v4 stream record from CoreBluetooth's byte-stream presentation of
    /// the CoC. The wire still carries exactly one record per SDU; this loop only handles partial
    /// `InputStream` reads.
    public func receiveRecord() async throws -> Data {
        let header = try await readExactly(FlatStoreV4.streamHeaderLength)
        let b = header.startIndex
        let payloadLength = Int(header[b + 12]) | (Int(header[b + 13]) << 8)
        guard payloadLength > 0, payloadLength <= maximumRecordPayload else {
            throw DeviceError.transferRejected
        }
        let record = header + (try await readExactly(payloadLength))
        _ = try StreamRecord(decoding: record)
        return record
    }

    public func cancelReceive() {
        channel.cancelRead()
    }

    private func readExactly(_ length: Int) async throws -> Data {
        var out = Data(capacity: length)
        while out.count < length {
            let part = try await channel.read(maxLength: length - out.count)
            if part.isEmpty { throw ChannelDropped() }
            out.append(part)
        }
        return out
    }

    /// Legacy raw-byte seam retained for the in-package echo harness. The live companion path uses
    /// `sendRecord(_:)` exclusively.
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

    /// Legacy raw-byte seam retained for the in-package echo harness. The live companion path uses
    /// `receiveRecord()` exclusively.
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
