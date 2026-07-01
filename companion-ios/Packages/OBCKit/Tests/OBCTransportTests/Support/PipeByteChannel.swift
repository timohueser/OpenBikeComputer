import Foundation
@testable import OBCTransport

/// In-memory `ByteChannel` for framing tests — a loopback where whatever is
/// written becomes readable. Stands in for the L2CAP CoC so `BLEChannel`'s
/// framing/resume/cancel run with no hardware.
///
/// Models two properties of the real CoC that the tests depend on:
///
///   • **Bounded buffer** (`capacity`) ≈ credit-based flow control — a writer can't
///     outrun a stalled reader, so a mid-transfer `cancel` genuinely stops delivery.
///   • **Atomic drop** (`failAfter`) — a failing `write` delivers *nothing*, modeling
///     the framing layer's per-frame commit (a partial frame never validates CRC, so
///     the sender re-sends it whole on resume).
actor PipeByteChannel: ByteChannel {
    private var buffer = Data()
    private let capacity: Int
    private var readWaiter: (max: Int, cont: CheckedContinuation<Data, Never>)?
    private var writeWaiter: CheckedContinuation<Void, Error>?
    private var closed = false

    private var failAt: Int?      // one-shot: the write crossing this many bytes throws once
    private var corruptAt: Int?   // one-shot: flip a bit at this delivered-byte index

    /// Total bytes accepted by `write` (test introspection).
    private(set) var bytesWrittenSoFar = 0
    /// Whether the one-shot `failAfter` drop has fired (test introspection).
    private(set) var faultTriggered = false

    init(capacity: Int = .max) { self.capacity = capacity }

    /// Arm a one-shot drop: the first `write` that would push the cumulative byte
    /// count past `bytes` throws `ChannelDropped` (delivering nothing), then heals.
    func failAfter(_ bytes: Int) { failAt = bytes }

    /// Arm a one-shot silent corruption: flip a bit in the byte at delivered index
    /// `index`. Models an error the BLE link CRC missed — only the end-to-end CRC
    /// catches it.
    func corruptByte(at index: Int) { corruptAt = index }

    func write(_ data: Data) async throws {
        if closed { throw ChannelDropped() }
        if let threshold = failAt, bytesWrittenSoFar + data.count > threshold {
            failAt = nil                 // heal — a subsequent resume succeeds
            faultTriggered = true
            throw ChannelDropped()       // atomic: deliver nothing
        }
        var data = data
        if let index = corruptAt, index >= bytesWrittenSoFar, index < bytesWrittenSoFar + data.count {
            corruptAt = nil
            let local = data.index(data.startIndex, offsetBy: index - bytesWrittenSoFar)
            data[local] ^= 0x01
        }
        // Backpressure: block until the reader drains below capacity (or we close).
        while buffer.count >= capacity, !closed {
            try await withCheckedThrowingContinuation { writeWaiter = $0 }
        }
        if closed { throw ChannelDropped() }
        bytesWrittenSoFar += data.count
        buffer.append(data)
        serveReadWaiter()
    }

    func read(maxLength: Int) async throws -> Data {
        if let chunk = drain(maxLength) { return chunk }
        if closed { return Data() }  // clean EOF
        return await withCheckedContinuation { continuation in
            readWaiter = (maxLength, continuation)
        }
    }

    func close() {
        closed = true
        readWaiter?.cont.resume(returning: Data())
        readWaiter = nil
        writeWaiter?.resume(throwing: ChannelDropped())
        writeWaiter = nil
    }

    /// Pop up to `maxLength` buffered bytes, or nil if the buffer is empty.
    private func drain(_ maxLength: Int) -> Data? {
        guard !buffer.isEmpty else { return nil }
        let n = Swift.min(maxLength, buffer.count)
        let out = Data(buffer.prefix(n))
        buffer.removeFirst(n)
        if buffer.count < capacity, let waiter = writeWaiter {  // room freed → unblock writer
            writeWaiter = nil
            waiter.resume()
        }
        return out
    }

    private func serveReadWaiter() {
        guard let waiter = readWaiter, let chunk = drain(waiter.max) else { return }
        readWaiter = nil
        waiter.cont.resume(returning: chunk)
    }
}
