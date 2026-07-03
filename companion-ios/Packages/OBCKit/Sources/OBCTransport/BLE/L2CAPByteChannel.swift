#if canImport(CoreBluetooth)
@preconcurrency import CoreBluetooth
import Foundation

/// A `ByteChannel` backed by a CoreBluetooth `CBL2CAPChannel` — the real CoC data
/// plane. The channel's `InputStream`/`OutputStream` are scheduled on a dedicated
/// run-loop thread; `read`/`write` bridge the stream delegate callbacks to
/// async/await via continuations.
///
/// > **Real path only — gated on firmware `A5`.** This compiles and is structured
/// > for bring-up, but is **not yet hardware-validated** (no device advertises the
/// > CoC PSM until `A5`). The framing above it (`BLEChannel`) is fully tested
/// > against the in-memory pipe instead.
///
/// `public` so the A5 echo harness / A9 soak rig (`EchoHarness`) wraps its own
/// `CBL2CAPChannel` into the *same* byte layer `BLETransport` uses, rather than
/// re-implementing the stream bridging.
public final class L2CAPByteChannel: NSObject, ByteChannel, StreamDelegate, @unchecked Sendable {
    /// The CoreBluetooth channel itself — retained for the byte layer's whole lifetime because
    /// **CoreBluetooth closes the L2CAP channel when the `CBL2CAPChannel` is deallocated**. Holding
    /// only its streams (as this class otherwise does) lets the object die at the end of the
    /// `didOpen` delegate callback, and macOS tears the CoC down ~milliseconds later — the peer sees
    /// `ChannelClosed` before a single byte flows.
    private let channel: CBL2CAPChannel
    private let input: InputStream
    private let output: OutputStream
    private let lock = NSLock()
    private let thread: Thread

    private var inbound = Data()
    private var outbound = Data()
    private var readWaiter: (max: Int, cont: CheckedContinuation<Data, Error>)?
    private var writeWaiter: CheckedContinuation<Void, Error>?
    private var closed = false
    private var failed = false

    public init(channel: CBL2CAPChannel) {
        self.channel = channel
        self.input = channel.inputStream
        self.output = channel.outputStream
        // A dedicated run-loop thread services the CoC's NSStream delegate events. A run loop with no
        // input sources returns from `run()` *immediately*, so pin it alive with a `Port` — without
        // this the thread exits before the streams are ever scheduled, no CoC bytes flow, and the peer
        // sees the channel close. The loop wakes periodically so a `cancel()` on teardown is prompt.
        self.thread = Thread {
            let runLoop = RunLoop.current
            runLoop.add(Port(), forMode: .default)
            while !Thread.current.isCancelled {
                runLoop.run(mode: .default, before: Date(timeIntervalSinceNow: 0.25))
            }
        }
        super.init()
        thread.start()
        // Schedule the streams on the run-loop thread and open them.
        perform(#selector(schedule), on: thread, with: nil, waitUntilDone: false)
    }

    @objc private func schedule() {
        for stream in [input, output] {
            stream.delegate = self
            stream.schedule(in: .current, forMode: .default)
            stream.open()
        }
    }

    /// Whether the channel can still move bytes — `BLETransport` checks this to
    /// decide between reusing the CoC and re-opening it (after a teardown/drop).
    public var isOpen: Bool {
        lock.lock()
        defer { lock.unlock() }
        return !closed && !failed
    }

    // MARK: ByteChannel

    public func write(_ data: Data) async throws {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Void, Error>) in
            lock.lock()
            if failed || closed { lock.unlock(); cont.resume(throwing: ChannelDropped()); return }
            outbound.append(data)
            writeWaiter = cont
            lock.unlock()
            pumpOutbound()
        }
    }

    public func read(maxLength: Int) async throws -> Data {
        try await withCheckedThrowingContinuation { (cont: CheckedContinuation<Data, Error>) in
            lock.lock()
            if !inbound.isEmpty {
                let n = Swift.min(maxLength, inbound.count)
                let out = inbound.prefix(n)
                inbound.removeFirst(n)
                lock.unlock()
                cont.resume(returning: Data(out))
                return
            }
            if closed { lock.unlock(); cont.resume(returning: Data()); return }  // clean EOF
            if failed { lock.unlock(); cont.resume(throwing: ChannelDropped()); return }
            readWaiter = (maxLength, cont)
            lock.unlock()
        }
    }

    public func close() async {
        // Lock + thread-hop live in a synchronous helper (both are flagged inside
        // an async body); resuming the waiter afterward is async-safe.
        beginClose()?.resume(returning: Data())
    }

    private func beginClose() -> CheckedContinuation<Data, Error>? {
        lock.lock()
        guard !closed else { lock.unlock(); return nil }
        closed = true
        let read = readWaiter; readWaiter = nil
        lock.unlock()
        perform(#selector(teardown), on: thread, with: nil, waitUntilDone: false)
        return read?.cont
    }

    @objc private func teardown() {
        for stream in [input, output] {
            stream.close()
            stream.remove(from: .current, forMode: .default)
        }
        thread.cancel() // let the run-loop thread exit (it polls isCancelled between wake-ups)
    }

    // MARK: StreamDelegate

    public func stream(_ aStream: Stream, handle eventCode: Stream.Event) {
        switch eventCode {
        case .hasBytesAvailable:
            drainInbound()
        case .hasSpaceAvailable:
            pumpOutbound()
        case .errorOccurred, .endEncountered:
            fail(eventCode == .endEncountered)
        default:
            break
        }
    }

    private func drainInbound() {
        var scratch = [UInt8](repeating: 0, count: 4096)
        lock.lock()
        while input.hasBytesAvailable {
            let n = input.read(&scratch, maxLength: scratch.count)
            if n > 0 { inbound.append(contentsOf: scratch[0..<n]) } else { break }
        }
        guard let waiter = readWaiter, !inbound.isEmpty else { lock.unlock(); return }
        readWaiter = nil
        let count = Swift.min(waiter.max, inbound.count)
        let out = Data(inbound.prefix(count))
        inbound.removeFirst(count)
        lock.unlock()
        waiter.cont.resume(returning: out)
    }

    private func pumpOutbound() {
        lock.lock()
        while output.hasSpaceAvailable, !outbound.isEmpty {
            let n = outbound.withUnsafeBytes { output.write($0.bindMemory(to: UInt8.self).baseAddress!, maxLength: outbound.count) }
            if n > 0 { outbound.removeFirst(n) } else { break }
        }
        if outbound.isEmpty, let cont = writeWaiter {
            writeWaiter = nil
            lock.unlock()
            cont.resume()
            return
        }
        lock.unlock()
    }

    private func fail(_ cleanEnd: Bool) {
        lock.lock()
        failed = true
        let read = readWaiter; readWaiter = nil
        let write = writeWaiter; writeWaiter = nil
        lock.unlock()
        if cleanEnd { read?.cont.resume(returning: Data()) } else { read?.cont.resume(throwing: ChannelDropped()) }
        write?.resume(throwing: ChannelDropped())
    }
}
#endif
