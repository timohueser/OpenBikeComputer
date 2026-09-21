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

    /// Reassembles one protocol-v4 stream record from CoreBluetooth's byte-stream presentation
    /// of the CoC. The wire still carries exactly one record per SDU; this loop only handles
    /// partial `InputStream` reads.
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

    /// Tear the underlying channel down. It unblocks a peer parked on backpressure and, on the
    /// real path, makes the device discard its partial.
    public func close() async {
        await channel.close()
    }
}
