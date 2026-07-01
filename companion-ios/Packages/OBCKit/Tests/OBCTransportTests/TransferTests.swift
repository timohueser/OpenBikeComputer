import XCTest
@testable import OBCTransport

/// End-to-end framing over the in-memory `PipeByteChannel`: round-trip, offset
/// resume after an induced drop, and clean cancel teardown — the B1 acceptance
/// scenarios, with no hardware.
final class TransferTests: XCTestCase {
    // MARK: Round-trip (upload)

    func testUploadRoundTripsThroughFraming() async throws {
        let object = Data((0..<3000).map { UInt8(($0 * 13 + 7) & 0xFF) })
        let pipe = PipeByteChannel()
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let received = deviceReader(pipe)
        let handle = channel.upload(object, type: .route, objectID: 7)
        try await drainToFinish(handle)

        let result = try await withTimeout(5) { try await received.value }
        XCTAssertEqual(result, object)
    }

    // MARK: Round-trip (download)

    func testDownloadReassemblesObject() async throws {
        let object = Data((0..<2500).map { UInt8(($0 * 5) & 0xFF) })
        let pipe = PipeByteChannel()

        // Device side: frame the object into the pipe.
        for frame in encodeFrames(object, type: .ride, objectID: 4, chunk: 128) {
            try await pipe.write(frame)
        }

        let channel = BLEChannel(channel: pipe)
        let (_, result) = channel.download(objectID: 4)
        let got = try await withTimeout(5) { try await result.value }
        XCTAssertEqual(got, object)
    }

    // MARK: Offset-resume after a drop

    func testUploadResumesAfterDrop() async throws {
        let object = Data((0..<2000).map { UInt8($0 & 0xFF) })
        let pipe = PipeByteChannel()
        await pipe.failAfter(500)  // drop partway through
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let received = deviceReader(pipe)
        let handle = channel.upload(object, type: .ride, objectID: 3)

        // Wait for the induced drop, then nudge the paused transfer back to life.
        try await waitUntil(timeout: 2) { await pipe.faultTriggered }
        let resumer = Task { for _ in 0..<200 { handle.resume(); await Task.yield() } }
        defer { resumer.cancel() }

        let result = try await withTimeout(5) { try await received.value }
        XCTAssertEqual(result, object)                 // full object despite the drop
        let written = await pipe.bytesWrittenSoFar
        XCTAssertGreaterThan(written, object.count)     // re-sent the dropped frame
    }

    // MARK: Cancel tears down cleanly

    func testCancelStopsDeliveryAndTearsDown() async throws {
        let object = Data(repeating: 0xAB, count: 5000)
        let pipe = PipeByteChannel(capacity: 256)  // backpressure: writer can't outrun a stalled reader
        let channel = BLEChannel(channel: pipe, chunkSize: 64)

        let handle = channel.upload(object, type: .ride, objectID: 9)
        try await waitUntil(timeout: 2) { await pipe.bytesWrittenSoFar > 0 }
        handle.cancel()

        // The progress stream finishes (no hang), delivery stopped short, channel closed.
        try await withTimeout(5) { for await _ in handle.progress {} }
        let writtenBeforeCancel = await pipe.bytesWrittenSoFar
        XCTAssertLessThan(writtenBeforeCancel, object.count)

        // Drain whatever was buffered, then reads return EOF — the channel is torn down.
        var drained = 0
        while true {
            let chunk = try await pipe.read(maxLength: 4096)
            if chunk.isEmpty { break }
            drained += chunk.count
        }
        XCTAssertLessThan(drained, object.count)
    }

    // MARK: Helpers

    /// A concurrent "device" that reads frames off the pipe and reassembles them.
    private func deviceReader(_ pipe: PipeByteChannel) -> Task<Data, Error> {
        Task {
            let reader = FrameReader(channel: pipe)
            var assembler = TransferAssembler()
            while let frame = try await reader.next() {
                if try assembler.ingest(header: frame.header, payload: frame.payload) { break }
            }
            return assembler.object ?? Data()
        }
    }

    private func encodeFrames(_ object: Data, type: ObjectType, objectID: UInt16, chunk: Int) -> [Data] {
        var out: [Data] = []
        var offset = 0
        while offset < object.count {
            let end = Swift.min(offset + chunk, object.count)
            let payload = Data(object[(object.startIndex + offset)..<(object.startIndex + end)])
            out.append(FrameCodec.encode(
                type: type, objectID: objectID, totalLen: UInt32(object.count),
                offset: UInt32(offset), payload: payload
            ))
            offset = end
        }
        return out
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
