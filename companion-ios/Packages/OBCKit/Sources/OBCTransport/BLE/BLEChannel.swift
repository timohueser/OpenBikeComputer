import Foundation
import OBCProtocolV4
import OBCDomain

/// The physical protocol-v4 stream channel over the L2CAP CoC. Each write is one complete
/// `StreamRecord`; reads reassemble that same record from CoreBluetooth's partial `InputStream`
/// delivery. Announce/result correlation and recovery live in `TransferClient`, not here.
public struct BLEChannel: Sendable {
    private let channel: any ByteChannel
    private let chunkSize: Int

    /// Conservative outbound record size. The peer's receive limit is independent of ours.
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

    /// Reassembles a record from the byte stream. The peer can use a different chunk size;
    /// the wire's UInt16 payload length bounds the receive allocation.
    public func receiveRecord() async throws -> Data {
        var record = Data()
        do {
            try await readExactly(FlatStoreV4.streamHeaderLength, into: &record)
            let b = record.startIndex
            let payloadLength = Int(record[b + 12]) | (Int(record[b + 13]) << 8)
            guard payloadLength > 0 else { throw DeviceError.transferRejected }
            try await readExactly(FlatStoreV4.streamHeaderLength + payloadLength, into: &record)
            _ = try StreamRecord(decoding: record)
            return record
        } catch {
            // Once any bytes are consumed, abandoning this record loses framing. Only a
            // cancellation between records can keep the channel for the next request.
            if !record.isEmpty || !(error is CancellationError) { await channel.close() }
            throw error
        }
    }

    public func cancelReceive() {
        channel.cancelRead()
    }

    private func readExactly(_ length: Int, into record: inout Data) async throws {
        while record.count < length {
            let part = try await channel.read(maxLength: length - record.count)
            if part.isEmpty { throw ChannelDropped() }
            record.append(part)
        }
    }

    /// Tear the underlying channel down. It unblocks a peer parked on backpressure and, on the
    /// real path, makes the device discard its partial.
    public func close() async {
        await channel.close()
    }
}
