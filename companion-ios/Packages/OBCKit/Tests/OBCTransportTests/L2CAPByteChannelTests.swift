#if canImport(CoreBluetooth)
import Foundation
import Testing
@testable import OBCTransport

@Suite("L2CAP receive and control-reply liveness", .timeLimit(.minutes(1)))
struct L2CAPByteChannelTests {
    @Test("A missing control reply fails the link even without a parked stream read")
    func missingControlReply() async throws {
        let failure = AsyncPromise<Bool>()
        let channel = makeChannel { failure.fulfill(true) }
        channel.expectControlResponse(true)
        #expect(await failure.value)
        #expect(!channel.isOpen)
        await channel.close()
    }

    @Test("Receiving the control reply leaves an idle channel usable")
    func completedControlReply() async throws {
        let failure = AsyncPromise<Bool>()
        let channel = makeChannel { failure.fulfill(true) }
        channel.expectControlResponse(true)
        channel.expectControlResponse(false)
        try await Task.sleep(for: .milliseconds(1300))
        #expect(failure.current == nil)
        #expect(channel.isOpen)
        await channel.close()
    }

    @Test("A read cancelled before it parks leaves the channel usable")
    func cancellationBeforeRead() async throws {
        let failure = AsyncPromise<Bool>()
        let channel = makeChannel { failure.fulfill(true) }
        let start = AsyncPromise<Bool>()
        let task = Task {
            _ = await start.value
            return try await channel.read(maxLength: 16)
        }
        task.cancel()
        start.fulfill(true)
        await #expect(throws: CancellationError.self) { try await task.value }
        #expect(channel.isOpen)
        #expect(failure.current == nil)
        try await channel.write(Data([7, 8]))
        #expect(try await channel.read(maxLength: 16) == Data([7, 8]))
        await channel.close()
    }

    @Test("Cancelling an in-flight read does not close the channel")
    func cancellationDuringRead() async throws {
        let failure = AsyncPromise<Bool>()
        let channel = makeChannel { failure.fulfill(true) }
        let task = Task { try await channel.read(maxLength: 16) }
        // Cover waiter installation on either side of cancellation.
        await Task.yield()
        task.cancel()
        await #expect(throws: CancellationError.self) { try await task.value }
        #expect(channel.isOpen)
        #expect(failure.current == nil)
        try await channel.write(Data([9]))
        #expect(try await channel.read(maxLength: 16) == Data([9]))
        await channel.close()
    }

    private func makeChannel(onFailure: @escaping @Sendable () -> Void) -> L2CAPByteChannel {
        var input: InputStream?
        var output: OutputStream?
        Stream.getBoundStreams(withBufferSize: 1024, inputStream: &input, outputStream: &output)
        return L2CAPByteChannel(
            input: input!, output: output!, stallTimeout: 0.05, onFailure: onFailure)
    }
}
#endif
