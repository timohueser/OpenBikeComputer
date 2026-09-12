import Foundation
import Testing
import OBCDomain
import OBCProtocolV4
@testable import OBCTransport

@Suite("BLE record channel")
struct BLEChannelTests {
    private func record(offset: UInt64 = 0) throws -> Data {
        try StreamRecord(requestID: RequestID(rawValue: 7)!, offset: offset, payload: Data([1, 2, 3, 4])).encode()
    }

    @Test("Each write stays one record; partial reads preserve record boundaries", arguments: [1, 7, 16, 64])
    func recordBoundaries(readSize: Int) async throws {
        let pipe = BufferedChannel(readSize: readSize)
        let channel = BLEChannel(channel: pipe, chunkSize: 20)
        let first = try record(), second = try record(offset: 4)
        try await channel.sendRecord(first)
        try await channel.sendRecord(second)
        #expect(pipe.writes == [first, second])
        #expect(try await channel.receiveRecord() == first)
        #expect(try await channel.receiveRecord() == second)
    }

    @Test("Oversized and malformed writes are refused before reaching the pipe")
    func invalidWrites() async throws {
        let pipe = BufferedChannel()
        let channel = BLEChannel(channel: pipe, chunkSize: 20)
        let valid = try record()
        await #expect(throws: DeviceError.transferRejected) { try await channel.sendRecord(valid + Data([0])) }
        var malformed = valid
        malformed[14] = 1
        await #expect(throws: WireError.invalidReserved) { try await channel.sendRecord(malformed) }
        #expect(pipe.writes.isEmpty)
    }

    @Test("Incomplete and invalid incoming records cannot be returned")
    func invalidReads() async throws {
        let valid = try record()
        for prefix in [Data(valid.prefix(5)), Data(valid.dropLast())] {
            let pipe = BufferedChannel()
            try await pipe.write(prefix)
            let channel = BLEChannel(channel: pipe)
            await #expect(throws: ChannelDropped.self) { try await channel.receiveRecord() }
        }
        for length in [0, 5] {
            var header = Data(valid.prefix(16))
            header[12] = UInt8(length)
            let pipe = BufferedChannel()
            try await pipe.write(header)
            let channel = BLEChannel(channel: pipe, chunkSize: 20)
            await #expect(throws: DeviceError.transferRejected) { try await channel.receiveRecord() }
        }
    }

    @Test("Cancellation and close reach the physical byte channel")
    func physicalControl() async {
        let pipe = BufferedChannel()
        let channel = BLEChannel(channel: pipe)
        channel.cancelReceive()
        #expect(pipe.cancels == 1)
        await channel.close()
        await #expect(throws: ChannelDropped.self) { try await channel.sendRecord(record()) }
    }
}

/// Finite input with bounded reads, so malformed records fail without a timer or parked task.
private final class BufferedChannel: ByteChannel, @unchecked Sendable {
    private let lock = NSLock()
    private let readSize: Int
    private var buffer = Data()
    private var written: [Data] = []
    private var cancelled = 0
    private var closed = false

    init(readSize: Int = 64) { self.readSize = readSize }
    var writes: [Data] { lock.withLock { written } }
    var cancels: Int { lock.withLock { cancelled } }

    func write(_ data: Data) async throws {
        try lock.withLock {
            if closed { throw ChannelDropped() }
            written.append(data)
            buffer.append(data)
        }
    }

    func read(maxLength: Int) async throws -> Data {
        lock.withLock {
            let part = Data(buffer.prefix(min(readSize, maxLength)))
            buffer.removeFirst(part.count)
            return part
        }
    }

    func cancelRead() { lock.withLock { cancelled += 1 } }
    func close() async { lock.withLock { closed = true } }
}
