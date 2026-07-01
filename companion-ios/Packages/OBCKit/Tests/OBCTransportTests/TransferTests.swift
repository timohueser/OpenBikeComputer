import XCTest
import OBCDomain
@testable import OBCTransport

/// End-to-end bulk transfer over the in-memory `PipeByteChannel`: raw-byte streaming
/// (no wire framing), whole-object CRC verify, offset-resume after an induced drop,
/// and clean cancel teardown — the B1 acceptance scenarios, with no hardware.
final class TransferTests: XCTestCase {
    // MARK: Round-trip (upload → device reassembles + CRC matches)

    func testUploadStreamsRawBytesAndVerifies() async throws {
        let object = Data((0..<3000).map { UInt8(($0 * 13 + 7) & 0xFF) })
        let pipe = PipeByteChannel()
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let received = deviceReceive(pipe, length: object.count)
        let handle = channel.upload(object)
        try await drainToFinish(handle)

        let bytes = try await withTimeout(5) { try await received.value }
        XCTAssertEqual(bytes, object)
        XCTAssertEqual(CRC32.checksum(bytes), CRC32.checksum(object))  // CRC the phone announces
    }

    // MARK: Round-trip (download verifies the announced CRC)

    func testDownloadVerifiesCRC() async throws {
        let object = Data((0..<2500).map { UInt8(($0 * 5) & 0xFF) })
        let pipe = PipeByteChannel()
        try await pipe.write(object)  // device streams raw bytes

        let channel = BLEChannel(channel: pipe, chunkSize: 128)
        let (_, result) = channel.download(length: object.count, expectedCRC: CRC32.checksum(object))
        let got = try await withTimeout(5) { try await result.value }
        XCTAssertEqual(got, object)
    }

    // MARK: CRC reject — an error the link CRC missed

    func testDownloadRejectsSilentlyCorruptedObject() async throws {
        let object = Data((0..<1500).map { UInt8($0 & 0xFF) })
        let pipe = PipeByteChannel()
        await pipe.corruptByte(at: 900)   // flip one bit in transit
        try await pipe.write(object)

        let channel = BLEChannel(channel: pipe, chunkSize: 128)
        let (_, result) = channel.download(length: object.count, expectedCRC: CRC32.checksum(object))
        do {
            _ = try await withTimeout(5) { try await result.value }
            XCTFail("expected crcMismatch")
        } catch let error as DeviceError {
            XCTAssertEqual(error, .crcMismatch)  // rejected, never committed
        }
    }

    // MARK: Offset-resume after a drop

    func testUploadResumesAfterDrop() async throws {
        let object = Data((0..<2000).map { UInt8($0 & 0xFF) })
        let pipe = PipeByteChannel()
        await pipe.failAfter(500)  // drop partway through
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let received = deviceReceive(pipe, length: object.count)
        let handle = channel.upload(object)

        try await waitUntil(timeout: 2) { await pipe.faultTriggered }
        let resumer = Task { for _ in 0..<200 { handle.resume(); await Task.yield() } }
        defer { resumer.cancel() }

        let bytes = try await withTimeout(5) { try await received.value }
        XCTAssertEqual(bytes, object)                        // full object despite the drop
        XCTAssertEqual(CRC32.checksum(bytes), CRC32.checksum(object))
        // Byte-exact resume: raw streaming re-sends only the bytes past the last
        // committed offset — no wasted retransmission of a whole frame.
        let written = await pipe.bytesWrittenSoFar
        XCTAssertEqual(written, object.count)
    }

    // MARK: Cancel tears down cleanly

    func testCancelStopsDeliveryAndTearsDown() async throws {
        let object = Data(repeating: 0xAB, count: 5000)
        let pipe = PipeByteChannel(capacity: 256)  // backpressure: writer can't outrun a stalled reader
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let handle = channel.upload(object)
        try await waitUntil(timeout: 2) { await pipe.bytesWrittenSoFar > 0 }
        handle.cancel()

        try await withTimeout(5) { for await _ in handle.progress {} }   // finishes, no hang
        let written = await pipe.bytesWrittenSoFar
        XCTAssertLessThan(written, object.count)                        // delivery stopped short

        var drained = 0
        while true {
            let chunk = try await pipe.read(maxLength: 4096)
            if chunk.isEmpty { break }   // channel torn down → EOF
            drained += chunk.count
        }
        XCTAssertLessThan(drained, object.count)
    }

    // MARK: Helpers

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

    private func drainToFinish(_ handle: TransferHandle) async throws {
        try await withTimeout(5) { for await _ in handle.progress {} }
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
