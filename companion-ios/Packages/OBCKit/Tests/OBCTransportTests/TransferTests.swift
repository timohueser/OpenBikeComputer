import XCTest
import OBCDomain
@testable import OBCTransport

/// End-to-end bulk transfer over the in-memory `PipeByteChannel`: raw-byte streaming
/// (no wire framing), whole-object CRC verify, whole-object restart after an induced
/// drop, and clean cancel teardown — all with no hardware.
final class TransferTests: XCTestCase {
    // MARK: Round-trip (upload → device reassembles + CRC matches)

    func testSendStreamsRawBytesByteIdentical() async throws {
        let object = Data((0..<3000).map { UInt8(($0 * 13 + 7) & 0xFF) })
        let pipe = PipeByteChannel()
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let received = deviceReceive(pipe, length: object.count)
        try await channel.send(object)

        let bytes = try await withTimeout(5) { try await received.value }
        XCTAssertEqual(bytes, object)
        XCTAssertEqual(CRC32.checksum(bytes), CRC32.checksum(object))  // CRC the phone announces
    }

    func testSendReportsMonotonicProgress() async throws {
        let object = Data(repeating: 0x5A, count: 1000)
        let pipe = PipeByteChannel()
        let channel = BLEChannel(channel: pipe, chunkSize: 128)
        let ticks = Ticks()

        let received = deviceReceive(pipe, length: object.count)
        try await channel.send(object) { tick in ticks.append(tick.bytesDone) }
        _ = try await withTimeout(5) { try await received.value }

        let seen = ticks.values
        XCTAssertEqual(seen.last, object.count)
        XCTAssertEqual(seen, seen.sorted(), "progress never rewinds within an attempt")
    }

    // MARK: Round-trip (download verifies the announced CRC)

    func testReceiveVerifiesCRC() async throws {
        let object = Data((0..<2500).map { UInt8(($0 * 5) & 0xFF) })
        let pipe = PipeByteChannel()
        try await pipe.write(object)  // device streams raw bytes

        let channel = BLEChannel(channel: pipe, chunkSize: 128)
        let got = try await withTimeout(5) {
            try await channel.receive(length: object.count, expectedCRC: CRC32.checksum(object))
        }
        XCTAssertEqual(got, object)
    }

    // MARK: CRC reject — an error the link CRC missed

    func testReceiveRejectsSilentlyCorruptedObject() async throws {
        let object = Data((0..<1500).map { UInt8($0 & 0xFF) })
        let pipe = PipeByteChannel()
        await pipe.corruptByte(at: 900)   // flip one bit in transit
        try await pipe.write(object)

        let channel = BLEChannel(channel: pipe, chunkSize: 128)
        do {
            _ = try await withTimeout(5) {
                try await channel.receive(length: object.count, expectedCRC: CRC32.checksum(object))
            }
            XCTFail("expected crcMismatch")
        } catch let error as DeviceError {
            XCTAssertEqual(error, .crcMismatch)  // rejected, never committed
        }
    }

    // MARK: Drop → whole-object restart (spec §1 principle 4)

    func testDroppedSendRestartsWhole() async throws {
        let object = Data((0..<2000).map { UInt8($0 & 0xFF) })
        let pipe = PipeByteChannel()
        await pipe.failAfter(500)  // drop partway through
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        do {
            try await channel.send(object)
            XCTFail("expected the induced drop")
        } catch is ChannelDropped {}

        // The device discards its partial on a drop (spec §4.2) — model it by
        // draining what did arrive before the retry.
        while await pipe.bufferedByteCount > 0 {
            _ = try await pipe.read(maxLength: 4096)
        }

        // Restart: the WHOLE object is re-sent from byte 0 and lands intact.
        let received = deviceReceive(pipe, length: object.count)
        try await channel.send(object)
        let bytes = try await withTimeout(5) { try await received.value }
        XCTAssertEqual(bytes, object)
        let written = await pipe.bytesWrittenSoFar
        XCTAssertGreaterThan(written, object.count, "the partial plus the full restart crossed the pipe")
    }

    // MARK: Cancel tears down cleanly

    func testCancelStopsDeliveryAndTearsDown() async throws {
        let object = Data(repeating: 0xAB, count: 5000)
        let pipe = PipeByteChannel(capacity: 256)  // backpressure: writer can't outrun a stalled reader
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let sender = Task { try await channel.send(object) }
        try await waitUntil(timeout: 2) { await pipe.bytesWrittenSoFar > 0 }
        // The real cancel path: stop the pump AND close the channel (the close is
        // what unblocks a write parked on backpressure — and what makes the device
        // discard its partial).
        sender.cancel()
        await channel.close()

        do {
            try await withTimeout(5) { try await sender.value }
            XCTFail("a canceled send must not report success")
        } catch {}
        let written = await pipe.bytesWrittenSoFar
        XCTAssertLessThan(written, object.count)  // delivery stopped short
    }

    // MARK: Helpers

    /// Ordered progress capture, reference-typed so the `@Sendable` tick closure
    /// can append from the sending task.
    private final class Ticks: @unchecked Sendable {
        private let lock = NSLock()
        private var storage: [Int] = []
        func append(_ value: Int) {
            lock.lock()
            storage.append(value)
            lock.unlock()
        }
        var values: [Int] {
            lock.lock()
            defer { lock.unlock() }
            return storage
        }
    }

    /// A concurrent "device" that reads `length` raw bytes off the pipe.
    private func deviceReceive(_ pipe: PipeByteChannel, length: Int) -> Task<Data, Error> {
        Task {
            var buffer = Data(capacity: length)
            while buffer.count < length {
                let chunk = try await pipe.read(maxLength: length - buffer.count)
                if chunk.isEmpty { break }  // EOF
                buffer.append(chunk)
            }
            return buffer
        }
    }

    private func waitUntil(timeout: Double, _ condition: @Sendable () async -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await condition() { return }
            await Task.yield()
        }
        XCTFail("condition not met within \(timeout)s")
    }

    private func withTimeout<T: Sendable>(_ seconds: Double, _ operation: @Sendable @escaping () async throws -> T) async throws -> T {
        try await withThrowingTaskGroup(of: T.self) { group in
            group.addTask { try await operation() }
            group.addTask {
                try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                throw TimeoutError()
            }
            let result = try await group.next()!
            group.cancelAll()
            return result
        }
    }

    private struct TimeoutError: Error {}
}
